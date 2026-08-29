// Users, roles, and per-agent access (durable in Postgres).
//
// Roles: 'admin' | 'member'. The FIRST admin is minted by the first-run claim
// (auth/claim.ts); after that, roles are granted in Admin → People. A sign-in
// never changes anyone's role.
//
// Per-agent access policy: admins → all agents. A member with NO access rows →
// all agents (open by default); with rows → restricted to exactly those. Admins
// manage member allow-lists (UI later).

import { db } from './db/pg'
import { agentCaller, subjectModel, subjectProven, type AgentSubject } from './agent-auth'
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

/** Upsert the identity into `users`. A sign-in assigns 'member' to a
 *  brand-new sub and never touches an existing role — the first admin comes
 *  from the claim (auth/claim.ts), every later one from Admin → People. */
export async function upsertUser(identity: Identity): Promise<User> {
  const sql = await db()
  const rows = await sql`
    insert into users (sub, email, name, picture, role, last_seen_at)
    values (${identity.sub}, ${identity.email}, ${identity.name}, ${identity.picture}, 'member', now())
    on conflict (sub) do update set
      email = excluded.email,
      -- a display name the user set (≠ email) survives logins; the provider
      -- identity only fills the unfriendly defaults (empty, or name = email).
      name = case
        when users.name is null or users.name = '' or users.name = users.email then excluded.name
        else users.name
      end,
      picture = excluded.picture,
      last_seen_at = now()
    returning id, sub, email, name, picture, role
  `
  const user = rows[0] as User
  // Org-wide boards (the workspace Helpdesk) are everyone's by definition, so
  // a sign-in joins this user to any they lack. Dynamic import for the same
  // reason as allManageRoutes below: boards.ts imports from this file, and a
  // static cycle would rather be avoided than argued with. Never fatal — a
  // user who could not be joined still signs in; the next login retries.
  const { joinOrgWideBoards } = await import('./boards')
  await joinOrgWideBoards(user.id).catch((e: unknown) =>
    console.error(`[users] could not join ${user.id} to org-wide boards:`, e),
  )
  return user
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
  const agent = await agentCaller(request)
  if (agent instanceof Response) return null
  if (agent) {
    // Proxying a human — and inheriting their admin role — is the one thing a
    // self-declared name must never buy. Belt and braces: agent-auth already
    // refuses a legacy caller that CLAIMS a personal-assistant name (with a
    // message naming the container to roll), so this is unreachable today and
    // stays as the guarantee for anything that loosens the door.
    if (agent.legacy) return null
    const model = agent.model
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
 *  non-DM channels, implicit editor on non-private KB/artifacts).
 *
 *  Takes the CALLER, not a bare name: elevation is the largest grant an agent
 *  identity carries, so it is never handed to an identity that was merely
 *  asserted (legacy shared-key caller). */
export async function isElevatedAssistant(subject: AgentSubject): Promise<boolean> {
  if (!subjectProven(subject)) return false
  const model = subjectModel(subject)
  const sql = await db()
  const rows = await sql`
    select 1 from agent_defs d join users u on u.id = d.owner_user_id
    where d.model = ${model} and d.elevated and u.role = 'admin'
  `
  return rows.length > 0
}

/** The owner a personal assistant acts for, or null. The identity-proxy reach
 *  this answers — "your assistant manages your boards for you" — is the OWNER'S
 *  OWN view (their memberships, their DMs), not org-wide: that larger grant is
 *  `elevated` on the agent_defs row and stays gated by `isElevatedAssistant`.
 *
 *  Demands the resolved CALLER, never a bare model string. `subjectProven`
 *  reads a string as proven, and owner reach is a human-identity grant — the
 *  same belt-and-braces `actingUser` applies (it checks `legacy` explicitly
 *  even though agent-auth already refuses a legacy caller claiming a
 *  personal-assistant name). */
export async function assistantOwnerId(subject: AgentSubject): Promise<string | null> {
  if (typeof subject === 'string' || subject.legacy) return null
  const sql = await db()
  const rows = (await sql`
    select d.owner_user_id as id from agent_defs d
    where d.model = ${subject.model} and d.owner_user_id is not null
  `) as unknown as Array<{ id: string }>
  return rows[0]?.id ?? null
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

/** The user's platform-default reasoning effort, or null for "the model's own
 *  default everywhere". Stored as a bare level string ('high') rather than
 *  model-scoped: it travels across every model the user talks to, and each
 *  surface applies it only when that model's metadata publishes the level —
 *  so a level the current model does not know is inert, never an error. */
export async function getPreferredEffort(userId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`select preferred_effort as e from users where id = ${userId}`) as unknown as Array<{ e: string | null }>
  return rows[0]?.e ?? null
}

export async function setPreferredEffort(userId: string, effort: string | null): Promise<void> {
  const sql = await db()
  await sql`update users set preferred_effort = ${effort} where id = ${userId}`
}

/** The person's IANA time zone — the zone their brief opens in and their
 *  digest arrives in. Null means "follow the workspace zone", which is the
 *  chain `zoneFor` in daily-brief-config.ts resolves. Validated at the
 *  profile-PUT boundary, trusted (and degraded to UTC by `localMoment`) after. */
export async function getTimezone(userId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`select timezone as tz from users where id = ${userId}`) as unknown as Array<{ tz: string | null }>
  return rows[0]?.tz ?? null
}

export async function setTimezone(userId: string, tz: string | null): Promise<void> {
  const sql = await db()
  await sql`update users set timezone = ${tz} where id = ${userId}`
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
  /** Has a password account (a user_password_credentials row) — the People
   *  list badges these; removing one ends that person's password sign-in. */
  hasPasswordAccount: boolean
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
           exists(select 1 from user_password_credentials c where c.user_id = u.id) as "hasPasswordAccount",
           coalesce(array_agg(a.agent_model) filter (where a.agent_model is not null), '{}') as "agentModels",
           min(d.model) as "assistantModel", coalesce(bool_or(d.elevated), false) as "assistantElevated"
    from users u
    left join user_agent_access a on a.user_id = u.id
    left join agent_defs d on d.owner_user_id = u.id
    group by u.id
    order by lower(coalesce(u.email, u.name, '')) asc
  `
  return rows as unknown as AdminUser[]
}

export async function setUserRole(userId: string, role: Role): Promise<void> {
  const sql = await db()
  await sql`update users set role = ${role} where id = ${userId}`
}

/** Admins currently holding the role — the last-admin guard's input. */
export async function adminCount(): Promise<number> {
  const sql = await db()
  const rows = await sql`select count(*)::int as n from users where role = 'admin'`
  return (rows[0] as { n: number } | undefined)?.n ?? 0
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

/** The human an AGENT CALLER may act for — its owner when it is a personal
 *  assistant, null otherwise. Owner-proxying (their boards, their KB, their
 *  Google account) is escalation, so a legacy caller gets null: identified,
 *  but not proven to BE that assistant.
 *
 *  Use this — not `personalAssistantOwners().get(model)` — on any surface that
 *  has a resolved caller. The map lookup takes a bare string, which throws the
 *  `legacy` flag away silently; this takes the caller and consults it. The map
 *  is for LISTINGS (many models, no caller): channel-replies, mcp-registry. */
export async function assistantOwnerFor(subject: AgentSubject): Promise<string | null> {
  if (!subjectProven(subject)) return null
  return (await personalAssistantOwners()).get(subjectModel(subject)) ?? null
}

/** model → owner_user_id for every PERSONAL assistant (owner_user_id set).
 *  Listing helper — for a per-CALLER decision use `assistantOwnerFor`. */
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
// module stays client-import-free. Enabled apps extend this set dynamically
// with EVERY app view (work and manage) — apps are explicit-grant only, an
// admin adds each one per person.
const MANAGE_VIEW_ROUTES = ['/agents', '/models', '/mcp', '/templates', '/observability', '/apps']

const allManageRoutes = async (): Promise<string[]> => {
  const { appViewRoutes } = await import('./apps')
  return [...MANAGE_VIEW_ROUTES, ...(await appViewRoutes())]
}

/** Views a member may NOT reach: their explicit work-view denials PLUS every
 *  Manage view they haven't been granted. Admins are never restricted. */
export async function deniedViews(userId: string, role: Role): Promise<string[]> {
  if (role === 'admin') return []
  const sql = await db()
  const rows = (await sql`
    select denied_views as "deniedViews", allowed_manage_views as "allowedManageViews" from users where id = ${userId}
  `) as unknown as Array<{ deniedViews: string[]; allowedManageViews: string[] | null }>
  const allowed = new Set(rows[0]?.allowedManageViews ?? [])
  return [...(rows[0]?.deniedViews ?? []), ...(await allManageRoutes()).filter((v) => !allowed.has(v))]
}

/** Replace a member's granted Manage views. */
export async function setAllowedManageViews(userId: string, views: string[]): Promise<void> {
  const sql = await db()
  const valid = await allManageRoutes()
  await sql`update users set allowed_manage_views = ${views.filter((v) => valid.includes(v))} where id = ${userId}`
}

export async function setDeniedViews(userId: string, views: string[]): Promise<void> {
  const sql = await db()
  await sql`update users set denied_views = ${views} where id = ${userId}`
}
