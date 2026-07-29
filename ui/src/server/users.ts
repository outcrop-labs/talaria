// Users, roles, and per-agent access (durable in Postgres).
//
// Roles: 'admin' | 'member'. Admins are designated by AUTH_ADMIN_EMAILS; anyone
// else who signs in (already gated by AUTH_ALLOWED_*) becomes a member.
//
// Per-agent access policy: admins → all agents. A member with NO access rows →
// all agents (open by default); with rows → restricted to exactly those. Admins
// manage member allow-lists (UI later).

import { db } from './db/pg'
import { agentName, checkAgentKey } from './agent-auth'
import { getSessionUser } from './auth/session'

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

export interface ActingUser {
  id: string
  role: Role
  /** For attribution: the human, or "<assistant> (for <human>)". */
  label: string
  viaAssistant: boolean
  /** Admin-elevated assistant (org-wide view/edit; owner must be an admin).
   *  Always false for humans — a human admin's access is unchanged by this. */
  elevated: boolean
}

/** Who a request acts AS: the signed-in human — or, for a PERSONAL assistant
 *  calling with the fleet key, its owner (the identity-proxy model: your
 *  assistant manages your boards for you). General agents resolve to null
 *  here; governance actions stay human(-proxied). */
export async function actingUser(request: Request): Promise<ActingUser | null> {
  if (checkAgentKey(request)) {
    const model = agentName(request)
    if (!model) return null
    const sql = await db()
    const rows = (await sql`
      select u.id, u.role, u.email, u.name, d.elevated from agent_defs d
      join users u on u.id = d.owner_user_id
      where d.model = ${model} and d.owner_user_id is not null
    `) as unknown as Array<{ id: string; role: Role; email: string | null; name: string | null; elevated: boolean }>
    const owner = rows[0]
    if (!owner) return null // not a personal assistant → no proxied identity
    return {
      id: owner.id,
      role: owner.role,
      label: `${model} (for ${owner.email ?? owner.name ?? owner.id})`,
      viaAssistant: true,
      // Elevation only bites while the owner is still an admin — demote the
      // human and the assistant's reach collapses with them.
      elevated: owner.elevated && owner.role === 'admin',
    }
  }
  const user = await getSessionUser(request)
  if (!user) return null
  return { id: user.id, role: user.role, label: user.email ?? user.name ?? 'user', viaAssistant: false, elevated: false }
}

/** True only for a personal assistant an admin explicitly promoted AND whose
 *  owner is currently an admin. Gates org-wide agent access (all boards, all
 *  non-DM channels, implicit editor on non-private KB/artifacts). */
export async function isElevatedAssistant(model: string): Promise<boolean> {
  const sql = await db()
  const rows = await sql`
    select 1 from agent_defs d join users u on u.id = d.owner_user_id
    where d.model = ${model} and d.elevated and u.role = 'admin'
  `
  return rows.length > 0
}

/** Flip org-wide elevation on a user's personal assistant. Returns false if
 *  the user has no assistant. Elevating requires the owner to be an admin. */
export async function setAssistantElevated(userId: string, elevated: boolean): Promise<boolean> {
  const sql = await db()
  const rows = await sql`
    update agent_defs set elevated = ${elevated}
    where owner_user_id = ${userId} returning model
  `
  return rows.length > 0
}

/** A user's role (member when unknown — the restrictive default). */
export async function getUserRole(userId: string): Promise<Role> {
  const sql = await db()
  const rows = (await sql`select role from users where id = ${userId}`) as unknown as Array<{ role: Role }>
  return rows[0]?.role ?? 'member'
}

/** Preferred gateway model for AI drafting (muse); null = server default. */
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
  /** Manage-section views this member HAS been granted (default: none). */
  allowedManageViews: string[]
  /** Email is in AUTH_ADMIN_EMAILS — role is pinned to admin at every login. */
  pinnedAdmin: boolean
  /** The user's personal assistant model, if they have one. */
  assistantModel: string | null
  /** Assistant promoted to org-wide view/edit (only effective while admin). */
  assistantElevated: boolean
}

/** The admin console's user list: roles, activity, and agent allow-lists. */
export async function listUsersAdmin(): Promise<AdminUser[]> {
  const sql = await db()
  const rows = await sql`
    select u.id, u.email, u.name, u.role, u.can_mint_keys as "canMintKeys", u.denied_views as "deniedViews",
           coalesce(u.allowed_manage_views, '{}') as "allowedManageViews",
           u.last_seen_at as "lastSeenAt", u.created_at as "createdAt",
           coalesce(array_agg(a.agent_model) filter (where a.agent_model is not null), '{}') as "agentModels",
           min(d.model) as "assistantModel", coalesce(bool_or(d.elevated), false) as "assistantElevated"
    from users u
    left join user_agent_access a on a.user_id = u.id
    left join agent_defs d on d.owner_user_id = u.id
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

// Manage-section routes: default DENIED for members, granted explicitly via
// allowed_manage_views. Kept here (not imported from lib/nav) so the server
// module stays client-import-free.
const MANAGE_VIEW_ROUTES = ['/agents', '/models', '/mcp', '/templates', '/observability']

/** Views a member may NOT reach: their explicit work-view denials PLUS every
 *  Manage view they haven't been granted. Admins are never restricted. */
export async function deniedViews(userId: string, role: Role): Promise<string[]> {
  if (role === 'admin') return []
  const sql = await db()
  const rows = (await sql`
    select denied_views as "deniedViews", allowed_manage_views as "allowedManageViews" from users where id = ${userId}
  `) as unknown as Array<{ deniedViews: string[]; allowedManageViews: string[] | null }>
  const allowed = new Set(rows[0]?.allowedManageViews ?? [])
  return [...(rows[0]?.deniedViews ?? []), ...MANAGE_VIEW_ROUTES.filter((v) => !allowed.has(v))]
}

/** Replace a member's granted Manage views. */
export async function setAllowedManageViews(userId: string, views: string[]): Promise<void> {
  const sql = await db()
  await sql`update users set allowed_manage_views = ${views.filter((v) => MANAGE_VIEW_ROUTES.includes(v))} where id = ${userId}`
}

export async function setDeniedViews(userId: string, views: string[]): Promise<void> {
  const sql = await db()
  await sql`update users set denied_views = ${views} where id = ${userId}`
}
