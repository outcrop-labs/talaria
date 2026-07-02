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

/** Everyone who has signed in — for people pickers (share, invite, channels). */
export async function listUsers(): Promise<Array<{ id: string; email: string | null; name: string | null }>> {
  const sql = await db()
  const rows = await sql`select id, email, name from users order by lower(coalesce(email, name, '')) asc`
  return rows as unknown as Array<{ id: string; email: string | null; name: string | null }>
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
