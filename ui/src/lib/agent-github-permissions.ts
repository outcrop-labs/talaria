// One agent's GitHub permissions on each git host, as its repo editor draws
// them (/api/workbench/github-permissions/{agentId}, talaria-git-hosts'
// `permissions`). The set is the agent's WHOLE set on the host, not extras
// above a base: any key GitHub's App catalog knows, at any level GitHub takes
// it at, or the default when an admin set none. PURE, so the node-env tests
// pin it: which installation a key is measured against, why one is not held,
// what the add picker offers first, and the next set each edit writes.
//
// Nothing here gates anything. A key the installation does not hold is still
// settable (no token carries it until the installation accepts it), and the
// high-impact notes say what a key allows, not whether it may be given: the
// server's only check is the catalog.

import type { PermissionLevel, PermissionMap } from './git-permissions'

export type PermissionScope = 'repository' | 'organization'

/** One key GitHub's App catalog knows (talaria-github's `CATALOG`). */
export interface CatalogEntry {
  key: string
  scope: PermissionScope
  levels: PermissionLevel[]
}

/** One installation the host's App is on, as the holdings record has it. */
export interface PermissionsInstallation {
  id: string
  account: string
  /** What the App's registration asked for at the last good read. */
  appPermissions: PermissionMap | null
  /** What the installation had accepted then. Null: never read. */
  held: PermissionMap | null
  /** Its page on GitHub, where an owner of the account accepts. */
  url: string | null
}

/** One granted repo on the host, with the installation that mints its token. */
export interface PermissionsRepo {
  repo: string
  installation?: string | null
  held?: PermissionMap | null
  shortfall?: PermissionMap | null
}

/** One host's block of the GET. On a host whose own token decides
 *  (`hostWide`: a PAT, Forgejo), `permissions` and `isDefault` are null and
 *  nothing below them is sent. */
export interface HostPermissions {
  host: string
  name: string
  kind: string
  narrowing: 'perRepo' | 'hostWide'
  /** Why this host's own token decides, when it does. */
  hostWide: string | null
  /** The agent's set on the host, or the default when `isDefault`. */
  permissions: PermissionMap | null
  isDefault: boolean | null
  /** What the host's App asks for, from the record. Null: not known. */
  appPermissions?: PermissionMap | null
  installations?: PermissionsInstallation[]
  repos: PermissionsRepo[]
  updatedAt?: string | null
}

export interface AgentGithubPermissions {
  agentId: string
  /** Whether this caller writes: admins only. */
  canEdit: boolean
  default: PermissionMap
  catalog: CatalogEntry[]
  hosts: HostPermissions[]
}

const RANK: Record<PermissionLevel, number> = { read: 0, write: 1, admin: 2 }

/** Whether `set` holds `key` at `level` or higher. */
export const holds = (set: PermissionMap | null | undefined, key: string, level: PermissionLevel): boolean =>
  set?.[key] != null && RANK[set[key]] >= RANK[level]

/** Where a key applies: the catalog's word, else GitHub's naming (its new
 *  organization keys are `organization_…`). */
export const scopeOf = (catalog: CatalogEntry[], key: string): PermissionScope =>
  catalog.find((e) => e.key === key)?.scope ?? (key.startsWith('organization_') ? 'organization' : 'repository')

/** The installations that mint the agent's tokens on this host: the ones its
 *  granted repos are minted on, or every one the host uses while it holds no
 *  repo there — the server's `held` measures against the same. */
export function mintingInstallations(block: HostPermissions): PermissionsInstallation[] {
  const all = block.installations ?? []
  const ids = new Set(block.repos.map((r) => r.installation).filter((id): id is string => !!id))
  return ids.size === 0 ? all : all.filter((i) => ids.has(i.id))
}

/** Why a key is, or is not, on the agent's tokens. */
export type HeldState =
  /** Every installation that mints the agent's tokens holds it. */
  | { kind: 'held' }
  /** No installation's holdings are on record yet (an admin's Refresh). */
  | { kind: 'unknown' }
  /** The App's registration does not ask for it: its owner adds it to the
   *  App on GitHub, then each account accepts. */
  | { kind: 'appMissing' }
  /** The App asks for it and an owner of these accounts has not accepted it
   *  on GitHub yet. */
  | { kind: 'awaitingApproval'; installations: PermissionsInstallation[] }
  /** Not held, and what the App asks for is not on record. */
  | { kind: 'notHeld'; installations: PermissionsInstallation[] }

export function heldState(block: HostPermissions, key: string, level: PermissionLevel): HeldState {
  const read = mintingInstallations(block).filter((i) => i.held != null)
  if (read.length === 0) return { kind: 'unknown' }
  const lacking = read.filter((i) => !holds(i.held, key, level))
  if (lacking.length === 0) return { kind: 'held' }
  const app = block.appPermissions ?? lacking.find((i) => i.appPermissions != null)?.appPermissions ?? null
  if (app == null) return { kind: 'notHeld', installations: lacking }
  if (!holds(app, key, level)) return { kind: 'appMissing' }
  return { kind: 'awaitingApproval', installations: lacking }
}

/** The sentence a key that is not held carries, or null when it is. */
export function heldSentence(state: HeldState): string | null {
  switch (state.kind) {
    case 'held':
      return null
    case 'unknown':
      return 'What the installation holds is not on record yet: Refresh in Admin → Git hosts reads it.'
    case 'appMissing':
      return "The GitHub App doesn't ask for this: its owner adds it to the App on GitHub, then each account accepts it. No token carries it until then."
    case 'awaitingApproval':
      return `Waiting for an owner of ${accounts(state.installations)} to accept it on GitHub. No token carries it until then.`
    case 'notHeld':
      return `The installation on ${accounts(state.installations)} doesn't hold this, so no token carries it yet.`
  }
}

const accounts = (installations: PermissionsInstallation[]): string =>
  installations.map((i) => i.account || `installation ${i.id}`).join(', ') || 'the account'

/** What a high-impact key lets the agent do, said beside it. Information,
 *  not a gate: an admin may give any of them. */
export function impactNote(key: string): string | null {
  if (key === 'administration')
    return 'Administration lets the agent change the repository itself: its settings, branch protection rules, collaborators and teams, and archive or delete it.'
  if (key === 'secrets')
    return "Secrets lets the agent create, update and delete the repository's Actions secrets (write), or list their names (read). It never reads their values."
  if (key === 'workflows')
    return 'Workflows lets the agent change files under .github/workflows: what runs in CI, with the repository secrets that CI can read.'
  if (key.startsWith('organization_') || key === 'members')
    return 'An organization permission reaches past this repository to the whole organization. GitHub takes some of them only on an organization token; where it refuses one on a repository token, the agent gets the rest.'
  return null
}

/** One key of the agent's set, ready to draw. */
export interface SetRow {
  key: string
  level: PermissionLevel
  /** The levels GitHub takes it at; just its own level for a key the catalog
   *  does not know. */
  levels: PermissionLevel[]
  scope: PermissionScope
}

/** The set as rows, grouped Repository then Organization, each in the
 *  catalog's order (a key the catalog does not know after the rest). */
export function setRows(set: PermissionMap | null, catalog: CatalogEntry[]): Record<PermissionScope, SetRow[]> {
  const order = (key: string) => {
    const i = catalog.findIndex((e) => e.key === key)
    return i === -1 ? catalog.length : i
  }
  const rows = Object.entries(set ?? {})
    .map(([key, level]) => ({
      key,
      level,
      levels: catalog.find((e) => e.key === key)?.levels ?? [level],
      scope: scopeOf(catalog, key),
    }))
    .sort((a, b) => order(a.key) - order(b.key) || a.key.localeCompare(b.key))
  return {
    repository: rows.filter((r) => r.scope === 'repository'),
    organization: rows.filter((r) => r.scope === 'organization'),
  }
}

/** An option of the add picker. */
export interface AddOption {
  key: string
  scope: PermissionScope
  /** What the installation (or the App) says about it, for the row's sub. */
  note: string
  /** The level it is added at. */
  level: PermissionLevel
}

/** Every catalog key the set does not name. What the installations that mint
 *  the agent's tokens hold comes first (it rides at once), then what the App
 *  asks for that an owner has not accepted yet, then the rest; the catalog's
 *  order within each. A key is added at the highest level GitHub takes it at
 *  that the installation holds, else the lowest it takes. */
export function addOptions(block: HostPermissions, catalog: CatalogEntry[]): AddOption[] {
  const set = block.permissions ?? {}
  const minting = mintingInstallations(block).filter((i) => i.held != null)
  const app = block.appPermissions ?? null
  const ranked = catalog
    .filter((e) => !(e.key in set))
    .map((e, i) => {
      const heldAt = minting.length === 0 ? undefined : [...e.levels].reverse().find((l) => minting.every((m) => holds(m.held, e.key, l)))
      if (heldAt) return { rank: 0, i, option: { key: e.key, scope: e.scope, level: heldAt, note: `${e.scope} · held` } }
      const asked = app != null && e.levels.some((l) => holds(app, e.key, l))
      const note = asked ? `${e.scope} · the App asks, not accepted yet` : app == null ? e.scope : `${e.scope} · the App doesn't ask for it`
      return { rank: asked ? 1 : 2, i, option: { key: e.key, scope: e.scope, level: e.levels[0] ?? 'read', note } }
    })
  return ranked.sort((a, b) => a.rank - b.rank || a.i - b.i).map((r) => r.option)
}

/** The set with `key` at `level`. */
export const withPermission = (set: PermissionMap, key: string, level: PermissionLevel): PermissionMap => ({ ...set, [key]: level })

/** The set without `key`. */
export function withoutPermission(set: PermissionMap, key: string): PermissionMap {
  const next = { ...set }
  delete next[key]
  return next
}

/** What a PUT says it read: null while the agent is on the default, so a
 *  write from the default compares against whatever the default is now. */
export const fromOf = (block: HostPermissions): PermissionMap | null => (block.isDefault ? null : (block.permissions ?? null))
