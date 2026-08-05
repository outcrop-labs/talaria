// Shared types + helpers for the MCP page (Mcp.svelte + its cards/modals).
import { createQuery } from '@tanstack/svelte-query'
import { getList } from '@/lib/fetch-json'

export interface McpServerRow {
  id: string
  name: string
  label: string
  description: string | null
  url: string
  headers: Record<string, string>
  timeoutSecs: number | null
  enabled: boolean
  allAgents: boolean
  authMode: 'org' | 'per-user'
  builtin: boolean
  appSlug: string | null
  oauthEnabled: boolean
  /** OAuth org connection state (null for header-auth servers). */
  orgConnected: boolean | null
  oauthMeta: { dcr: boolean; clientSet: boolean; documentation: string | null } | null
  tools: Array<{ name: string; description?: string }>
  toolsRefreshedAt: string | null
  assignments: Array<{ agentModel: string; tools: string[] | null }>
  userAccess: Array<{ userId: string; allowed: boolean; tools: string[] | null }>
}

export const connectPopup = (serverId: string, scope: 'org' | 'me') =>
  window.open(`/api/mcp/oauth/start?server=${serverId}&scope=${scope}`, 'talaria-mcp-oauth', 'width=620,height=780')

export function useMcpServers() {
  return createQuery(() => ({
    queryKey: ['mcp-servers'],
    // "No MCP servers yet" invites an admin to register one they may already
    // have. Only a genuine empty registry earns that screen.
    queryFn: (): Promise<McpServerRow[]> => getList<McpServerRow>('/api/mcp/servers', 'servers'),
  }))
}

export async function patchServer(id: string, body: unknown): Promise<string | null> {
  const r = await fetch(`/api/mcp/servers/${id}`, {
    method: 'PUT',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!r.ok) return ((await r.json().catch(() => ({}))) as { error?: string }).error ?? `failed (${r.status})`
  return null
}

// Scope dropdown sentinels: explicit states, not empty-placeholder magic.
export const ALL_TOOLS = '__all_tools__'
export const NO_ACCESS = '__no_access__'

/** What a scope selection means. Sentinels win over tool picks when NEWLY
 *  added; otherwise the tool subset stands. */
export function resolveScopePick(
  sel: string[],
  prev: { denied: boolean; tools: string[] | null },
): { denied: boolean; tools: string[] | null } {
  const pickedNone = sel.includes(NO_ACCESS) && !prev.denied
  if (pickedNone) return { denied: true, tools: null }
  const pickedAll = sel.includes(ALL_TOOLS) && (prev.denied || prev.tools !== null)
  if (pickedAll) return { denied: false, tools: null }
  const tools = sel.filter((t) => t !== ALL_TOOLS && t !== NO_ACCESS)
  return { denied: false, tools: tools.length ? tools : null }
}

export interface LibraryHeaderRow {
  name: string
  description: string | null
  isRequired: boolean
  isSecret: boolean
  placeholder: string | null
  default: string | null
  choices: string[] | null
}

export interface LibraryServerRow {
  registryName: string
  title: string
  description: string | null
  url: string
  domain: string | null
  icon: string | null
  tier: 'first-party' | 'verified' | 'community'
  requiredHeaders: LibraryHeaderRow[]
}

// Outline chips in the mono chrome voice — gold reserved for the official tier.
export const TIER_BADGE: Record<LibraryServerRow['tier'], { label: string; cls: string; hint: string }> = {
  'first-party': {
    label: 'official',
    cls: 'border-accent/50 text-accent',
    hint: 'Published by the company itself, hosted on its own verified domain',
  },
  verified: { label: 'verified', cls: 'border-line text-muted', hint: 'Domain-verified publisher, hosted elsewhere' },
  community: { label: 'community', cls: 'border-line-subtle text-ink-dim', hint: 'Community-built (io.github namespace)' },
}

// Where to create the app, for providers we recognize. One-time, org-owner.
export const OAUTH_APP_PORTALS: Record<string, string> = {
  'api.githubcopilot.com': 'https://github.com/settings/developers',
}

export const slugify = (v: string) => v.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)
