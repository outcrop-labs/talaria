// Conversations + messages — durable, per-user, in Postgres.
import { db } from './db/pg'
import { refBlocks } from './refs'
import { attachmentTextBlocks } from './uploads'
import type { ToolCall } from '@/lib/sse-parse'
import type { Finding } from './guardrails'

export interface ConversationRow {
  id: string
  agentModel: string
  title: string | null
  updatedAt: string
  /** An assistant reply is streaming right now (the agent is working). */
  working: boolean
  /** The LATEST assistant turn errored — the thread needs a re-run. */
  failed: boolean
  /** Multiplayer plans: your standing on it. Absent/owner for your own. */
  role?: 'owner' | 'collaborator'
  /** For plans shared WITH you: who owns it (display label). */
  ownerLabel?: string | null
}

export interface MessageRow {
  role: 'user' | 'assistant'
  content: string
  reasoning: string
  tools: ToolCall[]
  status: 'streaming' | 'complete' | 'error'
  seq: number
  attachments?: Array<{ id: string; filename: string; mime: string; size: number }>
  /** Who wrote a user turn (multiplayer plans show voices apart). */
  authorUserId?: string | null
  authorLabel?: string | null
  /** Confab-guard findings on an assistant reply (annotate/strict modes). */
  guard?: Finding[] | null
  metadata?: Record<string, unknown>
}

/** THE THREE KINDS OF DURABLE CONVERSATION.
 *
 *  'chat' is one person and an agent. 'plan' is a shared thinking surface with a
 *  living document beside it. 'research' is the same shape as a plan — several
 *  people, one agent, one document that grows — over a cited report rather than
 *  a plan, and shared through `research_members` rather than
 *  `conversation_members`. */
export type ConversationKind = 'chat' | 'plan' | 'research'

/** The user's conversations of a given kind, newest activity first. `working`
 *  marks threads with a reply streaming right now — the sidebar shows them and
 *  selecting the agent lands on them instead of a fresh thread. */
export async function listConversations(userId: string, kind: ConversationKind = 'chat'): Promise<ConversationRow[]> {
  const sql = await db()
  // Plans are multiplayer: you see your own AND ones shared with you (with the
  // owner's label for the sidebar). Chats stay strictly your own.
  const rows = await sql`
    select c.id, c.agent_model as "agentModel", c.title, c.updated_at as "updatedAt",
           exists(
             select 1 from messages m
             where m.conversation_id = c.id and m.role = 'assistant' and m.status = 'streaming'
               and m.created_at > now() - interval '10 minutes'
           ) as working,
           coalesce((
             select m.status = 'error' from messages m
             where m.conversation_id = c.id and m.role = 'assistant'
             order by m.seq desc limit 1
           ), false) as failed,
           case when c.user_id = ${userId} then 'owner' else 'collaborator' end as role,
           case when c.user_id = ${userId} then null else coalesce(o.name, o.email) end as "ownerLabel"
    from conversations c
    join users o on o.id = c.user_id
    where c.archived = false and c.kind = ${kind}
      and (c.user_id = ${userId} or (${kind} = 'plan' and exists(
        select 1 from conversation_members cm where cm.conversation_id = c.id and cm.user_id = ${userId}
      )))
    order by c.updated_at desc
  `
  return rows as unknown as ConversationRow[]
}

/** Your standing on a PLAN conversation: owner, collaborator, or none.
 *  (Chats never resolve through this — they stay owner-only.) */
export async function planRole(userId: string, conversationId: string): Promise<'owner' | 'collaborator' | null> {
  const sql = await db()
  const rows = (await sql`
    select case when c.user_id = ${userId} then 'owner'
                when cm.user_id is not null then 'collaborator' end as role
    from conversations c
    left join conversation_members cm on cm.conversation_id = c.id and cm.user_id = ${userId}
    where c.id = ${conversationId} and c.kind = 'plan'
  `) as unknown as Array<{ role: 'owner' | 'collaborator' | null }>
  return rows[0]?.role ?? null
}

export interface PlanMember {
  userId: string
  name: string | null
  email: string | null
  role: 'owner' | 'collaborator'
}

export async function listPlanMembers(conversationId: string): Promise<PlanMember[]> {
  const sql = await db()
  return (await sql`
    select u.id as "userId", u.name, u.email, 'owner' as role
    from conversations c join users u on u.id = c.user_id where c.id = ${conversationId}
    union all
    select u.id as "userId", u.name, u.email, 'collaborator' as role
    from conversation_members cm join users u on u.id = cm.user_id
    where cm.conversation_id = ${conversationId}
  `) as unknown as PlanMember[]
}

export async function addPlanMember(conversationId: string, userId: string): Promise<void> {
  const sql = await db()
  await sql`
    insert into conversation_members (conversation_id, user_id)
    values (${conversationId}, ${userId}) on conflict do nothing
  `
}

export async function removePlanMember(conversationId: string, userId: string): Promise<void> {
  const sql = await db()
  await sql`delete from conversation_members where conversation_id = ${conversationId} and user_id = ${userId}`
}

/** A conversation + its messages in order. Access: yours — or a plan you're a
 *  member of (multiplayer). User turns carry author identity for display. */
export async function getConversation(
  userId: string,
  conversationId: string,
): Promise<{ conversation: ConversationRow; messages: MessageRow[] } | null> {
  const sql = await db()
  const conv = await sql`
    select c.id, c.agent_model as "agentModel", c.title, c.updated_at as "updatedAt",
           case when c.user_id = ${userId} then 'owner' else 'collaborator' end as role
    from conversations c
    where c.id = ${conversationId} and c.kind in ('chat', 'plan', 'research')
      and (
        c.user_id = ${userId}
        or (c.kind = 'plan' and exists(
          select 1 from conversation_members cm where cm.conversation_id = c.id and cm.user_id = ${userId}
        ))
        -- RESEARCH ACCESS FOLLOWS THE RUN, not a second membership list.
        -- research_members already says who the run is shared with; deriving
        -- from it means a person cannot keep the conversation after losing the
        -- report, which two lists would eventually allow. (No backticks in here:
        -- this is inside a template literal and one would end the string.)
        or (c.kind = 'research' and exists(
          select 1 from research_runs r
            left join research_members rm on rm.run_id = r.id and rm.user_id = ${userId}
           where r.conversation_id = c.id
             and (r.owner_user_id = ${userId} or rm.user_id is not null)
        ))
      )
  `
  if (conv.length === 0) return null
  const messages = await sql`
    select m.role, m.content, m.reasoning, m.tools, m.status, m.seq, m.attachments, m.guard, m.metadata,
           m.author_user_id as "authorUserId", coalesce(u.name, u.email) as "authorLabel"
    from messages m left join users u on u.id = m.author_user_id
    where m.conversation_id = ${conversationId} order by m.seq asc
  `
  return { conversation: conv[0] as unknown as ConversationRow, messages: messages as unknown as MessageRow[] }
}

/** Create a conversation for a user + agent (kind 'chat' or 'plan'). An
 *  explicit plan template (plan surface only) is the highest link in the
 *  template resolution chain; null falls through to the agent's binding. */
export async function createConversation(
  userId: string,
  agentModel: string,
  title: string,
  kind: ConversationKind = 'chat',
  planTemplateId: string | null = null,
): Promise<string> {
  const sql = await db()
  const rows = await sql`
    insert into conversations (user_id, agent_model, title, kind, plan_template_id)
    values (${userId}, ${agentModel}, ${title}, ${kind}, ${kind === 'plan' ? planTemplateId : null})
    returning id
  `
  return (rows[0] as { id: string }).id
}

/** Verify a conversation belongs to the user; return its agent model. */
export async function ownedConversationModel(userId: string, conversationId: string): Promise<string | null> {
  return (await ownedConversation(userId, conversationId))?.agentModel ?? null
}

/** Ownership-checked conversation identity: agent model + kind + title. */
export async function ownedConversation(
  userId: string,
  conversationId: string,
): Promise<{ agentModel: string; kind: ConversationKind; title: string | null } | null> {
  const sql = await db()
  const rows = await sql`
    select agent_model as "agentModel", kind, title from conversations
    where id = ${conversationId} and user_id = ${userId} and kind in ('chat', 'plan')
  `
  return rows.length ? (rows[0] as { agentModel: string; kind: ConversationKind; title: string | null }) : null
}

/** Like ownedConversation, but a PLAN also admits its collaborators — chats
 *  stay strictly owner-only. Returns the plan owner's id for doc/index ACL. */
export async function accessibleConversation(
  userId: string,
  conversationId: string,
): Promise<{ agentModel: string; kind: ConversationKind; title: string | null; ownerUserId: string; role: 'owner' | 'collaborator'; planTemplateId: string | null } | null> {
  const sql = await db()
  const rows = await sql`
    select agent_model as "agentModel", kind, title, user_id as "ownerUserId",
           plan_template_id as "planTemplateId",
           case when user_id = ${userId} then 'owner' else 'collaborator' end as role
    from conversations c
    where id = ${conversationId} and kind in ('chat', 'plan')
      and (user_id = ${userId} or (kind = 'plan' and exists(
      select 1 from conversation_members cm where cm.conversation_id = c.id and cm.user_id = ${userId}
    )))
  `
  return rows.length
    ? (rows[0] as { agentModel: string; kind: ConversationKind; title: string | null; ownerUserId: string; role: 'owner' | 'collaborator'; planTemplateId: string | null })
    : null
}

/** Prior turns (role + content) for the gateway, oldest first. Multiplayer
 *  plans prefix user turns with the author's name so the agent can tell
 *  voices apart; 1:1 threads stay plain. Attached knowledge/artifact refs
 *  ride along on every rebuild — a queued turn or resume never loses them. */
export async function priorMessages(conversationId: string): Promise<Array<{ role: 'user' | 'assistant'; content: string }>> {
  const sql = await db()
  const rows = (await sql`
    select m.role, m.content, m.attachments, coalesce(u.name, u.email) as "authorLabel",
           (select count(*)::int from conversation_members cm where cm.conversation_id = m.conversation_id) as members
    from messages m left join users u on u.id = m.author_user_id
    where m.conversation_id = ${conversationId} and m.role in ('user','assistant')
      and (m.content <> '' or m.attachments <> '[]'::jsonb)
    order by m.seq asc
  `) as unknown as Array<{ role: 'user' | 'assistant'; content: string; attachments: unknown; authorLabel: string | null; members: number }>
  const multiVoice = rows.some((r) => r.members > 0)
  // Textual file uploads re-read their bytes on every rebuild — scope that to
  // the recent tail; ref chips carry their content inline and always ride.
  const FILE_TAIL = 12
  const mapped = await Promise.all(
    rows.map(async (r, i) => {
      const voiced = multiVoice && r.role === 'user' && r.authorLabel ? `${r.authorLabel}: ${r.content}` : r.content
      const files = r.role === 'user' && i >= rows.length - FILE_TAIL ? await attachmentTextBlocks(r.attachments) : ''
      return { role: r.role, content: `${voiced}${r.role === 'user' ? refBlocks(r.attachments) : ''}${files}` }
    }),
  )
  return mapped.filter((m) => m.content.trim() !== '')
}

/** Next sequence number for a conversation. */
export async function nextSeq(conversationId: string): Promise<number> {
  const sql = await db()
  const rows = await sql`select coalesce(max(seq), -1) + 1 as next from messages where conversation_id = ${conversationId}`
  return (rows[0] as { next: number }).next
}

/** Insert the user's turn; returns the message id (activity indexing keys on it). */
export async function insertUserMessage(
  conversationId: string,
  seq: number,
  content: string,
  attachments: unknown[] = [],
  authorUserId: string | null = null,
  metadata: Record<string, unknown> = {},
): Promise<string> {
  const sql = await db()
  const rows = await sql`
    insert into messages (conversation_id, seq, role, content, status, attachments, author_user_id, metadata)
    values (${conversationId}, ${seq}, 'user', ${content}, 'complete', ${sql.json(attachments as never)}, ${authorUserId}, ${sql.json(metadata as never)})
    returning id
  `
  return (rows[0] as { id: string }).id
}

/** The conversation's in-flight assistant reply, if any. A streaming row
 *  older than 10 minutes is a crashed persist — mark it errored and report
 *  none, so a dead stream can never wedge the conversation's turn-taking. */
export async function activeStreamingAssistant(conversationId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`
    select id, created_at < now() - interval '10 minutes' as stale
    from messages
    where conversation_id = ${conversationId} and role = 'assistant' and status = 'streaming'
    order by seq desc limit 1
  `) as unknown as Array<{ id: string; stale: boolean }>
  const row = rows[0]
  if (!row) return null
  if (row.stale) {
    await sql`update messages set status = 'error' where id = ${row.id}`
    return null
  }
  return row.id
}

/** Create the assistant row (status='streaming'); returns its id. */
export async function insertStreamingAssistant(conversationId: string, seq: number, metadata: Record<string, unknown> = {}): Promise<string> {
  const sql = await db()
  const rows = await sql`
    insert into messages (conversation_id, seq, role, status, metadata)
    values (${conversationId}, ${seq}, 'assistant', 'streaming', ${sql.json(metadata as never)})
    returning id
  `
  return (rows[0] as { id: string }).id
}

/** Flush accumulated assistant state (throttled during streaming, final at end). */
export async function updateAssistant(
  messageId: string,
  data: { content: string; reasoning: string; tools: ToolCall[]; status: 'streaming' | 'complete' | 'error' },
): Promise<void> {
  const sql = await db()
  await sql`
    update messages set
      content = ${data.content},
      reasoning = ${data.reasoning},
      tools = ${sql.json(data.tools as unknown as Parameters<typeof sql.json>[0])},
      status = ${data.status}
    where id = ${messageId}
  `
}

/** Pin confab-guard findings to a reply (annotate/strict). Metadata only —
 *  the UI renders a caveat; transcripts never see it. */
export async function setMessageGuard(messageId: string, findings: Finding[]): Promise<void> {
  const sql = await db()
  await sql`update messages set guard = ${sql.json(findings as unknown as Parameters<typeof sql.json>[0])} where id = ${messageId}`
}

/** Bump the conversation's updated_at (and set a title from the first turn if empty). */
export async function touchConversation(conversationId: string, titleIfEmpty?: string): Promise<void> {
  const sql = await db()
  if (titleIfEmpty) {
    await sql`
      update conversations set updated_at = now(),
        title = coalesce(nullif(title, ''), ${titleIfEmpty})
      where id = ${conversationId}
    `
  } else {
    await sql`update conversations set updated_at = now() where id = ${conversationId}`
  }
}
