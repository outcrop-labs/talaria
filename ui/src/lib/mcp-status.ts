// What one registered MCP server's card should SAY about itself, derived
// from the row — one function, so the card, the agent tab and the picker
// agree on what "needs a key" or "connected" means. In lib/ (not routes/) so
// vitest can pin the matrix.

import { isInternalServer, type ServerOrigin } from './mcp-servers'

export interface ProbeRow {
  /** `reconnect` is the health pass's: the org OAuth account WAS connected
   *  and the provider has since refused to refresh it. */
  state: 'ok' | 'auth-required' | 'unreachable' | 'error' | 'reconnect'
  detail: string
  at: string
  /** Health pass only: assignments whose tool subset names a tool the
   *  catalog no longer offers. */
  staleAssignments?: Array<{ agent: string; missing: string[] }>
  /** Health pass only (per-user OAuth servers): connected accounts whose
   *  refresh the provider rejected. */
  reconnectAccounts?: number
}

/** The fields the verdict reads — structural, so lib/ imports nothing from
 *  routes/. */
export interface StatusInput {
  enabled: boolean
  builtin: boolean
  appSlug: string | null
  package: unknown | null
  pkgStatus: string | null
  authMode: 'org' | 'per-user'
  oauthEnabled: boolean
  orgConnected: boolean | null
  oauthMeta: { dcr: boolean; clientSet: boolean; dcrRejected: string | null } | null
  probe: ProbeRow | null
  tools: Array<{ name: string }>
}

export type StatusKind =
  | 'disabled'
  | 'pulling'
  | 'image-failed'
  | 'needs-oauth-app'
  | 'reconnect'
  | 'needs-connect'
  | 'needs-key'
  | 'per-user'
  | 'unreachable'
  | 'error'
  | 'connected'
  | 'ready'
  | 'untested'

export interface ServerStatus {
  kind: StatusKind
  label: string
  tone: 'muted' | 'warning' | 'danger' | 'success'
  /** The sentence behind the chip — the probe's own words when it has them. */
  detail: string | null
  /** The one action the chip implies, if any. */
  action: 'connect' | 'setup-oauth-app' | 'set-credentials' | null
}

const chip = (
  kind: StatusKind,
  label: string,
  tone: ServerStatus['tone'],
  detail: string | null = null,
  action: ServerStatus['action'] = null,
): ServerStatus => ({ kind, label, tone, detail, action })

/** The verdict, most blocking first: lifecycle, then the auth it cannot do
 *  without, then what the last discovery found, then the good news. */
export function serverStatus(s: StatusInput): ServerStatus {
  if (!s.enabled) return chip('disabled', 'disabled', 'muted')
  if (s.pkgStatus === 'pulling') return chip('pulling', 'pulling image', 'muted')
  if (s.pkgStatus === 'error') return chip('image-failed', 'image failed', 'danger', s.probe?.detail ?? null)
  const internal = s.builtin || !!s.appSlug
  if (s.oauthEnabled && s.authMode === 'org' && !s.orgConnected) {
    // Connected once, refused since: the same Connect button, but the chip
    // says the grant was lost — a first-time "connect" would read as if
    // nobody had ever set it up.
    if (s.probe?.state === 'reconnect') {
      return chip('reconnect', 'reconnect org account', 'danger', s.probe.detail, 'connect')
    }
    const manual = !!s.oauthMeta && !s.oauthMeta.clientSet && (!s.oauthMeta.dcr || !!s.oauthMeta.dcrRejected)
    return manual
      ? chip('needs-oauth-app', 'needs OAuth app', 'warning', 'Register an OAuth app with the provider, then connect.', 'setup-oauth-app')
      : chip('needs-connect', 'connect org account', 'warning', 'This server uses OAuth. Connect the org account so agents can use it.', 'connect')
  }
  if (s.authMode === 'per-user') {
    // The health pass proves a per-user host answers (a 401 counts); when it
    // does not, that outranks "each person connects" — nobody can.
    if (s.probe?.state === 'unreachable') return chip('unreachable', 'unreachable', 'danger', s.probe.detail)
    if (s.probe?.state === 'error') return chip('error', 'error', 'danger', s.probe.detail)
    return chip(
      'per-user',
      'per-user accounts',
      'muted',
      'Each person connects their own account under Settings → Connections; until they do, their assistant is told where to connect.',
    )
  }
  if (s.probe?.state === 'auth-required' && !internal) {
    return chip('needs-key', 'needs credentials', 'warning', s.probe.detail, 'set-credentials')
  }
  if (s.probe?.state === 'unreachable') return chip('unreachable', 'unreachable', 'danger', s.probe.detail)
  if (s.probe?.state === 'error') return chip('error', 'error', 'danger', s.probe.detail)
  if (s.oauthEnabled && s.orgConnected) return chip('connected', 'connected', 'success', s.probe?.detail ?? null)
  if (s.probe?.state === 'ok' || s.tools.length > 0) {
    return chip('ready', 'ready', 'success', s.probe?.detail ?? null)
  }
  if (internal || s.package) return chip('ready', 'ready', 'success')
  return chip('untested', 'not yet discovered', 'muted', 'Ask the server for its tools to prove the connection.')
}

/** Whether the hourly health pass asks this server — what the card's
 *  "Checked …" line and Check now promise. The deployment's own servers
 *  (builtin, app, the Workbench) are not asked, nor a stdio package: it has
 *  no address, and its child spawns on first use. An oci-http package is,
 *  at its container. The api's rule (`health::in_scope`) too. */
export function healthChecked(s: ServerOrigin & { package: { transport: string } | null }): boolean {
  if (isInternalServer(s)) return false
  return !s.package || s.package.transport === 'http'
}

/** "Checked 5 minutes ago" — when the server was last asked (by the health
 *  pass or a person), from the probe's own timestamp; null = never asked. */
export function checkedLine(probe: ProbeRow | null, relative: (iso: string) => string): string | null {
  return probe?.at ? `Checked ${relative(probe.at)}` : null
}

/** The subset entries a server's catalog no longer offers, in the subset's
 *  order. An empty catalog proves nothing (never discovered, or the last
 *  discovery failed), so it reports nothing missing — the api's rule too. */
export function missingTools(subset: string[] | null, tools: Array<{ name: string }>): string[] {
  if (!subset || tools.length === 0) return []
  const offered = new Set(tools.map((t) => t.name))
  return [...new Set(subset.filter((t) => !offered.has(t)))]
}

/** Every assignment on a server whose subset lost a tool — the card's
 *  "N agents name tools this server no longer offers". */
export function staleAssignments(
  assignments: Array<{ agentModel: string; tools: string[] | null }>,
  tools: Array<{ name: string }>,
): Array<{ agentModel: string; missing: string[] }> {
  return assignments
    .map((a) => ({ agentModel: a.agentModel, missing: missingTools(a.tools, tools) }))
    .filter((a) => a.missing.length > 0)
}

/** "Drop missing tools": the subset without the tools the server no longer
 *  offers. Null when nothing would remain — the caller unassigns instead,
 *  because an empty subset grants nothing and `null` would grant EVERY tool. */
export function withoutMissing(subset: string[], missing: string[]): string[] | null {
  const gone = new Set(missing)
  const kept = subset.filter((t) => !gone.has(t))
  return kept.length ? kept : null
}

export interface CatalogStatus {
  count: number
  syncedAt: number | null
  syncing: boolean
  phase: 'empty' | 'syncing' | 'ready'
  error: string | null
}

/** The picker's one status line about the catalog. */
export function catalogLine(c: CatalogStatus | undefined, relative: (iso: string) => string): string {
  if (!c) return ''
  if (c.phase === 'empty') return c.syncing ? 'Catalog syncing — first results land in a moment' : 'Catalog not synced yet'
  const n = c.count.toLocaleString()
  if (c.phase === 'syncing') return `Catalog syncing — ${n} servers so far`
  const when = c.syncedAt ? ` · synced ${relative(new Date(c.syncedAt).toISOString())}` : ''
  const err = c.error ? ' · last sync failed' : ''
  return `${n} servers${when}${err}`
}

/** What the "no results" state should say — the difference between "nothing
 *  matches" and "nothing has been synced yet" is the whole message. */
export function emptyHint(c: CatalogStatus | undefined, q: string): { title: string; hint: string } {
  if (c && c.phase === 'empty') {
    return {
      title: c.syncing ? 'Catalog syncing' : 'Catalog not synced yet',
      hint: c.syncing
        ? 'The official registry is being mirrored; search answers as soon as the first pages land.'
        : 'Sync the catalog to search the official registry, or register a custom endpoint.',
    }
  }
  if (c && c.phase === 'syncing') {
    return {
      title: 'No match yet',
      hint: 'The catalog is still filling. Try again in a moment, or register a custom endpoint.',
    }
  }
  return {
    title: q.trim() ? `No servers match “${q.trim()}”` : 'No servers',
    hint: 'Hosted endpoints and runnable packages (npm/pypi/docker) both install from here. Try the publisher’s name, or the custom form for self-hosted servers.',
  }
}
