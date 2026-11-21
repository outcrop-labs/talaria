// Agent policy › GitHub access, as pure data: one word per agent for the
// overview, the requests that wait, and the permission list of one agent's
// editor. PURE, so the node-env tests pin it.
//
// One answer everywhere. The overview reads each Developer agent's own
// `GET /api/workbench/github-permissions/{id}` (the editor's query, the same
// cache entry), and `accessStatus` is built on `keyState`, the function
// behind the editor's rows. So the overview and the editor cannot disagree.
//
// What the installation holds is the most an admin may grant an agent; it is
// never what a token carries, and nothing here calls an agent "ok" from it. A
// key the record says is held reads Not checked until a token for this set
// says otherwise. Nothing here gates anything: the words say what is so.

import {
  addedKeys,
  heldState,
  holds,
  impactNote,
  keyState,
  mintingInstallations,
  offTokenWhy,
  scopeOf,
  since,
  type AddedKey,
  type AgentGithubPermissions,
  type CatalogEntry,
  type HostPermissions,
  type KeyState,
  type LastToken,
  type PermissionScope,
  type PermissionsInstallation,
} from './agent-github-permissions'
import { askedLabel, heldLabel, isDecidable, type PermissionRequest } from './git-permission-requests'
import { unclaimedKeys, unclaimedSentence, type HostAgents } from './git-host-agents'
import type { PermissionLevel } from './git-permissions'

export { addedKeys, addedOf } from './agent-github-permissions'

/** A dot's tone, the subset of `DotStatus` these words use. */
export type AccessTone = 'ok' | 'warn' | 'idle'

/** The editor's (and the overview's) query key for one agent's permissions. */
export const githubPermissionsKey = (agentId: string) => ['workbench-github-permissions', agentId]

/** The page's own address, and an agent's editor on it. */
export const GITHUB_ACCESS_HREF = '/admin/github'
export const githubAccessHref = (agentId?: string | null): string =>
  agentId ? `${GITHUB_ACCESS_HREF}?agent=${encodeURIComponent(agentId)}` : GITHUB_ACCESS_HREF

// ── One word per agent ──────────────────────────────────────────────────────

export type AccessKind =
  | 'developerOff'
  | 'retired'
  | 'noRepos'
  | 'refused'
  | 'missing'
  | 'notChecked'
  | 'workspaceSecret'
  | 'carries'
  | 'hostToken'

/** The word for each state, the same on every page. Short on purpose: the
 *  status column does not wrap, and the keys go in the row's hint. */
export const ACCESS_WORD: Record<AccessKind, string> = {
  developerOff: 'Developer off',
  retired: 'Retired',
  noRepos: 'No repositories',
  refused: 'Refused',
  missing: 'Missing permissions',
  notChecked: 'Not checked',
  workspaceSecret: 'Workspace secret',
  carries: 'Carries its set',
  hostToken: 'Host token',
}

const ACCESS_TONE: Record<AccessKind, AccessTone> = {
  developerOff: 'idle',
  retired: 'idle',
  noRepos: 'idle',
  refused: 'warn',
  missing: 'warn',
  notChecked: 'idle',
  workspaceSecret: 'idle',
  carries: 'ok',
  hostToken: 'idle',
}

export interface AccessStatus {
  kind: AccessKind
  word: string
  tone: AccessTone
  /** For Missing permissions: `key: level` of each key of the set that is
   *  not on the agent's token (or that the installation does not hold), in
   *  host then catalog order. Empty otherwise. */
  missing: string[]
}

const status = (kind: AccessKind, missing: string[] = []): AccessStatus => ({ kind, word: ACCESS_WORD[kind], tone: ACCESS_TONE[kind], missing })

/** The App's hosts, worst first: one host's word beats another's in this
 *  order. */
const HOST_ORDER: AccessKind[] = ['refused', 'missing', 'notChecked', 'workspaceSecret', 'carries']

/** A repo's last token that speaks for the agent's set now: the App's,
 *  minted for this set, or a refusal no set is said to have outlived (the
 *  same reading `keyState` makes). */
const speaksForSet = (t: LastToken | null | undefined): t is LastToken =>
  t != null && (t.source ?? 'git_host') === 'git_host' && (t.current === true || (!t.ok && t.current == null))

/** One key's word from its `keyState`: whether it is on the token, off it,
 *  or not known yet. The record's `held` is "not checked", never "on". */
function keyBucket(state: KeyState): 'off' | 'unchecked' | 'on' {
  if (state.kind === 'onToken') return 'on'
  if (state.kind === 'offToken') return 'off'
  if (state.kind === 'notYet') return 'unchecked'
  return state.held.kind === 'held' || state.held.kind === 'unknown' ? 'unchecked' : 'off'
}

/** One App host's word, and the keys it finds off the token. */
export function hostAccess(block: HostPermissions, catalog: CatalogEntry[]): { kind: AccessKind; missing: string[] } {
  // A workspace secret answers git here ahead of the App: whatever earlier
  // tokens said, none of these permissions apply to the next push.
  if (block.workspaceSecret?.trim()) return { kind: 'workspaceSecret', missing: [] }
  if (block.repos.some((r) => speaksForSet(r.lastToken) && !r.lastToken.ok)) return { kind: 'refused', missing: [] }
  const missing: string[] = []
  let unchecked = false
  for (const [key, level] of orderedSet(block, catalog)) {
    const bucket = keyBucket(keyState(block, key, level, scopeOf(catalog, key)))
    if (bucket === 'off') missing.push(`${key}: ${level}`)
    else if (bucket === 'unchecked') unchecked = true
  }
  if (missing.length > 0) return { kind: 'missing', missing }
  return { kind: unchecked ? 'notChecked' : 'carries', missing: [] }
}

/** The set's keys in the catalog's order (a key it does not know after). */
function orderedSet(block: HostPermissions, catalog: CatalogEntry[]): Array<[string, PermissionLevel]> {
  const at = (key: string) => {
    const i = catalog.findIndex((e) => e.key === key)
    return i === -1 ? catalog.length : i
  }
  return Object.entries(block.permissions ?? {}).sort(([a], [b]) => at(a) - at(b) || a.localeCompare(b))
}

/** Whether the host's App narrows tokens per repo, and the agent holds a repo
 *  there: the only hosts where a token is minted for its set. */
const appHostWithRepos = (h: HostPermissions): boolean => h.narrowing === 'perRepo' && h.repos.length > 0

/** One word for one agent, the same on the overview and in its editor:
 *  the Developer switch first, then whether it is retired, whether it holds
 *  any repo, and then the worst word of the App hosts it works on. An agent
 *  that works only where the host's own token decides reads Host token. */
export function accessStatus(def: { developer: boolean; enabled: boolean }, perms: Pick<AgentGithubPermissions, 'hosts' | 'catalog'>): AccessStatus {
  if (!def.developer) return status('developerOff')
  if (!def.enabled) return status('retired')
  const withRepos = perms.hosts.filter((h) => h.repos.length > 0)
  if (withRepos.length === 0) return status('noRepos')
  const app = withRepos.filter(appHostWithRepos)
  if (app.length === 0) return status('hostToken')
  const words = app.map((h) => hostAccess(h, perms.catalog))
  const worst = HOST_ORDER.find((k) => words.some((w) => w.kind === k)) ?? 'notChecked'
  return status(worst, worst === 'missing' ? words.flatMap((w) => w.missing) : [])
}

// ── The overview ────────────────────────────────────────────────────────────

/** What the overview knows of one agent def. */
export interface OverviewDef {
  id: string
  displayName: string
  model?: string
  developer: boolean
  enabled: boolean
}

/** One agent's permissions read: loading, read, or failed. */
export interface PermsRead {
  data?: AgentGithubPermissions | null
  error?: string | null
}

/** One host's agents read (`gitHostAgentsKey`): null for a server without the
 *  route (404), undefined while it loads. */
export interface HostAgentsRead {
  id: string
  name: string
  kind: string
  body: HostAgents | null | undefined
}

/** A row whose permissions read failed: it still opens, and says so. */
export const COULD_NOT_READ = 'Could not read'

export interface OverviewRow {
  id: string
  name: string
  href: string
  /** The row's hint: its permissions and repositories. */
  hint: string
  /** The keys not on its token, for a second clause of the hint. */
  missing: string[]
  /** Null while its read is in flight, or when it failed (`error`). */
  status: AccessStatus | null
  error: string | null
  /** Requests from it waiting on an admin. */
  waiting: number
}

export interface UnclaimedNotice {
  hostId: string
  hostName: string
  keys: string[]
  sentence: string
}

export interface Overview {
  /** Agents with the Developer switch on, live then retired, by name. */
  developer: OverviewRow[]
  /** Agents without it, by name. No permissions read is made for them. */
  others: OverviewRow[]
  /** Per GitHub host, what its installation accepted that no agent's set
   *  names. */
  unclaimed: UnclaimedNotice[]
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

/** The repos an agent holds, by the agents route: its rows on every host. */
function routeRepoCount(id: string, hosts: HostAgentsRead[]): number | null {
  const read = hosts.filter((h) => h.body != null)
  if (read.length === 0) return null
  return read.reduce((n, h) => n + (h.body?.agents.find((a) => a.agentId === id)?.repos?.length ?? 0), 0)
}

/** "Custom permissions · 2 repositories, on 2 hosts", from its own read. */
export function permsHint(perms: Pick<AgentGithubPermissions, 'hosts'>): string {
  const withRepos = perms.hosts.filter((h) => h.repos.length > 0)
  const repos = withRepos.reduce((n, h) => n + h.repos.length, 0)
  if (repos === 0) return 'No repositories'
  const app = perms.hosts.filter((h) => h.narrowing === 'perRepo')
  const custom = app.some((h) => h.isDefault === false)
  const where = withRepos.length > 1 ? `, on ${withRepos.length} hosts` : ''
  const which = app.length === 0 ? '' : `${custom ? 'Custom' : 'Default'} permissions · `
  return `${which}${plural(repos, 'repository', 'repositories')}${where}`
}

/** The overview's rows: the Developer agents (each from its own permissions
 *  read), the rest from the agents route's repo counts, and the unclaimed
 *  notices, one per GitHub host that has any. */
export function overviewRows(input: {
  defs: OverviewDef[]
  perms: Record<string, PermsRead | undefined>
  hosts: HostAgentsRead[]
  requests: PermissionRequest[]
}): Overview {
  const waiting = (id: string) => input.requests.filter((r) => isDecidable(r) && r.agentId === id && r.status === 'pending').length
  const byName = (a: OverviewRow, b: OverviewRow) => a.name.localeCompare(b.name)
  const counted = (id: string): string => {
    const n = routeRepoCount(id, input.hosts)
    return n == null ? '' : n === 0 ? 'No repositories' : plural(n, 'repository', 'repositories')
  }
  const base = (d: OverviewDef) => ({ id: d.id, name: d.displayName || d.id, href: githubAccessHref(d.id), waiting: waiting(d.id) })

  const live: OverviewRow[] = []
  const retired: OverviewRow[] = []
  const others: OverviewRow[] = []
  for (const d of input.defs) {
    if (!d.developer) {
      others.push({ ...base(d), hint: counted(d.id), missing: [], status: accessStatus(d, { hosts: [], catalog: [] }), error: null })
      continue
    }
    if (!d.enabled) {
      retired.push({ ...base(d), hint: counted(d.id), missing: [], status: accessStatus(d, { hosts: [], catalog: [] }), error: null })
      continue
    }
    const read = input.perms[d.id]
    if (read?.data) {
      const s = accessStatus(d, read.data)
      live.push({ ...base(d), hint: permsHint(read.data), missing: s.missing, status: s, error: null })
    } else {
      live.push({ ...base(d), hint: counted(d.id), missing: [], status: null, error: read?.error ?? null })
    }
  }

  const unclaimed = input.hosts
    .filter((h) => h.kind === 'github')
    .flatMap((h): UnclaimedNotice[] => {
      const keys = unclaimedKeys(h.body)
      const sentence = unclaimedSentence(keys, h.name)
      return sentence ? [{ hostId: h.id, hostName: h.name, keys, sentence }] : []
    })

  return { developer: [...live.sort(byName), ...retired.sort(byName)], others: others.sort(byName), unclaimed }
}

// ── Waiting on you ──────────────────────────────────────────────────────────

/** One open repository request, as the overview counts them. */
export interface RepoRequestLike {
  id: string
  agentId?: string | null
  org: string
  name: string
}

export type RequestRow =
  | {
      kind: 'decide'
      id: string
      agentId: string
      agentName: string
      /** "Builder asks for workflows write". */
      label: string
      /** Where, the ticket, and what the installation holds, as text. */
      hint: string
      word: string
      tone: AccessTone
      href: string
    }
  | { kind: 'fact'; id: string; text: string }

/** The words a fact-only request carries: that it waits, and why it says no
 *  more. */
export const factSentence = (hostName: string): string =>
  `A GitHub permission request waits on ${hostName}. It was raised on a board you are not a member of; an admin who is a member decides it.`

/** A request's state, in the words every page uses. */
export const requestWord = (r: { status: string }): { word: string; tone: AccessTone } =>
  r.status === 'awaiting_github' ? { word: 'Waiting on GitHub', tone: 'idle' } : { word: 'Waiting on you', tone: 'warn' }

/** The overview's "Waiting on you": one compact row per request, which opens
 *  the agent's editor to decide it there, and a line that counts the new
 *  repository requests the listed agents have open. */
export function requestRows(
  requests: PermissionRequest[],
  repoRequests: RepoRequestLike[],
  defs: Array<Pick<OverviewDef, 'id'>>,
): { rows: RequestRow[]; repoLine: string | null } {
  const rows = requests.map((r): RequestRow => {
    if (!isDecidable(r)) return { kind: 'fact', id: r.id, text: factSentence(r.hostName) }
    const ticket = r.ticket?.ref ?? null
    const hint = [`on ${r.hostName}, where it worked on ${r.repo}`, ticket, heldLabel(r.held)].filter(Boolean).join(' · ')
    return {
      kind: 'decide',
      id: r.id,
      agentId: r.agentId,
      agentName: r.agentName,
      label: `${r.agentName} asks for ${askedLabel(r)}`,
      hint,
      ...requestWord(r),
      href: githubAccessHref(r.agentId),
    }
  })
  const ids = new Set(defs.map((d) => d.id))
  const repos = repoRequests.filter((r) => r.agentId && ids.has(r.agentId)).length
  const repoLine =
    repos === 0 ? null : repos === 1 ? '1 new-repository request waits in Git hosts' : `${repos} new-repository requests wait in Git hosts`
  return { rows, repoLine }
}

// ── The editor's permission list ────────────────────────────────────────────

export type LevelChoice = 'none' | PermissionLevel

export interface LevelOption {
  id: LevelChoice
  label: string
  disabled: boolean
  title?: string
}

export interface MatrixRow {
  key: string
  /** "Pull requests". */
  label: string
  scope: PermissionScope
  /** Its level in the set, or null when the set does not name it. */
  level: PermissionLevel | null
  /** None, then each level GitHub takes it at. Metadata has no None. */
  options: LevelOption[]
  /** What the installation holds, never in the success tone. */
  held: { word: string; tone: 'warn' | 'neutral'; link?: { href: string; label: string } }
  /** What the agent's token carried. `check`: offer Check token beside it. */
  token: { word: string; tone: AccessTone; title?: string; check?: boolean }
  /** What a high-impact key allows, for the row's tip. */
  impact: string | null
  added: AddedKey | null
  /** The agent has an open request for it on this host. */
  requested: boolean
}

export type MatrixView = 'set' | 'all'

/** "pull_requests" → "Pull requests". */
export const keyLabel = (key: string): string => {
  const s = key.replaceAll('_', ' ')
  return s.charAt(0).toUpperCase() + s.slice(1)
}

const LEVEL_WORD: Record<LevelChoice, string> = { none: 'None', read: 'Read', write: 'Write', admin: 'Admin' }

/** The level every minting installation holds `key` at, or null when one
 *  holds none of it. Undefined: none is on record. */
function heldLevel(block: HostPermissions, key: string): PermissionLevel | null | undefined {
  const read = mintingInstallations(block).filter((i) => i.held != null)
  if (read.length === 0) return undefined
  const highestFirst: PermissionLevel[] = ['admin', 'write', 'read']
  return highestFirst.find((l) => read.every((i) => holds(i.held, key, l))) ?? null
}

const accounts = (installations: PermissionsInstallation[]): string =>
  installations.map((i) => i.account || `installation ${i.id}`).join(', ') || 'the account'

/** The installation's word for a key: what it holds, or why not. */
function heldWord(block: HostPermissions, key: string, level: PermissionLevel | null, levels: PermissionLevel[]): MatrixRow['held'] {
  const at = heldLevel(block, key)
  if (at === undefined) return { word: 'What the installation holds is not on record', tone: 'neutral' }
  const inSet = level != null
  if (at != null && (!inSet || holds({ [key]: at }, key, level))) return { word: `Installation holds ${at}`, tone: 'neutral' }
  if (at != null && inSet) return { word: `Installation holds ${at} only`, tone: 'warn' }
  const state = heldState(block, key, level ?? levels[0] ?? 'read')
  const tone = inSet ? 'warn' : 'neutral'
  if (state.kind === 'appMissing') return { word: "The GitHub App doesn't ask for it", tone }
  if (state.kind === 'awaitingApproval') {
    const url = state.installations.find((i) => i.url)?.url
    return {
      word: `Not accepted on GitHub by ${accounts(state.installations)}.`,
      tone,
      ...(url ? { link: { href: url, label: 'Accept it on GitHub ↗' } } : {}),
    }
  }
  if (state.kind === 'notHeld') return { word: `The installation on ${accounts(state.installations)} doesn't hold it`, tone }
  return { word: 'What the installation holds is not on record', tone: 'neutral' }
}

const ADDED_TITLE = 'GitHub put it on the token unasked. Talaria never asks for it, and None here does not take it off.'

/** The token's word for a key of the set, from its `keyState`. */
function tokenWord(state: KeyState, nowMs: number): MatrixRow['token'] {
  switch (state.kind) {
    case 'onToken':
      return { word: `On the token (${state.repo}, ${since(state.at, nowMs)})`, tone: 'ok' }
    case 'offToken':
      return { word: `Not on the token (${state.repo}, ${since(state.at, nowMs)}): ${offTokenWhy(state)}`, tone: 'warn' }
    case 'notYet':
      return { word: 'Not checked', tone: 'idle', check: true }
    case 'record':
      if (state.held.kind === 'held' || state.held.kind === 'unknown') return { word: 'Not checked', tone: 'idle', check: true }
      return { word: 'No token carries it until then', tone: 'warn' }
  }
}

/** The permission list of one host: the catalog met with the agent's set,
 *  what the installation holds, what its tokens carried, what GitHub added,
 *  and what it asked for. `view: 'set'` lists its set, what GitHub added,
 *  what it has an open request for, and every key changed this visit; `all`
 *  lists the whole catalog. A non-empty `filter` searches the whole catalog
 *  by key or name, whatever the view. Grouped Repository then Organization,
 *  each in the catalog's order, a key the catalog does not know after. */
export function matrixRows(
  block: HostPermissions,
  catalog: CatalogEntry[],
  opts: { view: MatrixView; filter?: string; requested?: readonly string[]; touched?: ReadonlySet<string> | readonly string[]; nowMs: number },
): Record<PermissionScope, MatrixRow[]> {
  const set = block.permissions ?? {}
  const added = new Map(addedKeys(block).map((a) => [a.key, a]))
  const requested = new Set(opts.requested ?? [])
  const touched = new Set(opts.touched ?? [])
  const q = (opts.filter ?? '').trim().toLowerCase()
  const lastKey = Object.keys(set).length === 1 ? Object.keys(set)[0] : null

  // Every key that can be drawn: the catalog, then set keys it does not know.
  const entries: CatalogEntry[] = [
    ...catalog,
    ...Object.entries(set)
      .filter(([key]) => !catalog.some((e) => e.key === key))
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([key, level]): CatalogEntry => ({ key, scope: scopeOf(catalog, key), levels: [level] })),
  ]
  const shown = (key: string): boolean => {
    if (q) return key.toLowerCase().includes(q) || keyLabel(key).toLowerCase().includes(q)
    if (opts.view === 'all') return true
    return key in set || added.has(key) || requested.has(key) || touched.has(key)
  }

  const rows = entries.filter((e) => shown(e.key)).map((e): MatrixRow => {
    const level = set[e.key] ?? null
    const metadata = e.key === 'metadata'
    const options: LevelOption[] = [
      ...(metadata
        ? []
        : [
            {
              id: 'none' as const,
              label: LEVEL_WORD.none,
              disabled: e.key === lastKey,
              ...(e.key === lastKey ? { title: 'An agent keeps at least one permission. Set another before removing this one.' } : {}),
            },
          ]),
      ...e.levels.map((l): LevelOption => ({ id: l, label: LEVEL_WORD[l], disabled: false })),
    ]
    const add = level == null ? (added.get(e.key) ?? null) : null
    let token: MatrixRow['token']
    let held = heldWord(block, e.key, level, e.levels)
    if (metadata && (level == null || level === 'read')) {
      token = { word: 'On every token', tone: 'idle', title: 'GitHub puts metadata: read on every token.' }
      held = { word: '', tone: 'neutral' }
    } else if (level != null) {
      token = tokenWord(keyState(block, e.key, level, e.scope), opts.nowMs)
    } else if (add) {
      token = { word: `GitHub added it, ${add.level} (${add.repo}, ${since(add.at, opts.nowMs)})`, tone: add.level === 'read' ? 'idle' : 'warn', title: ADDED_TITLE }
    } else {
      token = { word: 'Not in its set', tone: 'idle' }
    }
    return {
      key: e.key,
      label: keyLabel(e.key),
      scope: e.scope,
      level,
      options,
      held,
      token,
      impact: impactNote(e.key),
      added: add,
      requested: requested.has(e.key),
    }
  })
  return {
    repository: rows.filter((r) => r.scope === 'repository'),
    organization: rows.filter((r) => r.scope === 'organization'),
  }
}

/** The keys this agent has an open request for on the host. */
export const requestedKeys = (requests: PermissionRequest[], agentId: string, host: string): string[] =>
  requests.flatMap((r) => (isDecidable(r) && r.agentId === agentId && r.host === host ? [r.permission] : []))
