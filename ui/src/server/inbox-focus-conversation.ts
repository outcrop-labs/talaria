import { db } from './db/pg'
import { actorOf, type SessionUser } from './api-guard'
import {
  insertStreamingAssistant,
  insertUserMessage,
  nextSeq,
  touchConversation,
  updateAssistant,
} from './conversations'
import { streamReply } from './inbox-focus-assistant'
import {
  findFocusItemForUser,
  focusAssistantFor,
  reissueFocusConfirmation,
  runFocusCommand,
} from './inbox-focus'
import {
  buildInboxConversationPrompt,
  INBOX_HISTORY_MAX_TURNS,
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
import { gatewayModelsFor } from './model-access'
import { effortsForModel } from './model-efforts'
import { personaConfiguredEffort } from './harness/persona'
import { attachmentTextBlocks, canAccessUpload, resolveAttachments } from './uploads'
import { refBlocks, resolveRefs, type MessageRef } from './refs'
import type {
  FocusActionResult,
  FocusCommandResponse,
  InboxCommandEvent,
  InboxConversationPage,
  InboxFocusContext,
  InboxMessageMetadata,
  InboxMessageAttachment,
  InboxTimelineEntry,
  FocusSourceType,
} from './inbox-focus-types'

export { buildInboxConversationPrompt, INBOX_HISTORY_MAX_TURNS, limitInboxModelHistory }

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

// ── Conversation instances ───────────────────────────────────────────────────
//
// The panel's conversation is SEGMENTED: many instances per owner, picked from
// a dropdown, rather than one ever-growing thread. Segmentation IS the context
// strategy — a fresh instance is how old context is shed, chosen by the person
// rather than imposed by a budget. Everything here is scoped by ownership and
// `kind = 'inbox'` so an instance id from the dropdown can never address one of
// the owner's ordinary chat conversations.

/** One row of the panel's chat picker. The preview is the FIRST user message —
 *  "what did I start this chat about" — not the last, which for a working
 *  thread is usually a fragment of the task at hand. */
export interface InboxConversationSummary {
  id: string
  preview: string
  createdAt: string
  updatedAt: string
}

const CONVERSATION_PREVIEW_CHARS = 80

export async function listInboxConversations(userId: string): Promise<InboxConversationSummary[]> {
  const sql = await db()
  const rows = (await sql`
    select c.id, c.created_at as "createdAt", c.updated_at as "updatedAt",
           (select left(m.content, ${CONVERSATION_PREVIEW_CHARS}) from messages m
             where m.conversation_id = c.id and m.role = 'user' and m.status = 'complete' and m.content <> ''
             order by m.seq asc limit 1) as preview
    from conversations c
    where c.user_id = ${userId} and c.kind = 'inbox' and c.archived = false
    order by c.updated_at desc
    limit 50
  `) as unknown as Array<InboxConversationSummary & { preview: string | null }>
  return rows.map((row) => ({ ...row, preview: row.preview ?? '' }))
}

export async function createInboxConversation(userId: string, agentModel: string | null): Promise<string> {
  const sql = await db()
  const rows = (await sql`
    insert into conversations (user_id, agent_model, title, kind)
    values (${userId}, ${agentModel ?? 'inbox-assistant'}, 'Inbox', 'inbox')
    returning id
  `) as unknown as Array<{ id: string }>
  return rows[0]!.id
}

export async function archiveInboxConversation(userId: string, conversationId: string): Promise<boolean> {
  const sql = await db()
  const rows = (await sql`
    update conversations set archived = true, updated_at = now()
    where id = ${conversationId} and user_id = ${userId} and kind = 'inbox' and archived = false
    returning id
  `) as unknown as Array<{ id: string }>
  return rows.length > 0
}

/** The instance a request may use: the requested one when it is the owner's
 *  live inbox conversation, else their most recently touched one. Null when
 *  they have none — the READ path stops here (an empty page, no row created);
 *  the WRITE path (`resolveInboxConversationForWrite`) starts a fresh one. */
async function ownedInboxConversationId(userId: string, requested: string | null | undefined): Promise<string | null> {
  const sql = await db()
  if (requested) {
    const rows = (await sql`
      select id from conversations
      where id = ${requested} and user_id = ${userId} and kind = 'inbox' and archived = false
      limit 1
    `) as unknown as Array<{ id: string }>
    if (rows[0]) return rows[0].id
  }
  const latest = (await sql`
    select id from conversations
    where user_id = ${userId} and kind = 'inbox' and archived = false
    order by updated_at desc limit 1
  `) as unknown as Array<{ id: string }>
  return latest[0]?.id ?? null
}

async function resolveInboxConversationForWrite(
  userId: string,
  agentModel: string | null,
  requested: string | null | undefined,
): Promise<string> {
  const existing = await ownedInboxConversationId(userId, requested)
  return existing ?? createInboxConversation(userId, agentModel)
}

/** The model's history for ONE inbox conversation instance.
 *
 *  THE CONTEXT STRATEGY, in one place (the bounds live beside
 *  `limitInboxModelHistory`, which applies them): segmentation first — the
 *  owner picks the instance from the panel's chat picker, and starting a
 *  fresh one is how old context is shed. Within an instance: a turn window,
 *  and ATTACHMENTS EXPAND ONLY WHILE FRESH — full text for the last two user
 *  turns (the "keep asking about the doc I just attached" loop), a bare
 *  `[attached: …]` marker after that. Re-expanding every file on every turn
 *  forever was the single biggest silent bloat in this prompt: one 6k-char
 *  attachment rode along in full until it aged out of the turn window, on
 *  every command, answering questions that had nothing to do with it.
 *    · QUICK ACTIONS NEVER ENTER — decisions, proposals, confirmations and
 *      undos are timeline rows, not messages, so the noise the owner clicks
 *      through cannot become the model's context. That is a property of this
 *      loader reading `messages` only, and it is deliberate.
 */
async function recentInboxHistory(conversationId: string): Promise<InboxModelTurn[]> {
  const sql = await db()
  const rows = (await sql`
    select role, content, attachments from messages
    where conversation_id = ${conversationId} and role in ('user', 'assistant')
      and status = 'complete' and content <> ''
    order by seq desc limit ${INBOX_HISTORY_MAX_TURNS}
  `) as unknown as Array<InboxModelTurn & { attachments: unknown }>
  const oldestFirst = rows.reverse()
  // Which user turns are "fresh" enough to carry their attachment text: the
  // LAST TWO user rows in the window. Everything older keeps a marker.
  const freshUserRows = new Set(
    oldestFirst
      .filter((row) => row.role === 'user')
      .slice(-2)
      .map((row) => oldestFirst.indexOf(row)),
  )
  const markerFor = (attachments: unknown): string => {
    const names = publicAttachments(attachments).map((a) => a.filename)
    return names.length ? `\n[attached: ${names.join(', ')}]` : ''
  }
  const mapped = await Promise.all(
    oldestFirst.map(async (row, index) => ({
      role: row.role,
      content:
        row.role === 'user'
          ? freshUserRows.has(index)
            ? `${row.content}${refBlocks(row.attachments)}${await attachmentTextBlocks(row.attachments)}`
            : `${row.content}${markerFor(row.attachments)}`
          : row.content,
    })),
  )
  return limitInboxModelHistory(mapped)
}

function publicAttachments(value: unknown): InboxMessageAttachment[] {
  if (!Array.isArray(value)) return []
  return value.flatMap((item) => {
    if (!item || typeof item !== 'object') return []
    const attachment = item as Record<string, unknown>
    if (
      typeof attachment.id !== 'string'
      || typeof attachment.filename !== 'string'
      || typeof attachment.mime !== 'string'
      || typeof attachment.size !== 'number'
    ) return []
    return [{
      id: attachment.id,
      filename: attachment.filename,
      mime: attachment.mime,
      size: attachment.size,
      ...(attachment.refType === 'kb-doc' || attachment.refType === 'artifact' ? { refType: attachment.refType } : {}),
    }]
  })
}

async function validatedResponseModel(user: SessionUser, requested: string | null | undefined): Promise<string | null> {
  if (!requested) return null
  const allowed = await gatewayModelsFor(user.role)
  if (!allowed.some((model) => model.id === requested)) throw new Error('That response model is no longer available to this account.')
  return requested
}

/** The effort the ANSWERING model may be asked for. Same rule as
 *  `validatedResponseModel`: the pick is checked against what the model's own
 *  metadata vouches for (`effortsForModel` resolves the assistant persona to
 *  the model actually serving it), and a pick that fails the check is an
 *  error rather than a silent drop — the sender has a picker built on the same
 *  metadata, so a mismatch means one of the two is stale and both should say
 *  so. Null (no pick) is always fine and means the model's default. */
async function validatedEffort(model: string | null, effort: string | null | undefined): Promise<string | null> {
  if (!effort) return null
  if (!model) throw new Error('Your assistant is not configured, so it cannot honor an effort pick.')
  const efforts = await effortsForModel(model)
  if (!efforts.includes(effort)) {
    throw new Error(
      `That effort ("${effort}") is not available on ${model}${efforts.length ? ` — offered: ${efforts.join(', ')}` : ''}.`,
    )
  }
  return effort
}

/** The persona-configured default for the answering model, held against its
 *  published levels — the same rule `/api/chat` applies when a chat sender
 *  made no pick. Null for anything that is not a persona with a configured
 *  effort, and for a configured level the model no longer publishes. */
async function defaultEffortFor(model: string | null): Promise<string | null> {
  if (!model) return null
  const configured = await personaConfiguredEffort(model).catch(() => null)
  if (!configured) return null
  return (await effortsForModel(model)).includes(configured) ? configured : null
}

function messageEntry(input: {
  id: string
  role: 'user' | 'assistant'
  content: string
  status: 'streaming' | 'complete' | 'error'
  createdAt: string
  metadata: InboxMessageMetadata
  attachments?: InboxMessageAttachment[]
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
    /** Which view the assistant panel is floating over (see surfaceBrief). */
    surface?: string | null
    delegateModel?: string | null
    responseModel?: string | null
    mode?: 'normal' | 'fast' | 'plan'
    /** Reasoning effort the owner picked, validated against the answering
     *  model's supported levels (see `validatedEffort`). */
    effort?: string | null
    attachmentIds?: string[]
    refs?: MessageRef[]
    /** Which conversation instance this command belongs to — the panel's chat
     *  picker. Validated against the owner's live inbox conversations; a null
     *  or stale id falls back to their most recent instance, creating one
     *  only when none exists. */
    conversationId?: string | null
    signal?: AbortSignal
  },
): AsyncGenerator<InboxCommandEvent> {
  const assistant = await focusAssistantFor(user.id)
  const responseModel = await validatedResponseModel(user, input.responseModel)
  // The owner's explicit pick, else the assistant persona's CONFIGURED default
  // (the agent editor's effort beside the assistant's model) — never both, and
  // the default only when the answering model IS the persona that carries it
  // (`personaConfiguredEffort` answers null for any other id, including a
  // picked response model, whose effort is nobody's to default).
  const answering = responseModel ?? assistant.model
  const effort = input.effort
    ? await validatedEffort(answering, input.effort)
    : await defaultEffortFor(answering)
  const mode = input.mode ?? 'normal'
  const modeInstruction = mode === 'plan'
    ? '\n\n[Plan mode: answer with a concise plan or clarifying question. Do not propose or imply execution.]'
    : mode === 'fast'
      ? '\n\n[Fast mode: answer directly and keep the response brief.]'
      : ''
  const conversationId = await resolveInboxConversationForWrite(user.id, assistant.model, input.conversationId)
  const item = input.focusKey ? await findFocusItemForUser(user, input.focusKey) : null
  if (input.focusKey && !item) {
    yield { type: 'error', message: 'That focus item changed or was already resolved.' }
    return
  }

  const focus = item ? focusContext(item) : null
  const requestedAttachmentIds = input.attachmentIds ?? []
  const attachmentAccess = await Promise.all(requestedAttachmentIds.map((id) => canAccessUpload(id, {
    userId: user.id,
    who: user.email ?? user.name,
    isAdmin: user.role === 'admin',
  })))
  const uploads = await resolveAttachments(requestedAttachmentIds.filter((_, index) => attachmentAccess[index]))
  const referenceChips = await resolveRefs(user, input.refs ?? [])
  const messageAttachments = [...uploads, ...referenceChips]
  const attachmentContext = `${refBlocks(messageAttachments)}${await attachmentTextBlocks(messageAttachments)}`
  const visibleAttachments = publicAttachments(messageAttachments)
  const history = await recentInboxHistory(conversationId)
  const createdAt = new Date().toISOString()
  const userMetadata: InboxMessageMetadata = {
    focus,
    actor: { type: 'human', id: user.id, label: actorOf(user) },
    responseModel,
    mode,
    effort,
  }
  const userSeq = await nextSeq(conversationId)
  const userMessageId = await insertUserMessage(conversationId, userSeq, input.instruction, messageAttachments, user.id, userMetadata as unknown as Record<string, unknown>)
  const assistantMetadata: InboxMessageMetadata = {
    focus,
    actor: { type: 'assistant', id: assistant.model, label: assistant.name ?? 'your assistant' },
    delegateModel: input.delegateModel ?? null,
    responseModel,
    mode,
    effort,
  }
  const assistantMessageId = await insertStreamingAssistant(conversationId, userSeq + 1, assistantMetadata as unknown as Record<string, unknown>)
  await touchConversation(conversationId)
  yield {
    type: 'conversation',
    conversationId,
    entry: messageEntry({ id: userMessageId, role: 'user', content: input.instruction, status: 'complete', createdAt, metadata: userMetadata, attachments: visibleAttachments }),
  }
  yield {
    type: 'status',
    label: mode === 'plan' ? 'Planning with your assistant' : mode === 'fast' ? 'Answering quickly' : focus ? 'Reviewing the active decision' : 'Thinking with your assistant',
  }

  try {
    let content: string
    let result: FocusCommandResponse | undefined
    /** True once a delta has gone out, so the tail does not re-send the reply. */
    let streamed = false
    if (item) {
      const command = await runFocusCommand(user, {
        key: item.key,
        instruction: input.instruction,
        delegateModel: input.delegateModel,
        responseModel,
        mode,
        effort,
        attachmentContext,
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
        instruction: `${input.instruction}${modeInstruction}${attachmentContext}`,
        surface: input.surface ?? null,
        focus: null,
        history,
        allowedActionIds: [],
      })
      // STREAMED, AND THE CONTENT EVENTS GO OUT AS THEY ARRIVE. This used to
      // await the whole reply and then hand the finished string to `chunkText`
      // below, which is not streaming: the status line sat there for the entire
      // turn with nothing under it, and then the answer landed all at once. On
      // a model that takes twenty seconds that reads as the assistant not
      // replying — which is exactly how it was reported.
      //
      // `streamed` is set so the tail of this function knows not to chunk a
      // reply the panel has already been given token by token.
      const model = responseModel ?? (assistant.configured ? assistant.model : null)
      if (!model) {
        content = 'Your personal assistant is not configured yet. You can still use the safe actions in the Focus Queue.'
      } else {
        const caller = responseModel ? `user:${user.email ?? user.id}` : `inbox:${model}`
        const reply = streamReply(model, [{ role: 'user', content: prompt }], caller, { max: 20_000, parentSignal: input.signal, effort })
        let accumulated = ''
        for (;;) {
          const next = await reply.next()
          if (next.done) {
            // The generator RETURNS the run's guarded text. Preferred over the
            // deltas we relayed: a redacted or repaired reply is what should be
            // persisted, even though the raw one is what was already on screen.
            content = next.value ?? accumulated
            break
          }
          accumulated += next.value
          streamed = true
          yield { type: 'content', text: next.value }
        }
        if (!content) content = 'Your assistant is temporarily unavailable. No tools or mutations were attempted.'
      }
    }

    await setMessageMetadata(assistantMessageId, assistantMetadata)
    await updateAssistant(assistantMessageId, { content, reasoning: '', tools: [], status: 'complete' })
    // The focus-command branch produces a finished proposal message rather than
    // a stream, so it is still chunked here. A streamed reply is already on
    // screen and re-sending it would print the answer twice.
    if (!streamed) for (const text of chunkText(content)) yield { type: 'content', text }
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
    const message = error instanceof Error ? error.message : 'Your assistant could not finish that response.'
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
  attachments: unknown
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
             m.role, m.content, m.status as "messageStatus", m.metadata, m.attachments,
             null::text as "decisionStatus", null::text as "actionId", null::text as instruction,
             null::jsonb as proposal, null::jsonb as outcome, null::jsonb as focus,
             null::timestamptz as "expiresAt", null::timestamptz as "completedAt"
      from messages m where m.conversation_id = ${conversationId} and m.role in ('user', 'assistant')
      union all
      select 'decision'::text as "recordType", d.id, d.created_at as "createdAt",
             null::text as role, null::text as content, null::text as "messageStatus", null::jsonb as metadata, null::jsonb as attachments,
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

/** One instance's timeline page. READ-ONLY: an owner with no instances yet
 *  gets an empty page (conversationId null) rather than a conversation row
 *  created by a GET — instances are created by an explicit action (the
 *  picker's "New chat") or by the first thing written. */
export async function getInboxConversation(
  user: SessionUser,
  cursor?: string | null,
  conversationId?: string | null,
): Promise<InboxConversationPage> {
  const resolved = await ownedInboxConversationId(user.id, conversationId)
  if (!resolved) return { conversationId: null, entries: [], nextCursor: null, working: false }
  const { rows, hasMore } = await timelineRows(resolved, cursor)
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
        attachments: publicAttachments(row.attachments),
      })
    } else {
      const record = await timelineRecordForDecision(user, row)
      if (record) records.push(record)
    }
  }
  const oldest = rows.at(-1)
  return {
    conversationId: resolved,
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
  // No conversation resolution here, deliberately: the decision already
  // carries the instance it was made in (`linkDecision`/`recordInboxSnooze`
  // write it at insert), and under segmentation there is no "the user's one
  // conversation" to re-link it to. The earlier re-link was legacy from before
  // the column existed.
  const sql = await db()
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
  const conversationId = await resolveInboxConversationForWrite(user.id, assistant.model, null)
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
