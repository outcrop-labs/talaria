// Comms — one machinery behind three shapes of conversation: persistent
// #channels, Relays (named ad-hoc gatherings of humans + agents that conclude
// and archive), and human↔human DMs. Talaria owns the state; agents join as
// channel_agents and reply (streamed) when @mentioned. Message seq comes from
// a per-channel counter so concurrent writers can't collide.
import { db } from './db/pg'
import { publishChannel } from './realtime'
import { isElevatedAssistant } from './users'
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

/** Channels a given agent (by model) has been added to. */
export async function listChannelsForAgent(model: string): Promise<Channel[]> {
  const sql = await db()
  // An elevated assistant sees every live channel and relay — never DMs
  // (human↔human direct messages stay private regardless of elevation).
  if (await isElevatedAssistant(model)) {
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
 *  elevation, which still never reaches DMs. */
export async function agentMayAccessChannel(channelId: string, model: string): Promise<boolean> {
  if ((await listChannelAgents(channelId)).includes(model)) return true
  if (!(await isElevatedAssistant(model))) return false
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
  created_at as "createdAt", attachments, guard from channel_messages`

/** A channel's messages, oldest first. `sinceSeq` fetches only newer ones. */
export async function listChannelMessages(channelId: string, sinceSeq = -1, limit = 200): Promise<ChannelMessage[]> {
  const sql = await db()
  const rows = await sql.unsafe(
    `${MSG_SELECT} where channel_id = $1 and seq > $2 order by seq desc limit $3`,
    [channelId, sinceSeq, limit],
  )
  return (rows as unknown as ChannelMessage[]).reverse()
}

/** Insert a message, drawing seq from the channel's counter. */
export async function insertChannelMessage(
  channelId: string,
  authorType: 'user' | 'agent',
  author: string,
  content: string,
  status: 'streaming' | 'complete' = 'complete',
  attachments: unknown[] = [],
): Promise<ChannelMessage> {
  const sql = await db()
  const row = await sql.begin(async (tx) => {
    const seqRows = await tx`
      update channels set msg_seq = msg_seq + 1, updated_at = now() where id = ${channelId} returning msg_seq
    `
    const seq = (seqRows[0] as { msg_seq: number }).msg_seq
    const rows = await tx`
      insert into channel_messages (channel_id, seq, author_type, author, content, status, attachments)
      values (${channelId}, ${seq}, ${authorType}, ${author}, ${content}, ${status}, ${sql.json(attachments as never)})
      returning id, seq, author_type as "authorType", author, content, status, created_at as "createdAt", attachments
    `
    return rows[0] as unknown as ChannelMessage
  })
  publishChannel(channelId, { type: 'message', messageId: row.id, seq: row.seq })
  return row
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
