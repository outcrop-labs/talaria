// Inbox Focus Queue — a normalized, decision-first read model over Talaria's
// existing sources. Source tables remain authoritative. This module owns only
// queue ranking, per-user focus state, constrained assistant briefs/commands,
// and allowlisted action execution.

import { createHash } from 'node:crypto'
import { db } from './db/pg'
import { actorOf, type SessionUser } from './api-guard'
import { randomToken } from './auth/session'
import { open, seal } from './secretbox'
import { boardRole, canEdit } from './boards'
import { channelRole, insertChannelMessage, markChannelRead } from './channels'
import { notifyDmMessage, notifyUserMentions, triggerAgentReplies } from './channel-replies'
import { decideAction, listPending, type PendingAction } from './google/pending-actions'
import { logAudit } from './audit'
import { completeQualityReview, getTask, type TaskStatus } from './tasks'
import { statusMeta } from './statuses'
import { routedModelFor } from './fleet-agents'
import { requestGatewayJsonObject, requestJsonObject } from './inbox-focus-assistant'
import { gatewayModelsFor } from './model-access'
import {
  asIso,
  buildInboxConversationPrompt,
  confirmationMissInvalidates,
  dedupeItems,
  deterministicProposal,
  keyOf,
  sortItems,
  validBrief,
  validateCommandObject,
} from './inbox-focus-policy'
import { approvalItems, channelItems, notificationItems, taskItems } from './inbox-focus-sources'
import type {
  FocusActionResult,
  FocusAssistant,
  FocusCommandResult,
  FocusItem,
  FocusQueue,
  FocusSourceType,
  RawFocusItem,
} from './inbox-focus-types'

export * from './inbox-focus-types'

interface FocusStateRow {
  key: string
  snoozedUntil: string | null
  contentFingerprint: string | null
  brief: unknown
  briefGeneratedAt: string | null
}

const BRIEF_RETRY_MS = 2 * 60_000
const CONFIRMATION_MS = 10 * 60_000
const UNDO_MS = 30_000

const tokenHash = (token: string): string => createHash('sha256').update(token).digest('hex')

export async function focusAssistantFor(userId: string): Promise<FocusAssistant> {
  const sql = await db()
  const rows = (await sql`
    select model, display_name as name from agent_defs
    where owner_user_id = ${userId} and enabled
    order by created_at asc limit 1
  `) as unknown as Array<{ model: string; name: string }>
  const assistant = rows[0]
  return assistant
    ? { configured: true, model: assistant.model, name: assistant.name }
    : { configured: false, model: null, name: null }
}

async function focusStates(userId: string, keys: string[]): Promise<Map<string, FocusStateRow>> {
  if (!keys.length) return new Map()
  const sql = await db()
  const rows = (await sql`
    select source_type || ':' || source_id as key,
           snoozed_until as "snoozedUntil", content_fingerprint as "contentFingerprint",
           brief, brief_generated_at as "briefGeneratedAt"
    from inbox_focus_state
    where user_id = ${userId} and source_type || ':' || source_id = any(${keys})
  `) as unknown as FocusStateRow[]
  return new Map(rows.map((row) => [row.key, row]))
}

async function claimAndGenerateBrief(userId: string, assistant: FocusAssistant, item: RawFocusItem): Promise<void> {
  if (!assistant.configured || !assistant.model) return
  const sql = await db()
  const claimed = await sql`
    insert into inbox_focus_state (user_id, source_type, source_id, content_fingerprint, brief, brief_generated_at)
    values (${userId}, ${item.sourceType}, ${item.sourceId}, ${item.sourceFingerprint}, null, now())
    on conflict (user_id, source_type, source_id) do update
      set content_fingerprint = excluded.content_fingerprint, brief = null,
          brief_generated_at = now(), updated_at = now()
      where inbox_focus_state.content_fingerprint is distinct from excluded.content_fingerprint
         or (
           inbox_focus_state.brief is null
           and (
             inbox_focus_state.brief_generated_at is null
             or inbox_focus_state.brief_generated_at < now() - (${BRIEF_RETRY_MS} * interval '1 millisecond')
           )
         )
    returning source_id
  `
  if (claimed.length) void generateBrief(userId, assistant.model, item).catch(() => {})
}

async function generateBrief(userId: string, model: string, item: RawFocusItem): Promise<void> {
  const allowed = item.actions.map((action) => ({ id: action.id, label: action.label }))
  const prompt = [
    '[Inbox Focus Queue brief. Tools are disabled.]',
    'Return one JSON object only with keys: question, recommendation, recommendedActionId.',
    'Question: one short decision-focused question grounded only in the source evidence.',
    'Recommendation: one short recommended next step. Do not claim an action was executed.',
    `recommendedActionId must be null or exactly one of: ${allowed.map((action) => action.id).join(', ') || '(none)'}.`,
    `Source: ${JSON.stringify({ type: item.sourceType, evidence: item.evidence, metadata: item.metadata })}`,
  ].join('\n')
  const brief = validBrief(await requestJsonObject(model, prompt, 4_000), item.actions)
  if (!brief) return
  const sql = await db()
  await sql`
    update inbox_focus_state
    set brief = ${sql.json(brief as never)}, brief_generated_at = now(), updated_at = now()
    where user_id = ${userId} and source_type = ${item.sourceType} and source_id = ${item.sourceId}
      and content_fingerprint = ${item.sourceFingerprint}
  `
}

async function rawFocusItems(user: SessionUser): Promise<RawFocusItem[]> {
  const [approvals, tasks, channels, notifications] = await Promise.all([
    approvalItems(user),
    taskItems(user.id),
    channelItems(user.id),
    notificationItems(user.id),
  ])
  return sortItems(dedupeItems([...approvals, ...tasks, ...channels, ...notifications]))
}

export async function listFocusQueue(
  user: SessionUser,
  options: { enrich?: boolean; includeSnoozed?: boolean } = {},
): Promise<FocusQueue> {
  const raw = await rawFocusItems(user)
  const [states, assistant] = await Promise.all([focusStates(user.id, raw.map((item) => item.key)), focusAssistantFor(user.id)])
  const now = Date.now()
  const visible = raw.filter((item) => {
    if (options.includeSnoozed) return true
    const until = states.get(item.key)?.snoozedUntil
    return !until || new Date(until).getTime() <= now
  })

  const items = visible.map((item) => {
    const state = states.get(item.key)
    const cached = state?.contentFingerprint === item.sourceFingerprint ? validBrief(state.brief, item.actions) : null
    if (!cached) return item
    return { ...item, ...cached, briefStatus: 'cached' as const }
  })

  if (options.enrich !== false && items[0]) {
    const item = items[0]
    const state = states.get(item.key)
    const claimedAt = state?.briefGeneratedAt ? new Date(state.briefGeneratedAt).getTime() : 0
    const cacheFresh = state?.contentFingerprint === item.sourceFingerprint && validBrief(state.brief, item.actions)
    if (!cacheFresh && (!claimedAt || Date.now() - claimedAt > BRIEF_RETRY_MS || state?.contentFingerprint !== item.sourceFingerprint)) {
      item.briefStatus = assistant.configured ? 'pending' : 'fallback'
      void claimAndGenerateBrief(user.id, assistant, item)
    } else if (!cacheFresh && assistant.configured) {
      item.briefStatus = 'pending'
    }
  }

  const publicItems = items.map(({ bucket: _bucket, sourceFingerprint: _fingerprint, ...item }) => item)
  return {
    items: publicItems,
    counts: {
      total: publicItems.length,
      approvals: publicItems.filter((item) => item.sourceType === 'approval').length,
      tasks: publicItems.filter((item) => item.sourceType === 'task').length,
      comms: publicItems.filter((item) => item.sourceType === 'channel').length,
      notifications: publicItems.filter((item) => item.sourceType === 'notification').length,
    },
    assistant,
  }
}

export async function focusSummary(user: SessionUser): Promise<{ count: number }> {
  const raw = await rawFocusItems(user)
  const states = await focusStates(user.id, raw.map((item) => item.key))
  const now = Date.now()
  return {
    count: raw.filter((item) => {
      const until = states.get(item.key)?.snoozedUntil
      return !until || new Date(until).getTime() <= now
    }).length,
  }
}

export async function updateFocusState(
  user: SessionUser,
  input: { sourceType: FocusSourceType; sourceId: string; snoozedUntil?: string | null; viewed?: boolean },
): Promise<boolean> {
  const item = await findFocusItemForUser(user, keyOf(input.sourceType, input.sourceId))
  if (!item) return false
  const sql = await db()
  const updateSnooze = input.snoozedUntil !== undefined
  const snoozed = input.snoozedUntil ? new Date(input.snoozedUntil) : null
  const updateViewed = input.viewed === true
  await sql`
    insert into inbox_focus_state (user_id, source_type, source_id, snoozed_until, viewed_at)
    values (${user.id}, ${input.sourceType}, ${input.sourceId}, ${updateSnooze ? snoozed : null}, ${updateViewed ? new Date() : null})
    on conflict (user_id, source_type, source_id) do update
      set snoozed_until = case when ${updateSnooze} then excluded.snoozed_until else inbox_focus_state.snoozed_until end,
          viewed_at = case when ${updateViewed} then excluded.viewed_at else inbox_focus_state.viewed_at end,
          updated_at = now()
  `
  return true
}

export async function findFocusItemForUser(user: SessionUser, key: string): Promise<RawFocusItem | null> {
  const separator = key.indexOf(':')
  if (separator <= 0) return null
  const sourceType = key.slice(0, separator) as FocusSourceType
  const sourceId = key.slice(separator + 1)
  if (!sourceId) return null
  let items: RawFocusItem[]
  if (sourceType === 'approval') items = await approvalItems(user, sourceId)
  else if (sourceType === 'task') items = await taskItems(user.id, sourceId)
  else if (sourceType === 'channel') items = await channelItems(user.id, sourceId)
  else if (sourceType === 'notification') items = await notificationItems(user.id, sourceId)
  else return null
  return items.find((item) => item.key === key) ?? null
}

async function pendingApprovalFor(user: SessionUser, id: string): Promise<PendingAction | null> {
  const pending = await listPending(user.id, user.role === 'admin')
  return pending.find((action) => action.id === id) ?? null
}

async function createDecision(input: {
  userId: string
  item: FocusItem
  instruction?: string | null
  actionId?: string | null
  agentModel?: string | null
  delegateModel?: string | null
  status: string
  proposal?: unknown
  outcome?: unknown
  confirmationTokenHash?: string | null
  expiresAt?: Date | null
}): Promise<string> {
  const sql = await db()
  const rows = (await sql`
    insert into inbox_decisions (
      user_id, source_type, source_id, instruction, action_id, agent_model, delegate_model,
      status, proposal, outcome, confirmation_token_hash, expires_at, focus_context,
      completed_at
    ) values (
      ${input.userId}, ${input.item.sourceType}, ${input.item.sourceId}, ${input.instruction ?? null},
      ${input.actionId ?? null}, ${input.agentModel ?? null}, ${input.delegateModel ?? null},
      ${input.status}, ${input.proposal === undefined ? null : sql.json(input.proposal as never)},
      ${input.outcome === undefined ? null : sql.json(input.outcome as never)},
      ${input.confirmationTokenHash ?? null}, ${input.expiresAt ?? null},
      ${sql.json({
        key: `${input.item.sourceType}:${input.item.sourceId}`,
        question: input.item.question,
        sourceHref: input.item.sourceHref,
        sourceType: input.item.sourceType,
        sourceId: input.item.sourceId,
      } as never)},
      ${input.status === 'completed' || input.status === 'failed' || input.status === 'cancelled' ? sql`now()` : null}
    ) returning id
  `) as unknown as Array<{ id: string }>
  return rows[0]!.id
}

function exactPreview(item: FocusItem, actionId: string, payload: unknown, approval: PendingAction | null): { title: string; details: unknown } {
  if (approval) {
    return {
      title: actionId === 'approve' ? approval.summary ?? approval.kind : `Reject ${approval.summary ?? approval.kind}`,
      details: { kind: approval.kind, payload: approval.payload, agentModel: approval.agentModel, isOrg: approval.isOrg },
    }
  }
  if (item.sourceType === 'channel' && actionId === 'reply') {
    return { title: 'Post this reply', details: { message: (payload as { message?: unknown } | null)?.message ?? '' } }
  }
  return { title: item.question, details: payload ?? {} }
}

async function cancelDecision(user: SessionUser, decisionId: string): Promise<FocusActionResult> {
  const sql = await db()
  const rows = await sql`
    update inbox_decisions set status = 'cancelled', completed_at = now(),
      confirmation_token_hash = null, proposal = proposal - 'confirmationCipher'
    where id = ${decisionId} and user_id = ${user.id} and status = 'proposed'
    returning id
  `
  return rows.length ? { status: 'cancelled', decisionId } : { status: 'stale', message: 'That confirmation is no longer pending.' }
}

async function undoDecision(user: SessionUser, decisionId: string): Promise<FocusActionResult> {
  const sql = await db()
  const rows = (await sql`
    select id, source_type as "sourceType", source_id as "sourceId", action_id as "actionId", outcome
    from inbox_decisions
    where id = ${decisionId} and user_id = ${user.id} and status = 'completed'
      and completed_at > now() - (${UNDO_MS} * interval '1 millisecond')
  `) as unknown as Array<{ id: string; sourceType: FocusSourceType; sourceId: string; actionId: string; outcome: unknown }>
  const decision = rows[0]
  if (!decision || decision.actionId !== 'mark_read') return { status: 'stale', message: 'Undo is no longer available.' }
  const outcome = (decision.outcome && typeof decision.outcome === 'object' ? decision.outcome : {}) as Record<string, unknown>
  let restored = false
  if (decision.sourceType === 'notification') {
    const afterReadAt = typeof outcome.afterReadAt === 'string' ? outcome.afterReadAt : null
    if (!afterReadAt) return { status: 'stale', message: 'Undo can no longer verify the original read action.' }
    const updated = await sql`
      update notifications set read_at = null
      where id = ${decision.sourceId} and user_id = ${user.id} and read_at = ${afterReadAt}
      returning id
    `
    restored = updated.length > 0
  } else if (decision.sourceType === 'channel') {
    const before = typeof outcome.beforeCursor === 'number' ? outcome.beforeCursor : Number(outcome.beforeCursor ?? 0)
    const after = typeof outcome.afterCursor === 'number' ? outcome.afterCursor : Number(outcome.afterCursor ?? 0)
    const updated = await sql`
      update channel_members set last_read_seq = ${Math.max(0, before)}
      where channel_id = ${decision.sourceId} and user_id = ${user.id} and last_read_seq = ${Math.max(0, after)}
      returning channel_id
    `
    restored = updated.length > 0
  } else {
    return { status: 'stale', message: 'This action cannot be undone.' }
  }
  if (!restored) return { status: 'stale', message: 'New activity changed this source, so the previous read state was not restored.' }
  await sql`
    update inbox_decisions set status = 'cancelled', outcome = ${sql.json({ ...outcome, undone: true } as never)}, completed_at = now()
    where id = ${decisionId}
  `
  await logAudit({
    actor: actorOf(user),
    action: 'inbox.focus.undo',
    targetType: decision.sourceType,
    targetId: decision.sourceId,
    after: { decisionId },
  })
  return { status: 'undone', decisionId }
}

async function proposedConfirmation(
  user: SessionUser,
  item: RawFocusItem,
  actionId: string,
  payload: unknown,
  approval: PendingAction | null,
  actors: { agentModel?: string | null; delegateModel?: string | null } = {},
  existingDecisionId?: string,
): Promise<FocusActionResult> {
  const token = randomToken(24)
  const expiresAt = new Date(Date.now() + CONFIRMATION_MS)
  const preview = exactPreview(item, actionId, payload, approval)
  const proposal = { preview, payload, sourceFingerprint: item.sourceFingerprint, confirmationCipher: seal(token) }
  let decisionId = existingDecisionId
  if (decisionId) {
    const sql = await db()
    const rows = await sql`
      update inbox_decisions
      set proposal = ${sql.json(proposal as never)}, confirmation_token_hash = ${tokenHash(token)},
          expires_at = ${expiresAt}, agent_model = ${actors.agentModel ?? null},
          delegate_model = ${actors.delegateModel ?? null}
      where id = ${decisionId} and user_id = ${user.id} and source_type = ${item.sourceType}
        and source_id = ${item.sourceId} and action_id = ${actionId} and status = 'proposed'
        and confirmation_token_hash is null
      returning id
    `
    if (!rows.length) return { status: 'stale', message: 'That command proposal is no longer available.' }
  } else {
    decisionId = await createDecision({
      userId: user.id,
      item,
      actionId,
      agentModel: actors.agentModel,
      delegateModel: actors.delegateModel,
      status: 'proposed',
      proposal,
      confirmationTokenHash: tokenHash(token),
      expiresAt,
    })
  }
  return {
    status: 'confirmation_required',
    decisionId,
    confirmationToken: token,
    expiresAt: expiresAt.toISOString(),
    preview,
  }
}

async function consumeConfirmation(
  user: SessionUser,
  decisionId: string,
  token: string,
  item: RawFocusItem,
  actionId: string,
): Promise<{ payload: unknown; agentModel: string | null; delegateModel: string | null } | null> {
  const sql = await db()
  const rows = (await sql`
    update inbox_decisions set status = 'confirmed', confirmed_at = now(), confirmation_token_hash = null,
      proposal = proposal - 'confirmationCipher'
    where id = ${decisionId} and user_id = ${user.id} and source_type = ${item.sourceType}
      and source_id = ${item.sourceId} and action_id = ${actionId} and status = 'proposed'
      and expires_at > now() and confirmation_token_hash = ${tokenHash(token)}
      and proposal->>'sourceFingerprint' = ${item.sourceFingerprint}
    returning proposal, agent_model as "agentModel", delegate_model as "delegateModel"
  `) as unknown as Array<{ proposal: unknown; agentModel: string | null; delegateModel: string | null }>
  const proposal = rows[0]?.proposal
  if (!proposal || typeof proposal !== 'object') {
    const pending = (await sql`
      select status, expires_at as "expiresAt",
             proposal->>'sourceFingerprint' as "sourceFingerprint"
      from inbox_decisions where id = ${decisionId} and user_id = ${user.id} limit 1
    `)[0] as { status: string; expiresAt: string | Date | null; sourceFingerprint: string | null } | undefined
    if (confirmationMissInvalidates({
      status: pending?.status ?? null,
      expiresAt: pending?.expiresAt ?? null,
      storedFingerprint: pending?.sourceFingerprint ?? null,
      currentFingerprint: item.sourceFingerprint,
    })) {
      await sql`
        update inbox_decisions set status = 'failed', outcome = ${sql.json({ error: 'Confirmation expired or source changed.' } as never)},
          confirmation_token_hash = null, proposal = proposal - 'confirmationCipher', completed_at = now()
        where id = ${decisionId} and user_id = ${user.id} and status = 'proposed'
      `
    }
    return null
  }
  return {
    payload: (proposal as { payload?: unknown }).payload,
    agentModel: rows[0]!.agentModel,
    delegateModel: rows[0]!.delegateModel,
  }
}

export async function reissueFocusConfirmation(user: SessionUser, decisionId: string): Promise<FocusActionResult> {
  const sql = await db()
  const rows = (await sql`
    select source_type as "sourceType", source_id as "sourceId", action_id as "actionId", proposal,
           confirmation_token_hash as "confirmationTokenHash", expires_at as "expiresAt"
    from inbox_decisions
    where id = ${decisionId} and user_id = ${user.id} and status = 'proposed'
      and proposal ? 'preview'
    limit 1
  `) as unknown as Array<{
    sourceType: FocusSourceType
    sourceId: string
    actionId: string
    proposal: unknown
    confirmationTokenHash: string | null
    expiresAt: string | Date | null
  }>
  const row = rows[0]
  const proposal = row?.proposal && typeof row.proposal === 'object' ? row.proposal as Record<string, unknown> : null
  if (!row || !proposal) return { status: 'stale', message: 'That confirmation is no longer pending.' }

  const item = await findFocusItemForUser(user, `${row.sourceType}:${row.sourceId}`)
  const action = item?.actions.find((candidate) => candidate.id === row.actionId)
  const approval = item?.sourceType === 'approval' ? await pendingApprovalFor(user, item.sourceId) : null
  const valid = item && action?.confirmationRequired
    && proposal.sourceFingerprint === item.sourceFingerprint
    && (item.sourceType !== 'approval' || approval !== null)
  if (!valid) {
    await sql`
      update inbox_decisions set status = 'failed', outcome = ${sql.json({ error: 'Source state or permission changed before confirmation.' } as never)},
        confirmation_token_hash = null, proposal = proposal - 'confirmationCipher', completed_at = now()
      where id = ${decisionId} and user_id = ${user.id} and status = 'proposed'
    `
    return { status: 'stale', decisionId, message: 'That source changed or is no longer available to you.' }
  }

  const preview = exactPreview(item, row.actionId, proposal.payload, approval)
  const cipher = typeof proposal.confirmationCipher === 'string' ? proposal.confirmationCipher : null
  if (cipher && row.confirmationTokenHash && row.expiresAt && new Date(row.expiresAt).getTime() > Date.now()) {
    try {
      const token = open(cipher)
      if (tokenHash(token) === row.confirmationTokenHash) {
        return {
          status: 'confirmation_required',
          decisionId,
          confirmationToken: token,
          expiresAt: new Date(row.expiresAt).toISOString(),
          preview,
        }
      }
    } catch {
      // The encrypted capability cannot be recovered; issue a fresh one below.
    }
  }

  const token = randomToken(24)
  const expiresAt = new Date(Date.now() + CONFIRMATION_MS)
  const updated = await sql`
    update inbox_decisions
    set proposal = ${sql.json({ ...proposal, preview, confirmationCipher: seal(token) } as never)},
        confirmation_token_hash = ${tokenHash(token)}, expires_at = ${expiresAt}
    where id = ${decisionId} and user_id = ${user.id} and status = 'proposed'
    returning id
  `
  if (!updated.length) return { status: 'stale', decisionId, message: 'That confirmation is no longer pending.' }
  return {
    status: 'confirmation_required',
    decisionId,
    confirmationToken: token,
    expiresAt: expiresAt.toISOString(),
    preview,
  }
}

async function commandDecision(
  user: SessionUser,
  decisionId: string,
  item: RawFocusItem,
  actionId: string,
): Promise<{ payload: unknown; agentModel: string | null; delegateModel: string | null } | null> {
  const sql = await db()
  const rows = (await sql`
    select proposal, agent_model as "agentModel", delegate_model as "delegateModel"
    from inbox_decisions
    where id = ${decisionId} and user_id = ${user.id} and source_type = ${item.sourceType}
      and source_id = ${item.sourceId} and action_id = ${actionId} and status = 'proposed'
      and instruction is not null and confirmation_token_hash is null
      and proposal->>'sourceFingerprint' = ${item.sourceFingerprint}
    limit 1
  `) as unknown as Array<{ proposal: unknown; agentModel: string | null; delegateModel: string | null }>
  const row = rows[0]
  if (!row?.proposal || typeof row.proposal !== 'object') return null
  return {
    payload: (row.proposal as { payload?: unknown }).payload,
    agentModel: row.agentModel,
    delegateModel: row.delegateModel,
  }
}

async function replayDecision(
  user: SessionUser,
  input: { decisionId: string; key: string; actionId: string },
): Promise<FocusActionResult | null> {
  const separator = input.key.indexOf(':')
  if (separator <= 0) return null
  const sql = await db()
  const rows = (await sql`
    select status, outcome, completed_at as "completedAt"
    from inbox_decisions
    where id = ${input.decisionId} and user_id = ${user.id}
      and source_type = ${input.key.slice(0, separator)} and source_id = ${input.key.slice(separator + 1)}
      and action_id = ${input.actionId}
    limit 1
  `) as unknown as Array<{ status: string; outcome: unknown; completedAt: string | null }>
  const decision = rows[0]
  if (!decision) return null
  if (decision.status === 'completed') {
    const result: FocusActionResult = { status: 'completed', decisionId: input.decisionId, result: decision.outcome }
    const completedAt = decision.completedAt ? new Date(decision.completedAt).getTime() : 0
    if (input.actionId === 'mark_read' && completedAt > Date.now() - UNDO_MS) {
      result.undo = { decisionId: input.decisionId, expiresAt: new Date(completedAt + UNDO_MS).toISOString() }
    }
    return result
  }
  if (decision.status === 'failed') {
    const outcome = decision.outcome && typeof decision.outcome === 'object' ? decision.outcome as { error?: unknown } : null
    return { status: 'failed', decisionId: input.decisionId, message: String(outcome?.error ?? 'That action failed.') }
  }
  if (decision.status === 'confirmed') {
    return { status: 'stale', decisionId: input.decisionId, message: 'The confirmation was accepted, but its final outcome could not be verified. Open the source before trying again.' }
  }
  return null
}

async function completeDecision(decisionId: string, status: 'completed' | 'failed', outcome: unknown): Promise<void> {
  const sql = await db()
  await sql`
    update inbox_decisions set status = ${status}, outcome = ${sql.json(outcome as never)}, completed_at = now(),
      confirmation_token_hash = null, proposal = proposal - 'confirmationCipher'
    where id = ${decisionId}
  `
}

async function failCommandProposal(
  user: SessionUser,
  input: { decisionId?: string; key: string; actionId: string },
  status: 'failed' | 'stale',
  message: string,
): Promise<FocusActionResult> {
  if (!input.decisionId) return { status, message }
  const separator = input.key.indexOf(':')
  if (separator <= 0) return { status, message }
  const sql = await db()
  const rows = await sql`
    update inbox_decisions
    set status = 'failed', outcome = ${sql.json({ error: message, kind: status } as never)}, completed_at = now()
    where id = ${input.decisionId} and user_id = ${user.id}
      and source_type = ${input.key.slice(0, separator)} and source_id = ${input.key.slice(separator + 1)}
      and action_id = ${input.actionId} and status = 'proposed' and instruction is not null
    returning id
  `
  return { status, message, ...(rows.length ? { decisionId: input.decisionId } : {}) }
}

async function executeAction(
  user: SessionUser,
  item: FocusItem,
  actionId: string,
  payload: unknown,
  existingDecisionId?: string,
  agentModel?: string | null,
  delegateModel?: string | null,
): Promise<FocusActionResult> {
  const sql = await db()
  const actor = actorOf(user)
  let decisionId = existingDecisionId
  let outcome: Record<string, unknown>
  const finishEarly = async (status: 'failed' | 'stale', message: string): Promise<FocusActionResult> => {
    if (existingDecisionId) {
      await completeDecision(existingDecisionId, 'failed', { error: message, kind: status })
    }
    return { status, message, ...(existingDecisionId ? { decisionId: existingDecisionId } : {}) }
  }

  try {
    if (item.sourceType === 'notification' && actionId === 'mark_read') {
      const rows = (await sql`
        update notifications set read_at = now()
        where id = ${item.sourceId} and user_id = ${user.id} and read_at is null
        returning read_at as "afterReadAt"
      `) as unknown as Array<{ afterReadAt: string }>
      if (!rows[0]) return finishEarly('stale', 'That notification is already resolved.')
      outcome = { beforeReadAt: null, afterReadAt: asIso(rows[0].afterReadAt) }
    } else if (item.sourceType === 'channel' && actionId === 'mark_read') {
      const rows = (await sql`
        select member.last_read_seq as "beforeCursor", c.msg_seq as "afterCursor"
        from channel_members member join channels c on c.id = member.channel_id
        where member.channel_id = ${item.sourceId} and member.user_id = ${user.id} and c.archived_at is null
      `) as unknown as Array<{ beforeCursor: number; afterCursor: number }>
      if (!rows[0]) return finishEarly('stale', 'That conversation is no longer available.')
      await markChannelRead(item.sourceId, user.id, rows[0].afterCursor)
      outcome = rows[0]
    } else if (item.sourceType === 'channel' && actionId === 'reply') {
      const message = String((payload as { message?: unknown } | null)?.message ?? '').trim().slice(0, 20_000)
      if (!message) return finishEarly('failed', 'The reply is empty.')
      if (!(await channelRole(user.id, item.sourceId))) return finishEarly('stale', 'You no longer have access to that conversation.')
      const author = user.email ?? user.name ?? 'user'
      const posted = await insertChannelMessage(item.sourceId, 'user', author, message)
      await markChannelRead(item.sourceId, user.id, posted.seq)
      const channel = (await sql`select name, kind from channels where id = ${item.sourceId}`)[0] as { name: string; kind: string } | undefined
      if (channel?.kind === 'dm') void notifyDmMessage(item.sourceId, user.id, user.name ?? author, message).catch(() => {})
      else if (channel) void notifyUserMentions(item.sourceId, channel.name, user.id, user.name ?? author, message).catch(() => {})
      if (channel) void triggerAgentReplies(item.sourceId, channel.name, message, null).catch(() => {})
      outcome = { messageId: posted.id, seq: posted.seq, href: item.sourceHref }
    } else if (item.sourceType === 'task' && ['approve_task', 'request_changes'].includes(actionId)) {
      const task = await getTask(item.sourceId)
      if (!task || !canEdit(await boardRole(user.id, task.boardId))) return finishEarly('stale', 'The task is unavailable or your access changed.')
      const meta = await statusMeta(task.boardId)
      if (!meta.reviewKeys.includes(task.status)) return finishEarly('stale', 'That task is no longer awaiting review.')
      const approved = actionId === 'approve_task'
      const nextStatus = approved ? meta.doneKeys[0] : meta.keys.includes('in_progress') ? 'in_progress' : meta.assignedKey
      if (!nextStatus || !meta.keys.includes(nextStatus)) return finishEarly('failed', 'This board does not have a valid destination status for the review.')
      const updated = await completeQualityReview(task.id, actor, approved ? 'approved' : 'rejected', nextStatus as TaskStatus)
      if (!updated) return finishEarly('stale', 'That task changed before the review could be completed.')
      outcome = { taskId: task.id, status: updated.status }
    } else if (item.sourceType === 'approval' && ['approve', 'reject'].includes(actionId)) {
      const decision = actionId === 'approve' ? 'approve' : 'reject'
      const result = await decideAction(item.sourceId, { id: user.id, isAdmin: user.role === 'admin' }, decision, Date.now())
      if (!result || result.status === 'forbidden' || result.status === 'pending') return finishEarly('stale', 'That approval is no longer available.')
      if (result.status === 'not_connected' || result.status === 'failed') throw new Error(result.message ?? result.status)
      outcome = result
    } else {
      return finishEarly('failed', 'That action is not available for this item.')
    }

    decisionId ??= await createDecision({
      userId: user.id,
      item,
      actionId,
      agentModel,
      delegateModel,
      status: 'completed',
      outcome,
    })
    if (existingDecisionId) await completeDecision(existingDecisionId, 'completed', outcome)
    await logAudit({
      actor,
      action: `inbox.focus.${actionId}`,
      targetType: item.sourceType,
      targetId: item.sourceId,
      targetLabel: item.question,
      after: { decisionId, agentModel: agentModel ?? null, delegateModel: delegateModel ?? null, outcome },
    })
    const result: FocusActionResult = { status: 'completed', decisionId, result: outcome }
    if (actionId === 'mark_read') {
      result.undo = { decisionId, expiresAt: new Date(Date.now() + UNDO_MS).toISOString() }
    }
    return result
  } catch (error) {
    const message = error instanceof Error ? error.message : 'The action failed.'
    if (existingDecisionId) await completeDecision(existingDecisionId, 'failed', { error: message })
    else {
      decisionId = await createDecision({
        userId: user.id,
        item,
        actionId,
        agentModel,
        delegateModel,
        status: 'failed',
        outcome: { error: message },
      })
    }
    return { status: 'failed', decisionId, message }
  }
}

export async function runFocusAction(
  user: SessionUser,
  input: {
    key?: string
    actionId?: string
    payload?: unknown
    commandDecisionId?: string
    decisionId?: string
    confirmationToken?: string
    cancelDecisionId?: string
    undoDecisionId?: string
  },
): Promise<FocusActionResult> {
  if (input.cancelDecisionId) return cancelDecision(user, input.cancelDecisionId)
  if (input.undoDecisionId) return undoDecision(user, input.undoDecisionId)
  if (!input.key || !input.actionId) return { status: 'failed', message: 'A queue item and action are required.' }
  if (input.decisionId && input.confirmationToken) {
    const replayed = await replayDecision(user, { decisionId: input.decisionId, key: input.key, actionId: input.actionId })
    if (replayed) return replayed
  }
  const item = await findFocusItemForUser(user, input.key)
  if (!item) {
    return failCommandProposal(
      user,
      { decisionId: input.commandDecisionId, key: input.key, actionId: input.actionId },
      'stale',
      'That item was already resolved or is no longer accessible.',
    )
  }
  const action = item.actions.find((candidate) => candidate.id === input.actionId)
  if (!action) {
    return failCommandProposal(
      user,
      { decisionId: input.commandDecisionId, key: input.key, actionId: input.actionId },
      'failed',
      'That action is not allowlisted for this item.',
    )
  }

  const approval = item.sourceType === 'approval' ? await pendingApprovalFor(user, item.sourceId) : null
  if (item.sourceType === 'approval' && !approval) {
    return failCommandProposal(
      user,
      { decisionId: input.commandDecisionId, key: input.key, actionId: input.actionId },
      'stale',
      'That approval is no longer pending.',
    )
  }
  const command = input.commandDecisionId
    ? await commandDecision(user, input.commandDecisionId, item, action.id)
    : null
  if (input.commandDecisionId && !command) {
    return failCommandProposal(
      user,
      { decisionId: input.commandDecisionId, key: input.key, actionId: input.actionId },
      'stale',
      'That command proposal changed or was already used.',
    )
  }
  const payload = command ? command.payload : input.payload
  const actors = command
    ? { agentModel: command.agentModel, delegateModel: command.delegateModel }
    : { agentModel: null, delegateModel: null }

  if (action.confirmationRequired) {
    if (!input.confirmationToken || !input.decisionId) {
      return proposedConfirmation(user, item, action.id, payload, approval, actors, input.commandDecisionId)
    }
    const confirmed = await consumeConfirmation(user, input.decisionId, input.confirmationToken, item, action.id)
    if (!confirmed) return { status: 'stale', message: 'That confirmation expired or was already used.' }
    return executeAction(user, item, action.id, confirmed.payload, input.decisionId, confirmed.agentModel, confirmed.delegateModel)
  }

  return executeAction(user, item, action.id, payload, input.commandDecisionId, actors.agentModel, actors.delegateModel)
}

async function validDelegate(
  userId: string,
  requested: string | null | undefined,
  tier: string | null | undefined,
): Promise<string | null> {
  if (!requested) return null
  const sql = await db()
  const rows = await sql`
    select 1 from agent_defs where model = ${requested} and enabled
      and (owner_user_id is null or owner_user_id = ${userId}) limit 1
  `
  if (!rows.length) return null
  return routedModelFor(requested, tier)
}

async function commandFromModel(
  model: string,
  prompt: string,
  allowedActionIds: ReadonlySet<string>,
  signal?: AbortSignal,
): Promise<{ kind: 'clarification' | 'proposal'; message: string; actionId?: string; payload?: Record<string, unknown> } | null> {
  return validateCommandObject(await requestJsonObject(model, prompt, 6_000, signal), allowedActionIds)
}

async function validResponseModel(user: SessionUser, requested: string | null | undefined): Promise<string | null> {
  if (!requested) return null
  const allowed = await gatewayModelsFor(user.role)
  if (!allowed.some((model) => model.id === requested)) throw new Error('That response model is no longer available to this account.')
  return requested
}

async function commandFromGatewayModel(
  user: SessionUser,
  model: string,
  prompt: string,
  allowedActionIds: ReadonlySet<string>,
  signal?: AbortSignal,
): Promise<{ kind: 'clarification' | 'proposal'; message: string; actionId?: string; payload?: Record<string, unknown> } | null> {
  const value = await requestGatewayJsonObject(model, prompt, `user:${user.email ?? user.id}`, signal)
  return validateCommandObject(value, allowedActionIds)
}

export async function runFocusCommand(
  user: SessionUser,
  input: {
    key: string
    instruction: string
    delegateModel?: string | null
    delegateTier?: string | null
    responseModel?: string | null
    mode?: 'normal' | 'fast' | 'plan'
    attachmentContext?: string
    history?: Array<{ role: 'user' | 'assistant'; content: string }>
    signal?: AbortSignal
  },
): Promise<FocusCommandResult | null> {
  const item = await findFocusItemForUser(user, input.key)
  if (!item) return null
  const assistant = await focusAssistantFor(user.id)
  const delegateModel = await validDelegate(user.id, input.delegateModel, input.delegateTier)
  const responseModel = await validResponseModel(user, input.responseModel)
  const mode = input.mode ?? 'normal'
  const deterministic = mode === 'plan' ? null : deterministicProposal(item, input.instruction)
  const allowedActionIds = new Set(deterministic ? [deterministic.actionId] : [])
  let response: { kind: 'clarification' | 'proposal'; message: string; actionId?: string; payload?: Record<string, unknown> } | null = null
  let specialistResponse: { kind: 'clarification' | 'proposal'; message: string; actionId?: string; payload?: Record<string, unknown> } | null = null

  const sharedPrompt = buildInboxConversationPrompt({
    instruction: `${input.instruction}${input.attachmentContext ?? ''}`,
    focus: {
      key: item.key,
      question: item.question,
      sourceHref: item.sourceHref,
      evidence: item.evidence,
      metadata: item.metadata,
    },
    history: input.history ?? [],
    allowedActionIds: [...allowedActionIds],
  })

  if (delegateModel && mode !== 'fast') {
    specialistResponse = await commandFromModel(
      delegateModel,
      ['[Inbox Focus Queue specialist consultation. Tools are disabled. Do not execute anything.]', sharedPrompt].join('\n'),
      allowedActionIds,
      input.signal,
    )
  }

  if (mode === 'fast' && deterministic) {
    response = { kind: 'proposal', ...deterministic }
  } else if (responseModel || (assistant.configured && assistant.model)) {
    const prompt = [
      '[Inbox Focus Queue command. Tools are disabled. Do not execute anything.]',
      `[Response mode: ${mode}. ${mode === 'plan' ? 'Return a plan or clarification only; no executable action is allowed.' : mode === 'fast' ? 'Be brief and direct.' : 'Balance clarity and actionability.'}]`,
      'You are the personal assistant and final orchestrator. Assess the bounded specialist suggestion, if any.',
      `Specialist suggestion: ${JSON.stringify(specialistResponse)}`,
      sharedPrompt,
    ].join('\n')
    response = responseModel
      ? await commandFromGatewayModel(user, responseModel, prompt, allowedActionIds, input.signal)
      : await commandFromModel(assistant.model!, prompt, allowedActionIds, input.signal)
  }

  response ??= specialistResponse
  response ??= deterministic
    ? { kind: 'proposal', ...deterministic }
    : {
        kind: 'clarification',
        message: assistant.configured
          ? 'I could not safely map that instruction to an available action. Please clarify the intended outcome.'
          : 'Your personal assistant is not configured. You can still use the safe actions on this card or open the source.',
      }

  const decisionId = await createDecision({
    userId: user.id,
    item,
    instruction: input.instruction,
    actionId: response.actionId ?? null,
    agentModel: responseModel ?? assistant.model,
    delegateModel,
    status: response.actionId ? 'proposed' : 'completed',
    proposal: response.actionId
      ? { message: response.message, payload: response.payload ?? null, sourceFingerprint: item.sourceFingerprint }
      : undefined,
    outcome: response.actionId ? undefined : { kind: 'clarification' },
  })

  return { ...response, decisionId, assistant, consultedModel: delegateModel }
}
