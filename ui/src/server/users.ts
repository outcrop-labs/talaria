// Users, roles, and per-agent access (durable in Postgres).
//
// Roles: 'admin' | 'member'. Admins are designated by AUTH_ADMIN_EMAILS; anyone
// else who signs in (already gated by AUTH_ALLOWED_*) becomes a member.
//
// Per-agent access policy: admins → all agents. A member with NO access rows →
// all agents (open by default); with rows → restricted to exactly those. Admins
// manage member allow-lists (UI later).

import { db } from './db/pg'

export type Role = 'admin' | 'member'

export interface User {
  id: string
  sub: string
  email: string | null
  name: string | null
  picture: string | null
  role: Role
}

export interface Identity {
  sub: string
  email: string | null
  name: string | null
  picture: string | null
}

function adminEmails(): string[] {
  return (process.env.AUTH_ADMIN_EMAILS ?? '')
    .split(',')
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean)
}

/** Upsert the identity into `users`, assigning/keeping its role. */
export async function upsertUser(identity: Identity): Promise<User> {
  const sql = await db()
  const isAdmin = !!identity.email && adminEmails().includes(identity.email.toLowerCase())
  const role: Role = isAdmin ? 'admin' : 'member'
  const rows = await sql`
    insert into users (sub, email, name, picture, role, last_seen_at)
    values (${identity.sub}, ${identity.email}, ${identity.name}, ${identity.picture}, ${role}, now())
    on conflict (sub) do update set
      email = excluded.email,
      -- a display name the user set (≠ email) survives logins; the provider
      -- identity only fills the unfriendly defaults (empty, or name = email).
      name = case
        when users.name is null or users.name = '' or users.name = users.email then excluded.name
        else users.name
      end,
      picture = excluded.picture,
      last_seen_at = now(),
      -- promote admin-listed users; otherwise keep whatever role they have.
      role = case when ${role} = 'admin' then 'admin' else users.role end
    returning id, sub, email, name, picture, role
  `
  return rows[0] as User
}

/** Set a user's display name (profile setting). */
export async function setUserName(userId: string, name: string): Promise<void> {
  const sql = await db()
  await sql`update users set name = ${name} where id = ${userId}`
}

/** Preferred gateway model for AI drafting (copilot); null = server default. */
export async function getPreferredModel(userId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`select preferred_model as m from users where id = ${userId}`) as unknown as Array<{ m: string | null }>
  return rows[0]?.m ?? null
}

export async function setPreferredModel(userId: string, model: string | null): Promise<void> {
  const sql = await db()
  await sql`update users set preferred_model = ${model} where id = ${userId}`
}

/** Everyone who has signed in — for people pickers (share, invite, channels). */
export async function listUsers(): Promise<Array<{ id: string; email: string | null; name: string | null }>> {
  const sql = await db()
  const rows = await sql`select id, email, name from users order by lower(coalesce(email, name, '')) asc`
  return rows as unknown as Array<{ id: string; email: string | null; name: string | null }>
}

export interface AdminUser {
  id: string
  email: string | null
  name: string | null
  role: Role
  lastSeenAt: string
  createdAt: string
  /** Empty = all agents (open by default); non-empty = restricted to these. */
  agentModels: string[]
  /** May mint LLM-gateway API keys (admins always may, regardless). */
  canMintKeys: boolean
  /** Nav routes this member may NOT reach (empty = all views). */
  deniedViews: string[]
  /** Email is in AUTH_ADMIN_EMAILS — role is pinned to admin at every login. */
  pinnedAdmin: boolean
}

/** The admin console's user list: roles, activity, and agent allow-lists. */
export async function listUsersAdmin(): Promise<AdminUser[]> {
  const sql = await db()
  const rows = await sql`
    select u.id, u.email, u.name, u.role, u.can_mint_keys as "canMintKeys", u.denied_views as "deniedViews",
           u.last_seen_at as "lastSeenAt", u.created_at as "createdAt",
           coalesce(array_agg(a.agent_model) filter (where a.agent_model is not null), '{}') as "agentModels"
    from users u left join user_agent_access a on a.user_id = u.id
    group by u.id
    order by lower(coalesce(u.email, u.name, '')) asc
  `
  const admins = adminEmails()
  return (rows as unknown as AdminUser[]).map((u) => ({
    ...u,
    pinnedAdmin: !!u.email && admins.includes(u.email.toLowerCase()),
  }))
}

export async function setUserRole(userId: string, role: Role): Promise<void> {
  const sql = await db()
  await sql`update users set role = ${role} where id = ${userId}`
}

/** Grant/revoke the ability to mint LLM-gateway API keys (admins always may). */
export async function setUserCanMintKeys(userId: string, canMint: boolean): Promise<void> {
  const sql = await db()
  await sql`update users set can_mint_keys = ${canMint} where id = ${userId}`
}

/** Replace a user's agent allow-list. Empty = all agents (open by default). */
export async function setUserAgentAccess(userId: string, models: string[]): Promise<void> {
  const sql = await db()
  await sql.begin(async (tx) => {
    await tx`delete from user_agent_access where user_id = ${userId}`
    for (const m of models) {
      await tx`insert into user_agent_access (user_id, agent_model) values (${userId}, ${m}) on conflict do nothing`
    }
  })
}

/** The set of agent models a user may use: 'all' or an explicit allow-list. */
export async function allowedAgents(userId: string, role: Role): Promise<'all' | string[]> {
  if (role === 'admin') return 'all'
  const sql = await db()
  const rows = await sql`select agent_model from user_agent_access where user_id = ${userId}`
  if (rows.length === 0) return 'all'
  return rows.map((r) => r.agent_model as string)
}

export function canUseAgent(access: 'all' | string[], model: string): boolean {
  return access === 'all' || access.includes(model)
}

/** model → owner_user_id for every PERSONAL assistant (owner_user_id set). */
export async function personalAssistantOwners(): Promise<Map<string, string>> {
  const sql = await db()
  const rows = (await sql`
    select model, owner_user_id as "ownerUserId" from agent_defs where owner_user_id is not null
  `) as unknown as Array<{ model: string; ownerUserId: string }>
  return new Map(rows.map((r) => [r.model, r.ownerUserId]))
}

/** Owner-aware USE gate. A personal assistant is usable ONLY by its owner —
 *  otherwise another user could drive it and, through its identity-proxied tools
 *  (Google, memory, the owner's private soul), read the OWNER's account/context.
 *  Non-personal agents fall back to the per-user access list. Returns a predicate
 *  so a listing can filter many models with one pair of queries. */
export async function usableAgentGate(userId: string, role: Role): Promise<(model: string) => boolean> {
  const access = await allowedAgents(userId, role)
  const owners = await personalAssistantOwners()
  return (model: string) => {
    const owner = owners.get(model)
    if (owner && owner !== userId) return false // someone else's personal assistant
    return canUseAgent(access, model)
  }
}

/** Owner-aware single-model check (chat, channels). See usableAgentGate. */
export async function canUseAgentModel(userId: string, role: Role, model: string): Promise<boolean> {
  return (await usableAgentGate(userId, role))(model)
}

/** Views a member may NOT reach. Admins are never restricted (always []). */
export async function deniedViews(userId: string, role: Role): Promise<string[]> {
  if (role === 'admin') return []
  const sql = await db()
  const rows = (await sql`select denied_views as "deniedViews" from users where id = ${userId}`) as unknown as Array<{
    deniedViews: string[]
  }>
  return rows[0]?.deniedViews ?? []
}

export async function setDeniedViews(userId: string, views: string[]): Promise<void> {
  const sql = await db()
  await sql`update users set denied_views = ${views} where id = ${userId}`
}
