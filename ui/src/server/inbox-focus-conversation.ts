import { db } from './db/pg'
import { actorOf, type SessionUser } from './api-guard'
import {
  insertStreamingAssistant,
  insertUserMessage,
  nextSeq,
  touchConversation,
  updateAssistant,
} from './conversations'
import { requestText } from './inbox-focus-assistant'
import {
  findFocusItemForUser,
  focusAssistantFor,
  reissueFocusConfirmation,
  runFocusCommand,
} from './inbox-focus'
import {
  buildInboxConversationPrompt,
  limitInboxModelHistory,
  type InboxModelTurn,
} from './inbox-focus-policy'
import {
  buildInboxTimeline,
  decodeInboxTimelineCursor,
  encodeInboxTimelineCursor,
  normalizeInboxTimelineTimestamp,
  type InboxTimelineRecord,
} from '@/lib/inbox-focus-timeline'
import type {
  FocusActionResult,
  FocusCommandResponse,
  InboxCommandEvent,
  InboxConversationPage,
  InboxFocusContext,
  InboxMessageMetadata,
  InboxTimelineEntry,
  FocusSourceType,
} from './inbox-focus-types'

export { buildInboxConversationPrompt, limitInboxModelHistory }

const PAGE_SIZE = 30
const UNDO_MS = 30_000
const g = globalThis as unknown as { __talariaInboxFocusLocks?: Set<string> }
const locks = g.__talariaInboxFocusLocks ??= new Set<string>()

export function acquireInboxFocusLock(userId: string): (() => void) | null {
  if (locks.has(userId)) return null
  locks.add(userId)
  let released = false
  return () => {
    if (released) return
    released = true
    locks.delete(userId)
  }
}

function focusContext(item: { key: string; question: string; sourceHref: string; sourceType: FocusSourceType; sourceId: string }): InboxFocusContext {
  return {
    key: item.key,
    question: item.question,
    sourceHref: item.sourceHref,
    sourceType: item.sourceType,
    sourceId: item.sourceId,
  }
}

export async function ensureInboxConversation(userId: string, agentModel: string | null): Promise<string> {
  const sql = await db()
  const rows = await sql`
    insert into conversations (user_id, agent_model, title, kind)
    values (${userId}, ${agentModel ?? 'inbox-assistant'}, 'Inbox', 'inbox')
    on conflict (user_id) where kind = 'inbox' and archived = false
    do update set agent_model = excluded.agent_model
    returning id
  `
  return (rows[0] as { id: string }).id
}

async function recentInboxHistory(conversationId: string): Promise<InboxModelTurn[]> {
  const sql = await db()
  const rows = (await sql`
    select role, content from messages
    where conversation_id = ${conversationId} and role in ('user', 'assistant')
      and status = 'complete' and content <> ''
    order by seq desc limit 20
  `) as unknown as InboxModelTurn[]
  return limitInboxModelHistory(rows.reverse())
}

function messageEntry(input: {
  id: string
  role: 'user' | 'assistant'
  content: string
  status: 'streaming' | 'complete' | 'error'
  createdAt: string
  metadata: InboxMessageMetadata
}): InboxTimelineEntry {
  return buildInboxTimeline([{ recordType: 'message', ...input }]).find((entry) => entry.kind === 'message')!
}

function chunkText(value: string, size = 96): string[] {
  const chunks: string[] = []
  for (let index = 0; index < value.length; index += size) chunks.push(value.slice(index, index + size))
  return chunks
}

async function linkDecision(
  decisionId: string,
  conversationId: string,
  userMessageId: string,
  assistantMessageId: string,
): Promise<void> {
  const sql = await db()
  await sql`
    update inbox_decisions
    set conversation_id = ${conversationId}, user_message_id = ${userMessageId}, assistant_message_id = ${assistantMessageId}
    where id = ${decisionId}
  `
}

async function setMessageMetadata(messageId: string, metadata: InboxMessageMetadata): Promise<void> {
  const sql = await db()
  await sql`update messages set metadata = ${sql.json(metadata as never)} where id = ${messageId}`
}

export async function* runInboxConversationCommand(
  user: SessionUser,
  input: {
    instruction: string
    focusKey?: string | null
    delegateModel?: string | null
    delegateTier?: string | null
    signal?: AbortSignal
  },
): AsyncGenerator<InboxCommandEvent> {
  const assistant = await focusAssistantFor(user.id)
  const conversationId = await ensureInboxConversation(user.id, assistant.model)
  const item = input.focusKey ? await findFocusItemForUser(user, input.focusKey) : null
  if (input.focusKey && !item) {
    yield { type: 'error', message: 'That focus item changed or was already resolved.' }
    return
  }

  const focus = item ? focusContext(item) : null
  const history = await recentInboxHistory(conversationId)
  const createdAt = new Date().toISOString()
  const userMetadata: InboxMessageMetadata = {
    focus,
    actor: { type: 'human', id: user.id, label: actorOf(user) },
  }
  const userSeq = await nextSeq(conversationId)
  const userMessageId = await insertUserMessage(conversationId, userSeq, input.instruction, [], user.id, userMetadata as unknown as Record<string, unknown>)
  const assistantMetadata: InboxMessageMetadata = {
    focus,
    actor: { type: 'assistant', id: assistant.model, label: assistant.name ?? 'Scout' },
    delegateModel: input.delegateModel ?? null,
  }
  const assistantMessageId = await insertStreamingAssistant(conversationId, userSeq + 1, assistantMetadata as unknown as Record<string, unknown>)
  await touchConversation(conversationId)
  yield {
    type: 'conversation',
    conversationId,
    entry: messageEntry({ id: userMessageId, role: 'user', content: input.instruction, status: 'complete', createdAt, metadata: userMetadata }),
  }
  yield { type: 'status', label: focus ? 'Reviewing the active decision' : 'Thinking with Scout' }

  try {
    let content: string
    let result: FocusCommandResponse | undefined
    if (item) {
      const command = await runFocusCommand(user, {
        key: item.key,
        instruction: input.instruction,
        delegateModel: input.delegateModel,
        delegateTier: input.delegateTier,
        history,
        signal: input.signal,
      })
      if (!command) {
        result = { status: 'stale', message: 'That focus item changed or was already resolved.' }
        content = result.message
      } else {
        result = command
        content = command.message
        assistantMetadata.delegateModel = command.consultedModel ?? null
        assistantMetadata.decisionId = command.decisionId
        await linkDecision(command.decisionId, conversationId, userMessageId, assistantMessageId)
        if (command.consultedModel) yield { type: 'status', label: `Consulted ${command.consultedModel}` }
      }
    } else {
      const prompt = buildInboxConversationPrompt({
        instruction: input.instruction,
        focus: null,
        history,
        allowedActionIds: [],
      })
      content = assistant.configured && assistant.model
        ? await requestText(assistant.model, [{ role: 'user', content: prompt }], 20_000, input.signal)
          ?? 'Scout is temporarily unavailable. No tools or mutations were attempted.'
        : 'Your personal assistant is not configured yet. You can still use the safe actions in the Focus Queue.'
    }

    await setMessageMetadata(assistantMessageId, assistantMetadata)
    await updateAssistant(assistantMessageId, { content, reasoning: '', tools: [], status: 'complete' })
    for (const text of chunkText(content)) yield { type: 'content', text }
    if (result && !('status' in result) && result.kind === 'proposal') {
      const activity = await timelineEntryForDecision(user, result.decisionId)
      if (activity) yield { type: 'activity', entry: activity }
    }
    const assistantEntry = messageEntry({
      id: assistantMessageId,
      role: 'assistant',
      content,
      status: 'complete',
      createdAt: new Date().toISOString(),
      metadata: assistantMetadata,
    })
    yield { type: 'done', conversationId, entry: assistantEntry, ...(result ? { result } : {}) }
  } catch (error) {
    const message = error instanceof Error ? error.message : 'Scout could not finish that response.'
    await updateAssistant(assistantMessageId, { content: '', reasoning: '', tools: [], status: 'error' }).catch(() => {})
    yield { type: 'error', message }
  }
}

interface TimelineSqlRow {
  recordType: 'message' | 'decision'
  id: string
  createdAt: string | Date
  role: 'user' | 'assistant' | null
  content: string | null
  messageStatus: 'streaming' | 'complete' | 'error' | null
  metadata: InboxMessageMetadata | null
  decisionStatus: string | null
  actionId: string | null
  instruction: string | null
  proposal: unknown
  outcome: unknown
  focus: InboxFocusContext | null
  expiresAt: string | Date | null
  completedAt: string | Date | null
}

async function timelineRows(conversationId: string, cursor?: string | null): Promise<{ rows: TimelineSqlRow[]; hasMore: boolean }> {
  const sql = await db()
  const before = cursor ? decodeInboxTimelineCursor(cursor) : null
  const rows = (await sql`
    select * from (
      select 'message'::text as "recordType", m.id, m.created_at as "createdAt",
             m.role, m.content, m.status as "messageStatus", m.metadata,
             null::text as "decisionStatus", null::text as "actionId", null::text as instruction,
             null::jsonb as proposal, null::jsonb as outcome, null::jsonb as focus,
             null::timestamptz as "expiresAt", null::timestamptz as "completedAt"
      from messages m where m.conversation_id = ${conversationId} and m.role in ('user', 'assistant')
      union all
      select 'decision'::text as "recordType", d.id, d.created_at as "createdAt",
             null::text as role, null::text as content, null::text as "messageStatus", null::jsonb as metadata,
             d.status as "decisionStatus", d.action_id as "actionId", d.instruction,
             d.proposal, d.outcome, d.focus_context as focus, d.expires_at as "expiresAt", d.completed_at as "completedAt"
      from inbox_decisions d where d.conversation_id = ${conversationId} and d.focus_context is not null
    ) timeline
    where (${before?.createdAt ?? null}::timestamptz is null
      or timeline."createdAt" < ${before?.createdAt ?? null}::timestamptz
      or (timeline."createdAt" = ${before?.createdAt ?? null}::timestamptz and timeline.id::text < ${before?.id ?? null}))
    order by timeline."createdAt" desc, timeline.id desc
    limit ${PAGE_SIZE + 1}
  `) as unknown as TimelineSqlRow[]
  return { rows: rows.slice(0, PAGE_SIZE), hasMore: rows.length > PAGE_SIZE }
}

async function timelineRecordForDecision(
  user: SessionUser,
  row: TimelineSqlRow,
  currentResult?: FocusActionResult,
): Promise<InboxTimelineRecord | null> {
  if (row.recordType !== 'decision' || !row.focus || !row.decisionStatus) return null
  let status = row.decisionStatus
  let confirmationToken = currentResult?.confirmationToken
  let expiresAt = currentResult?.expiresAt
    ?? (row.expiresAt ? normalizeInboxTimelineTimestamp(row.expiresAt) : undefined)
  let proposal = row.proposal
  const proposalObject = proposal && typeof proposal === 'object' ? proposal as Record<string, unknown> : null
  if (status === 'proposed' && proposalObject?.preview && !confirmationToken) {
    const reissued = await reissueFocusConfirmation(user, row.id)
    if (reissued.status === 'confirmation_required') {
      confirmationToken = reissued.confirmationToken
      expiresAt = reissued.expiresAt
      proposal = { ...proposalObject, preview: reissued.preview }
    } else if (reissued.status === 'stale') {
      status = 'failed'
    }
  }
  const completedAt = row.completedAt ? new Date(row.completedAt).getTime() : 0
  const undoExpiresAt = row.actionId === 'mark_read' && status === 'completed' && completedAt > Date.now() - UNDO_MS
    ? new Date(completedAt + UNDO_MS).toISOString()
    : currentResult?.undo?.expiresAt
  return {
    recordType: 'decision',
    id: row.id,
    createdAt: normalizeInboxTimelineTimestamp(row.createdAt),
    status,
    actionId: row.actionId,
    instruction: row.instruction,
    proposal,
    outcome: row.outcome,
    focus: row.focus,
    ...(confirmationToken ? { confirmationToken } : {}),
    ...(expiresAt ? { expiresAt } : {}),
    ...(undoExpiresAt ? { undoExpiresAt } : {}),
  }
}

export async function getInboxConversation(user: SessionUser, cursor?: string | null): Promise<InboxConversationPage> {
  const assistant = await focusAssistantFor(user.id)
  const conversationId = await ensureInboxConversation(user.id, assistant.model)
  const { rows, hasMore } = await timelineRows(conversationId, cursor)
  const records: InboxTimelineRecord[] = []
  for (const row of rows) {
    if (row.recordType === 'message' && row.role && row.messageStatus) {
      records.push({
        recordType: 'message',
        id: row.id,
        createdAt: normalizeInboxTimelineTimestamp(row.createdAt),
        role: row.role,
        content: row.content ?? '',
        status: row.messageStatus,
        metadata: row.metadata ?? {},
      })
    } else {
      const record = await timelineRecordForDecision(user, row)
      if (record) records.push(record)
    }
  }
  const oldest = rows.at(-1)
  return {
    conversationId,
    entries: buildInboxTimeline(records),
    nextCursor: hasMore && oldest
      ? encodeInboxTimelineCursor(normalizeInboxTimelineTimestamp(oldest.createdAt), oldest.id)
      : null,
    working: rows.some((row) => row.recordType === 'message' && row.messageStatus === 'streaming'),
  }
}

export async function timelineEntryForDecision(
  user: SessionUser,
  decisionId: string,
  currentResult?: FocusActionResult,
): Promise<InboxTimelineEntry | null> {
  const assistant = await focusAssistantFor(user.id)
  const conversationId = await ensureInboxConversation(user.id, assistant.model)
  const sql = await db()
  await sql`
    update inbox_decisions set conversation_id = ${conversationId}
    where id = ${decisionId} and user_id = ${user.id}
  `
  const rows = (await sql`
    select 'decision'::text as "recordType", d.id, d.created_at as "createdAt",
           null::text as role, null::text as content, null::text as "messageStatus", null::jsonb as metadata,
           d.status as "decisionStatus", d.action_id as "actionId", d.instruction,
           d.proposal, d.outcome, d.focus_context as focus, d.expires_at as "expiresAt", d.completed_at as "completedAt"
    from inbox_decisions d where d.id = ${decisionId} and d.user_id = ${user.id}
  `) as unknown as TimelineSqlRow[]
  const record = rows[0] ? await timelineRecordForDecision(user, rows[0], currentResult) : null
  return record ? buildInboxTimeline([record]).find((entry) => entry.kind === 'activity') ?? null : null
}

export async function attachTimelineToActionResult(user: SessionUser, result: FocusActionResult): Promise<FocusActionResult> {
  if (!result.decisionId) return result
  const timelineEntry = await timelineEntryForDecision(user, result.decisionId, result)
  return timelineEntry ? { ...result, timelineEntry } : result
}

export async function recordInboxSnooze(
  user: SessionUser,
  input: { sourceType: FocusSourceType; sourceId: string; snoozedUntil: string },
): Promise<InboxTimelineEntry | null> {
  const item = await findFocusItemForUser(user, `${input.sourceType}:${input.sourceId}`)
  if (!item) return null
  const assistant = await focusAssistantFor(user.id)
  const conversationId = await ensureInboxConversation(user.id, assistant.model)
  const sql = await db()
  const rows = (await sql`
    insert into inbox_decisions (
      user_id, source_type, source_id, action_id, status, outcome, focus_context, conversation_id, completed_at
    ) values (
      ${user.id}, ${item.sourceType}, ${item.sourceId}, 'snooze', 'completed',
      ${sql.json({ snoozedUntil: input.snoozedUntil, message: `Snoozed until ${input.snoozedUntil}` } as never)},
      ${sql.json(focusContext(item) as never)}, ${conversationId}, now()
    ) returning id
  `) as unknown as Array<{ id: string }>
  return timelineEntryForDecision(user, rows[0]!.id)
}
