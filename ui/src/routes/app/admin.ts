// Shared types + query hooks for the Admin console (Admin.svelte + panels).
import { createQuery } from '@tanstack/svelte-query'
import { getJson, getList } from '@/lib/fetch-json'

export type AdminTab = 'org' | 'people' | 'agents' | 'retrieval' | 'storage' | 'secrets' | 'security'
export const ADMIN_TABS: { id: AdminTab; label: string }[] = [
  { id: 'org', label: 'Organization' },
  { id: 'people', label: 'People' },
  { id: 'agents', label: 'Agents' },
  { id: 'retrieval', label: 'Retrieval' },
  { id: 'storage', label: 'Storage' },
  // Its own tab rather than a section under Security: an operator whose
  // secrets are unreadable is not thinking about security policy, and this is
  // the page they get linked to from the banner.
  { id: 'secrets', label: 'Secrets' },
  { id: 'security', label: 'Security' },
]

export interface AdminUser {
  id: string
  email: string | null
  name: string | null
  role: 'admin' | 'member'
  lastSeenAt: string
  createdAt: string
  agentModels: string[]
  canMintKeys: boolean
  deniedViews: string[]
  allowedManageViews: string[]
  /** Has a DB-backed password account — badged in the People list. */
  hasPasswordAccount: boolean
  assistantModel: string | null
  assistantElevated: boolean
}

export function useAdminUsers() {
  return createQuery(() => ({
    // An admin console that renders an EMPTY People list because /api/admin/users
    // 500'd is telling an owner their org has no members. Non-2xx throws.
    queryKey: ['admin-users'],
    queryFn: (): Promise<AdminUser[]> => getList<AdminUser>('/api/admin/users', 'users'),
  }))
}

export interface PermCatalogEntry {
  id: string
  label: string
  hint: string
  group: string
  memberDefault: boolean
}
export interface PermsData {
  catalog: PermCatalogEntry[]
  orgDefaults: Record<string, boolean>
  overrides: Record<string, Record<string, boolean>>
}

export function useAdminPermissions() {
  return createQuery(() => ({
    // Every permission chip on this page is gated on `perms` being truthy, so a
    // failed read used to make the whole permissions model DISAPPEAR — an admin
    // reads that as "nothing is restricted here". Non-2xx throws.
    queryKey: ['admin-permissions'],
    queryFn: (): Promise<PermsData> => getJson<PermsData>('/api/admin/permissions'),
  }))
}

/** The one shape of `/api/admin/settings`, declared ONCE.
 *
 *  Three components read this endpoint under the single `['admin-settings']`
 *  cache key (two here, `ModelsMemberAccessPanel` on the Models page) and each
 *  used to declare its own narrower return type over that one entry —
 *  TypeScript happily believed three different contracts for the same cached
 *  object, so the day one of them started fetching something else nothing
 *  would have complained. One type, one hook. */
/** Rolling-window LLM spend ceilings (server: llm-gateway.ts). Null/0 = off. */
export interface LlmBudgets {
  windowHours: number
  org: { tokens?: number | null; usd?: number | null } | null
  perAgent: { tokens?: number | null; usd?: number | null } | null
  agents: Record<string, { tokens?: number | null; usd?: number | null }>
}
export interface AdminSettings {
  auditRetentionDays: number
  org: { name: string; about: string }
  memberModels: string[]
  llmBudgets: LlmBudgets
  cronMinIntervalMinutes: number
}
export function useAdminSettings() {
  return createQuery(() => ({
    queryKey: ['admin-settings'],
    queryFn: (): Promise<AdminSettings> => getJson<AdminSettings>('/api/admin/settings'),
  }))
}

export const permGroups = (catalog: PermCatalogEntry[]): Array<[string, PermCatalogEntry[]]> => {
  const groups = new Map<string, PermCatalogEntry[]>()
  for (const p of catalog) groups.set(p.group, [...(groups.get(p.group) ?? []), p])
  return [...groups.entries()]
}
