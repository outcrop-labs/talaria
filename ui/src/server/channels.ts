// Comms — one machinery behind three shapes of conversation: persistent
// #channels, Relays (named ad-hoc gatherings of humans + agents that conclude
// and archive), and human↔human DMs. Talaria owns the state; agents join as
// channel_agents and reply (streamed) when @mentioned. Message seq comes from
// a per-channel counter so concurrent writers can't collide.
import { db } from './db/pg'
import { briefsFollowMessage } from './daily-brief-stale'
import { guardAgentWrite } from './agent-writes'
import { publishChannel } from './realtime'
import { isElevatedAssistant } from './users'
import { subjectModel, type AgentSubject } from './agent-auth'
import type { Finding } from './guardrails'

export type ChannelRole = 'owner' | 'member'

/** 'channel' = persistent + ambient; 'group' = a Relay; 'dm' = human↔human. */
export type ChannelKind = 'channel' | 'group' | 'dm'

export interface Channel {
  id: string
  name: string
  topic: string | null
  kind: ChannelKind
  role: ChannelRole
  createdAt: string
  updatedAt: string
  /** For DMs: the other person (display identity). */
  peer?: { userId: string; name: string | null; email: string | null } | null
  /** Others' messages past this member's read cursor (member listings only). */
  unreadCount?: number
}

export interface ChannelMember {
  userId: string
  email: string | null
  name: string | null
  role: ChannelRole
}

export interface ChannelMessage {
  id: string
  seq: number
  authorType: 'user' | 'agent'
  author: string
  content: string
  status: 'streaming' | 'complete' | 'error'
  createdAt: string
  /** Set on thread replies — the root message they hang off. */
  threadRootId?: string | null
  editedAt?: string | null
  /** Rolled-up reactions, actors in insertion order. */
  reactions?: Array<{ emoji: string; actors: string[]; actorTypes: string[] }>
  /** On thread roots with replies: the rollup the main flow renders. */
  thread?: { count: number; authors: string[]; lastAt: string } | null
  attachments?: Array<{ id: string; filename: string; mime: string; size: number }>
  /** Confab-guard findings on an agent reply (annotate/strict modes). */
  guard?: Finding[] | null
}

/** Channels/relays/DMs the user belongs to, newest activity first. DMs carry
 *  the other person's identity so the UI can label them. unreadCount is the
 *  member's read cursor vs. others' complete messages. */
export async function listChannels(userId: string): Promise<Channel[]> {
  const sql = await db()
  const rows = (await sql`
    select c.id, c.name, c.topic, c.kind, m.role,
           c.created_at as "createdAt", c.updated_at as "updatedAt",
           pu.id as "peerId", pu.name as "peerName", pu.email as "peerEmail",
           (select count(*)::int from channel_messages msg
             where msg.channel_id = c.id and msg.seq > m.last_read_seq and msg.status = 'complete'
               and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user'))
           ) as "unreadCount"
    from channels c
    join channel_members m on m.channel_id = c.id and m.user_id = ${userId}
    join users self on self.id = ${userId}
    left join channel_members p on c.kind = 'dm' and p.channel_id = c.id and p.user_id <> ${userId}
    left join users pu on pu.id = p.user_id
    where c.archived_at is null
    order by c.updated_at desc
  `) as unknown as Array<Channel & { peerId: string | null; peerName: string | null; peerEmail: string | null }>
  return rows.map(({ peerId, peerName, peerEmail, ...c }) => ({
    ...c,
    peer: peerId ? { userId: peerId, name: peerName, email: peerEmail } : null,
  }))
}

/** Advance the member's read cursor (never backwards). */
export async function markChannelRead(channelId: string, userId: string, seq: number): Promise<void> {
  const sql = await db()
  await sql`
    update channel_members set last_read_seq = greatest(last_read_seq, ${seq})
    where channel_id = ${channelId} and user_id = ${userId}
  `
}

export async function channelRole(userId: string, channelId: string): Promise<ChannelRole | null> {
  const sql = await db()
  const rows = await sql`
    select role from channel_members where channel_id = ${channelId} and user_id = ${userId}
  `
  return rows.length ? (rows[0] as { role: ChannelRole }).role : null
}

/** Create a channel or a Relay; the creator becomes its owner (and first member). */
export async function createChannel(
  userId: string,
  name: string,
  topic?: string | null,
  kind: Exclude<ChannelKind, 'dm'> = 'channel',
): Promise<Channel> {
  const sql = await db()
  const row = await sql.begin(async (tx) => {
    const rows = await tx`
      insert into channels (name, topic, kind, created_by) values (${name}, ${topic ?? null}, ${kind}, ${userId})
      returning id, name, topic, kind, created_at as "createdAt", updated_at as "updatedAt"
    `
    const c = rows[0] as Omit<Channel, 'role'>
    await tx`insert into channel_members (channel_id, user_id, role) values (${c.id}, ${userId}, 'owner')`
    return c
  })
  return { ...row, role: 'owner' }
}

/** Find-or-create the DM between two people (deduped on the sorted id pair).
 *  Both sides are owners — a DM has no hierarchy. */
export async function ensureDm(userId: string, otherUserId: string): Promise<Channel> {
  if (userId === otherUserId) throw new Error('cannot DM yourself')
  const sql = await db()
  const dmKey = [userId, otherUserId].sort().join(':')
  const existing = (await sql`
    select id, name, topic, kind, created_at as "createdAt", updated_at as "updatedAt"
    from channels where dm_key = ${dmKey}
  `) as unknown as Array<Omit<Channel, 'role'>>
  if (existing[0]) {
    // Un-archive on revisit — a DM never really ends.
    await sql`update channels set archived_at = null where id = ${existing[0].id} and archived_at is not null`
    return { ...existing[0], role: 'owner' }
  }
  const row = await sql.begin(async (tx) => {
    const rows = await tx`
      insert into channels (name, topic, kind, dm_key, created_by)
      values ('', null, 'dm', ${dmKey}, ${userId})
      on conflict (dm_key) where dm_key is not null do update set updated_at = now()
      returning id, name, topic, kind, created_at as "createdAt", updated_at as "updatedAt"
    `
    const c = rows[0] as Omit<Channel, 'role'>
    await tx`insert into channel_members (channel_id, user_id, role) values (${c.id}, ${userId}, 'owner') on conflict do nothing`
    await tx`insert into channel_members (channel_id, user_id, role) values (${c.id}, ${otherUserId}, 'owner') on conflict do nothing`
    return c
  })
  return { ...row, role: 'owner' }
}

export async function updateChannel(channelId: string, patch: { name?: string; topic?: string | null }): Promise<void> {
  const sql = await db()
  if (patch.name !== undefined) await sql`update channels set name = ${patch.name}, updated_at = now() where id = ${channelId}`
  if (patch.topic !== undefined) await sql`update channels set topic = ${patch.topic}, updated_at = now() where id = ${channelId}`
  publishChannel(channelId, { type: 'channel' })
}

export async function archiveChannel(channelId: string): Promise<void> {
  const sql = await db()
  await sql`update channels set archived_at = now(), updated_at = now() where id = ${channelId}`
  publishChannel(channelId, { type: 'channel', deleted: true })
}

export async function deleteChannel(channelId: string): Promise<void> {
  const sql = await db()
  await sql`delete from channels where id = ${channelId}`
  publishChannel(channelId, { type: 'channel', deleted: true })
}

// ── Members (humans) & agents ────────────────────────────────────────────────
export async function listChannelMembers(channelId: string): Promise<ChannelMember[]> {
  const sql = await db()
  const rows = await sql`
    select m.user_id as "userId", u.email, u.name, m.role
    from channel_members m join users u on u.id = m.user_id
    where m.channel_id = ${channelId}
    order by (m.role = 'owner') desc, u.email asc
  `
  return rows as unknown as ChannelMember[]
}

/** Add a member by email (they must have signed in before). */
export async function addChannelMember(
  channelId: string,
  email: string,
): Promise<{ ok: true } | { ok: false; error: string }> {
  const sql = await db()
  const users = await sql`select id from users where lower(email) = ${email.trim().toLowerCase()}`
  if (users.length === 0) return { ok: false, error: 'No user with that email has signed in yet' }
  const userId = (users[0] as { id: string }).id
  await sql`
    insert into channel_members (channel_id, user_id) values (${channelId}, ${userId})
    on conflict do nothing
  `
  publishChannel(channelId, { type: 'channel' })
  return { ok: true }
}

export async function removeChannelMember(channelId: string, userId: string): Promise<void> {
  const sql = await db()
  // Never remove the owner via remove-member.
  await sql`delete from channel_members where channel_id = ${channelId} and user_id = ${userId} and role <> 'owner'`
  publishChannel(channelId, { type: 'channel' })
}

export async function listChannelAgents(channelId: string): Promise<string[]> {
  const sql = await db()
  const rows = await sql`select agent_model from channel_agents where channel_id = ${channelId} order by agent_model`
  return (rows as unknown as Array<{ agent_model: string }>).map((r) => r.agent_model)
}

/** Channels a given agent has been added to. Pass the AgentCaller where one is
 *  in hand — org-wide reach is only for a caller that PROVED its identity, and
 *  a bare string reads as proven (`subjectProven`), so `caller.model` here
 *  quietly throws the legacy flag away. */
export async function listChannelsForAgent(agent: AgentSubject): Promise<Channel[]> {
  const sql = await db()
  const model = subjectModel(agent)
  // An elevated assistant sees every live channel and relay — never DMs
  // (human↔human direct messages stay private regardless of elevation).
  if (await isElevatedAssistant(agent)) {
    const all = await sql`
      select c.id, c.name, c.topic, c.kind, c.created_at as "createdAt", c.updated_at as "updatedAt"
      from channels c where c.archived_at is null and c.kind <> 'dm'
      order by c.updated_at desc
    `
    return all as unknown as Channel[]
  }
  const rows = await sql`
    select c.id, c.name, c.topic, c.kind, c.created_at as "createdAt", c.updated_at as "updatedAt"
    from channels c join channel_agents a on a.channel_id = c.id and a.agent_model = ${model}
    where c.archived_at is null order by c.updated_at desc
  `
  return rows as unknown as Channel[]
}

/** May this agent read/post in this channel? Membership — or org-wide
 *  elevation, which still never reaches DMs. Pass the AgentCaller, not its
 *  model: elevation is only for a proven identity. */
export async function agentMayAccessChannel(channelId: string, agent: AgentSubject): Promise<boolean> {
  if ((await listChannelAgents(channelId)).includes(subjectModel(agent))) return true
  if (!(await isElevatedAssistant(agent))) return false
  const sql = await db()
  const rows = await sql`select 1 from channels where id = ${channelId} and kind <> 'dm' and archived_at is null`
  return rows.length > 0
}

export async function addChannelAgent(channelId: string, model: string): Promise<void> {
  const sql = await db()
  await sql`insert into channel_agents (channel_id, agent_model) values (${channelId}, ${model}) on conflict do nothing`
  publishChannel(channelId, { type: 'channel' })
}

export async function removeChannelAgent(channelId: string, model: string): Promise<void> {
  const sql = await db()
  await sql`delete from channel_agents where channel_id = ${channelId} and agent_model = ${model}`
  publishChannel(channelId, { type: 'channel' })
}

// ── Messages ─────────────────────────────────────────────────────────────────
const MSG_SELECT = `select id, seq, author_type as "authorType", author, content, status,
  created_at as "createdAt", attachments, guard, thread_root_id as "threadRootId",
  edited_at as "editedAt" from channel_messages`

/** Bolt reaction rollups + thread rollups onto a fetched page of messages. */
async function decorateMessages(messages: ChannelMessage[]): Promise<ChannelMessage[]> {
  if (messages.length === 0) return messages
  const sql = await db()
  const ids = messages.map((m) => m.id)
  const reactions = (await sql`
    select message_id as "messageId", emoji,
           array_agg(actor order by created_at) as actors,
           array_agg(actor_type order by created_at) as "actorTypes"
    from channel_message_reactions where message_id = any(${ids}::uuid[])
    group by message_id, emoji
    order by min(created_at)
  `) as unknown as Array<{ messageId: string; emoji: string; actors: string[]; actorTypes: string[] }>
  const threads = (await sql`
    select thread_root_id as "rootId", count(*)::int as count, max(created_at) as "lastAt",
           (array_agg(distinct author))[1:4] as authors
    from channel_messages where thread_root_id = any(${ids}::uuid[])
    group by thread_root_id
  `) as unknown as Array<{ rootId: string; count: number; lastAt: string; authors: string[] }>
  for (const m of messages) {
    const rs = reactions.filter((r) => r.messageId === m.id)
    if (rs.length) m.reactions = rs.map((r) => ({ emoji: r.emoji, actors: r.actors, actorTypes: r.actorTypes }))
    const t = threads.find((t) => t.rootId === m.id)
    if (t) m.thread = { count: t.count, authors: t.authors, lastAt: t.lastAt }
  }
  return messages
}

/** A channel's MAIN flow (thread replies live in their panels), oldest first.
 *  `sinceSeq` fetches only newer ones. `includeThreads` flattens everything
 *  back in — the distill/conclude summarizers want the whole conversation. */
export async function listChannelMessages(
  channelId: string,
  sinceSeq = -1,
  limit = 200,
  opts: { includeThreads?: boolean } = {},
): Promise<ChannelMessage[]> {
  const sql = await db()
  const rows = await sql.unsafe(
    `${MSG_SELECT} where channel_id = $1 and seq > $2 ${opts.includeThreads ? '' : 'and thread_root_id is null'} order by seq desc limit $3`,
    [channelId, sinceSeq, limit],
  )
  return decorateMessages((rows as unknown as ChannelMessage[]).reverse())
}

/** One thread: its root + replies, oldest first. */
export async function listThreadMessages(channelId: string, rootId: string): Promise<ChannelMessage[]> {
  const sql = await db()
  const rows = await sql.unsafe(
    `${MSG_SELECT} where channel_id = $1 and (id = $2 or thread_root_id = $2) order by seq asc limit 300`,
    [channelId, rootId],
  )
  return decorateMessages(rows as unknown as ChannelMessage[])
}

/** Insert a message, drawing seq from the channel's counter.
 *
 *  THE AGENT-POST GUARD LIVES HERE. `mcp post_to_channel` arrives as a tool
 *  ARGUMENT — model output that never went through a harness — and lands in a
 *  room everybody reads. `guardAgentWrite` runs the gate-safe rules over it,
 *  records what they find against the posting agent, and in strict mode stores
 *  the redacted body. It runs on `authorType === 'agent'` only: a person's
 *  message is not model output, and guard_findings.model has to keep meaning
 *  "this model's confabulation rate".
 *
 *  Inside the insert rather than at the route, so no caller can express the
 *  ungated post — and the streamed reply path costs nothing for it, because
 *  that path inserts an EMPTY row and fills it in through
 *  `updateChannelMessage`, where `guardChatReply` already stands.
 *
 *  Findings are recorded, not PINNED to the row. `channel_messages.guard` is in
 *  `MSG_SELECT`, and agents read channels through the same API humans do — so
 *  pinning a finding here would put its `snippet` (a verbatim excerpt of the
 *  flagged span) on a path back into a model's context, which is the one thing
 *  guardrails.ts forbids outright. */
export async function insertChannelMessage(
  channelId: string,
  authorType: 'user' | 'agent',
  author: string,
  content: string,
  status: 'streaming' | 'complete' = 'complete',
  attachments: unknown[] = [],
  threadRootId: string | null = null,
): Promise<ChannelMessage> {
  const sql = await db()
  const body = authorType === 'agent' ? (await guardAgentWrite('channel-post', { agent: author }, content)).text : content
  const row = await sql.begin(async (tx) => {
    const seqRows = await tx`
      update channels set msg_seq = msg_seq + 1, updated_at = now() where id = ${channelId} returning msg_seq
    `
    const seq = (seqRows[0] as { msg_seq: number }).msg_seq
    const rows = await tx`
      insert into channel_messages (channel_id, seq, author_type, author, content, status, attachments, thread_root_id)
      values (${channelId}, ${seq}, ${authorType}, ${author}, ${body}, ${status}, ${sql.json(attachments as never)}, ${threadRootId})
      returning id, seq, author_type as "authorType", author, content, status, created_at as "createdAt", attachments,
        thread_root_id as "threadRootId"
    `
    return rows[0] as unknown as ChannelMessage
  })
  publishChannel(channelId, { type: 'message', messageId: row.id, seq: row.seq })
  // THE BRIEF FOLLOWS THE CONVERSATION. A brief line says who is waiting on a
  // reply, and until this existed it learned that the reply had happened on the
  // next scheduled sweep — up to five minutes of the document telling you to
  // answer somebody you had just answered. This clears the sweep throttle and
  // rings the bell; it does not sweep (see daily-brief-stale.ts).
  briefsFollowMessage(channelId)
  return row
}

/** Toggle a reaction (add if absent, remove if present). Anyone in the room —
 *  human or agent — reacts under their own identity. */
export async function toggleReaction(
  channelId: string,
  messageId: string,
  emoji: string,
  actor: string,
  actorType: 'user' | 'agent',
): Promise<void> {
  const sql = await db()
  const removed = await sql`
    delete from channel_message_reactions
    where message_id = ${messageId} and emoji = ${emoji} and actor = ${actor} returning 1 as ok
  `
  if (removed.length === 0) {
    await sql`
      insert into channel_message_reactions (message_id, emoji, actor, actor_type)
      values (${messageId}, ${emoji}, ${actor}, ${actorType}) on conflict do nothing
    `
  }
  publishChannel(channelId, { type: 'message', messageId })
}

/** Author-gated edit: new content, edited marker, republish. */
export async function editChannelMessage(channelId: string, messageId: string, content: string): Promise<void> {
  const sql = await db()
  await sql`update channel_messages set content = ${content}, edited_at = now() where id = ${messageId}`
  publishChannel(channelId, { type: 'message', messageId })
}

/** Hard delete (author or channel owner). A thread root takes its replies
 *  with it (FK cascade) — the confirm dialog says so. */
export async function deleteChannelMessage(channelId: string, messageId: string): Promise<void> {
  const sql = await db()
  await sql`delete from channel_messages where id = ${messageId}`
  publishChannel(channelId, { type: 'message', messageId })
}

/** The row an edit/delete/react targets, for permission checks. */
export async function getChannelMessage(
  channelId: string,
  messageId: string,
): Promise<{ id: string; authorType: 'user' | 'agent'; author: string; threadRootId: string | null } | null> {
  const sql = await db()
  const rows = (await sql`
    select id, author_type as "authorType", author, thread_root_id as "threadRootId"
    from channel_messages where id = ${messageId} and channel_id = ${channelId}
  `) as unknown as Array<{ id: string; authorType: 'user' | 'agent'; author: string; threadRootId: string | null }>
  return rows[0] ?? null
}

/** Flush accumulated agent-reply state (throttled during streaming, final at end). */
export async function updateChannelMessage(
  channelId: string,
  messageId: string,
  content: string,
  status: 'streaming' | 'complete' | 'error',
): Promise<void> {
  const sql = await db()
  await sql`update channel_messages set content = ${content}, status = ${status} where id = ${messageId}`
  publishChannel(channelId, { type: 'message', messageId })
}

/** Pin confab-guard findings to an agent reply (annotate/strict); strict may
 *  also pass secret-redacted content to overwrite the saved copy. Republishes
 *  so live viewers pick the caveat up. */
export async function setChannelMessageGuard(
  channelId: string,
  messageId: string,
  findings: Finding[],
  redactedContent?: string,
): Promise<void> {
  const sql = await db()
  const guard = sql.json(findings as unknown as Parameters<typeof sql.json>[0])
  if (redactedContent !== undefined) {
    await sql`update channel_messages set guard = ${guard}, content = ${redactedContent} where id = ${messageId}`
  } else {
    await sql`update channel_messages set guard = ${guard} where id = ${messageId}`
  }
  publishChannel(channelId, { type: 'message', messageId })
}
