// Shared types + helpers for the MCP page (Mcp.svelte + its cards/modals) and
// the agent settings MCP tab, which browses and assigns the same registry.
import { createQuery } from '@tanstack/svelte-query'
import { errorMessage, getJson, getList, postJson, putJson } from '@/lib/fetch-json'
export { isInternalServer } from '@/lib/mcp-servers'
export { catalogLine, emptyHint, serverStatus, type CatalogStatus, type ProbeRow, type ServerStatus } from '@/lib/mcp-status'
// The teams directory is one read app-wide (`@/lib/teams`); the MCP access
// dialog reaches it through this module like the page's other shared helpers.
export { useTeamsDirectory, type TeamDirectoryEntry } from '@/lib/teams'
export {
  composeHeader,
  composeHeaders,
  headerFields,
  literalHeaders,
  type HeaderDecl,
  type HeaderField,
} from '@/lib/mcp-headers'
import type { CatalogStatus, ProbeRow } from '@/lib/mcp-status'
import type { HeaderDecl } from '@/lib/mcp-headers'

/** A marketplace package install (npm/pypi/oci): how the runtime container
 *  is built. Declarations only — env values are sealed server-side. */
export interface LibraryPackageRow {
  kind: 'npm' | 'pypi' | 'oci'
  identifier: string
  version: string | null
  runtimeHint: string | null
  transport: 'stdio' | 'http'
  containerPort: number | null
  transportPath: string | null
  image: string
  runArgs: Array<{ type?: string; name?: string; value?: string; default?: string | null; placeholder?: string | null; description?: string | null; isRequired?: boolean }>
  declaredEnv: LibraryHeaderRow[]
}

/** Where the OAuth app gets created, for providers the api has a row for. */
export interface OauthGuide {
  domain: string
  portal: string
  appName: string
  notes: string[]
}

export interface OauthMeta {
  dcr: boolean
  clientSet: boolean
  documentation: string | null
  /** The upstream's error code when it refused our registration (Vercel:
   *  "invalid_redirect_uri"); null when never refused or since cleared. */
  dcrRejected: string | null
  /** THE callback to register with the provider — computed by the api the
   *  same way the authorize request names it. */
  callbackUrl: string
  scopes: string[]
  scopesOverride: string[] | null
  guide: OauthGuide | null
}

export interface McpServerRow {
  id: string
  name: string
  label: string
  description: string | null
  url: string
  /** Keys only in practice — GET masks every value as •••. */
  headers: Record<string, string>
  timeoutSecs: number | null
  enabled: boolean
  allAgents: boolean
  authMode: 'org' | 'per-user'
  builtin: boolean
  appSlug: string | null
  oauthEnabled: boolean
  /** Header declarations captured at install (drive the credential forms). */
  requiredHeaders: HeaderDecl[]
  /** Package installs: the stored declarations doc (never env values). */
  package: LibraryPackageRow | null
  /** Package runtime state: pulling | error | ready | idle | stopped. */
  pkgStatus: string | null
  /** OAuth org connection state (null for header-auth servers). */
  orgConnected: boolean | null
  oauthMeta: OauthMeta | null
  /** What the last discovery said; null = never probed. */
  probe: ProbeRow | null
  tools: Array<{ name: string; description?: string }>
  toolsRefreshedAt: string | null
  assignments: Array<{ agentModel: string; tools: string[] | null }>
  userAccess: Array<{ userId: string; allowed: boolean; tools: string[] | null }>
  teamAccess: Array<{ teamId: string; allowed: boolean; tools: string[] | null }>
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
  try {
    await putJson<{ ok: true }>(`/api/mcp/servers/${id}`, body)
    return null
  } catch (e) {
    return errorMessage(e)
  }
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
  /** The registry's template ("Bearer {api_key}") — null when the whole
   *  header is user-supplied; a brace-free string is a fixed header. */
  value: string | null
  variables: Record<
    string,
    { description: string | null; isSecret: boolean; placeholder: string | null; default: string | null; choices: string[] | null }
  > | null
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
  /** Package-shipping entries (no hosted endpoint): how to run them. */
  package: LibraryPackageRow | null
}

export interface LibraryResponse {
  servers: LibraryServerRow[]
  catalog: CatalogStatus
}

export type TierFilter = 'all' | 'official' | 'verified' | 'community'
export type KindFilter = 'all' | 'hosted' | 'package'

/** The catalog read: the featured shelf for an empty query with no filters,
 *  a search otherwise. Both arms carry the catalog status. */
export function useMcpLibrary(params: () => { q: string; tier: TierFilter; kind: KindFilter }) {
  return createQuery(() => {
    const { q, tier, kind } = params()
    const query = q.trim()
    const sp = new URLSearchParams()
    if (!query && tier === 'all' && kind === 'all') sp.set('featured', '1')
    else {
      if (query) sp.set('q', query)
      if (tier !== 'all') sp.set('tier', tier)
      if (kind !== 'all') sp.set('kind', kind)
    }
    return {
      queryKey: ['mcp-library', query, tier, kind],
      queryFn: (): Promise<LibraryResponse> => getJson<LibraryResponse>(`/api/mcp/library?${sp.toString()}`),
      placeholderData: (prev: LibraryResponse | undefined) => prev,
      staleTime: 60_000,
    }
  })
}

export function useMcpCatalogStatus() {
  return createQuery(() => ({
    queryKey: ['mcp-library-status'],
    queryFn: async (): Promise<CatalogStatus> => (await getJson<LibraryResponse>('/api/mcp/library?featured=1')).catalog,
    staleTime: 15_000,
  }))
}

export const syncCatalog = () => postJson<{ ok: true; catalog: CatalogStatus }>('/api/mcp/library/sync')

export interface InstallOpts {
  headers?: Record<string, string>
  authMode?: 'org' | 'per-user'
  env?: Record<string, string>
  argValues?: Record<string, string>
}

export interface InstallOutcome {
  serverId: string
  oauthMeta: OauthMeta | null
  /** OAuth without self-registration: the admin must create an app first. */
  needsOauthApp: boolean
  /** OAuth that self-registers: the org account can be connected right away. */
  needsConnect: boolean
  /** Discovery's refusal, verbatim — a 401 here is "needs credentials". */
  discoveryError: string | null
  needsCredentials: boolean
}

export const needsOauthApp = (m: OauthMeta | null) => !!m && !m.clientSet && (!m.dcr || !!m.dcrRejected)

/** Register a marketplace entry in the org registry and run discovery, so
 *  the caller learns in one step whether the server is usable, needs a key,
 *  or needs an OAuth connection. The POST's own failure (a taken name, a
 *  refused package flag) throws; discovery's does not. */
export async function installLibraryServer(l: LibraryServerRow, opts: InstallOpts): Promise<InstallOutcome> {
  const { server } = await postJson<{ server: { id: string; oauthMeta: OauthMeta | null } }>('/api/mcp/servers', {
    name: slugify(l.title),
    label: l.title,
    url: l.package ? undefined : l.url,
    description: l.description,
    headers: opts.headers,
    authMode: l.package ? 'org' : opts.authMode,
    requiredHeaders: l.requiredHeaders,
    package: l.package ?? undefined,
    env: l.package ? opts.env : undefined,
    argValues: l.package ? opts.argValues : undefined,
  })
  const meta = server.oauthMeta
  const oauthApp = needsOauthApp(meta)
  const connect = !!meta && !oauthApp
  // An OAuth server cannot be asked for its tools before it is connected;
  // discovery would only answer the 401 the card already explains.
  const discoveryError = connect || oauthApp ? null : await patchServer(server.id, { refreshTools: true })
  return {
    serverId: server.id,
    oauthMeta: meta,
    needsOauthApp: oauthApp,
    needsConnect: connect,
    discoveryError,
    needsCredentials: !!discoveryError && /requires authentication/i.test(discoveryError) && !meta,
  }
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

export const slugify = (v: string) => v.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)

/** A free-form header row in the credential forms. */
export interface HeaderRow {
  name: string
  value: string
}

export const emptyHeaderRow = (): HeaderRow => ({ name: '', value: '' })

/** Rows → the header map the api stores: named rows with a value, trimmed
 *  names, last duplicate wins. */
export function headersOfRows(rows: HeaderRow[]): Record<string, string> {
  const out: Record<string, string> = {}
  for (const r of rows) {
    const name = r.name.trim()
    if (name && r.value.trim()) out[name] = r.value
  }
  return out
}

/** "a, b c" → ["a", "b", "c"]; null when nothing is left — the scopes field. */
export function parseScopes(input: string): string[] | null {
  const scopes = input
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter(Boolean)
  return scopes.length ? Array.from(new Set(scopes)) : null
}
