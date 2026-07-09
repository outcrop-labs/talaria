// Conversations + messages — durable, per-user, in Postgres.
import { db } from './db/pg'
import type { ToolCall } from '@/lib/sse-parse'

export interface ConversationRow {
  id: string
  agentModel: string
  title: string | null
  updatedAt: string
}

export interface MessageRow {
  role: 'user' | 'assistant'
  content: string
  reasoning: string
  tools: ToolCall[]
  status: 'streaming' | 'complete' | 'error'
  seq: number
  attachments?: Array<{ id: string; filename: string; mime: string; size: number }>
}

/** The user's conversations of a given kind, newest activity first. */
export async function listConversations(userId: string, kind: 'chat' | 'plan' = 'chat'): Promise<ConversationRow[]> {
  const sql = await db()
  const rows = await sql`
    select id, agent_model as "agentModel", title, updated_at as "updatedAt"
    from conversations
    where user_id = ${userId} and archived = false and kind = ${kind}
    order by updated_at desc
  `
  return rows as unknown as ConversationRow[]
}

/** A conversation (ownership-checked) + its messages in order. */
export async function getConversation(
  userId: string,
  conversationId: string,
): Promise<{ conversation: ConversationRow; messages: MessageRow[] } | null> {
  const sql = await db()
  const conv = await sql`
    select id, agent_model as "agentModel", title, updated_at as "updatedAt"
    from conversations where id = ${conversationId} and user_id = ${userId}
  `
  if (conv.length === 0) return null
  const messages = await sql`
    select role, content, reasoning, tools, status, seq, attachments
    from messages where conversation_id = ${conversationId} order by seq asc
  `
  return { conversation: conv[0] as unknown as ConversationRow, messages: messages as unknown as MessageRow[] }
}

/** Create a conversation for a user + agent (kind 'chat' or 'plan'). */
export async function createConversation(
  userId: string,
  agentModel: string,
  title: string,
  kind: 'chat' | 'plan' = 'chat',
): Promise<string> {
  const sql = await db()
  const rows = await sql`
    insert into conversations (user_id, agent_model, title, kind)
    values (${userId}, ${agentModel}, ${title}, ${kind})
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
): Promise<{ agentModel: string; kind: 'chat' | 'plan'; title: string | null } | null> {
  const sql = await db()
  const rows = await sql`
    select agent_model as "agentModel", kind, title from conversations
    where id = ${conversationId} and user_id = ${userId}
  `
  return rows.length ? (rows[0] as { agentModel: string; kind: 'chat' | 'plan'; title: string | null }) : null
}

/** Prior turns (role + content) for the gateway, oldest first. */
export async function priorMessages(conversationId: string): Promise<Array<{ role: 'user' | 'assistant'; content: string }>> {
  const sql = await db()
  const rows = await sql`
    select role, content from messages
    where conversation_id = ${conversationId} and role in ('user','assistant') and content <> ''
    order by seq asc
  `
  return rows as unknown as Array<{ role: 'user' | 'assistant'; content: string }>
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
): Promise<string> {
  const sql = await db()
  const rows = await sql`
    insert into messages (conversation_id, seq, role, content, status, attachments)
    values (${conversationId}, ${seq}, 'user', ${content}, 'complete', ${sql.json(attachments as never)})
    returning id
  `
  return (rows[0] as { id: string }).id
}

/** Create the assistant row (status='streaming'); returns its id. */
export async function insertStreamingAssistant(conversationId: string, seq: number): Promise<string> {
  const sql = await db()
  const rows = await sql`
    insert into messages (conversation_id, seq, role, status) values (${conversationId}, ${seq}, 'assistant', 'streaming')
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
