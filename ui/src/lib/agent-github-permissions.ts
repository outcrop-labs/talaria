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
//
// What the installation holds is not what the agent's token carries. The
// record says the most any token can carry; the agent's last token (its
// `git.credential` audit row, from GitHub's own mint reply) says what one
// did. `keyState` reads the token first and falls back to the record only
// while nothing was minted, and then never calls it verified. Every field
// the token view reads is optional: a server that does not send them gets
// today's record-only view.

import { permissionSummary, type PermissionLevel, type PermissionMap } from './git-permissions'

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

/** The newest credential the door served, or refused, the agent for one
 *  repo: its `git.credential` (or `git.credential_refused`) audit row. Never
 *  the token. */
export interface LastToken {
  /** When (ISO). */
  at: string
  /** False: the door refused it, and `error` says why. */
  ok: boolean
  /** `git_host` (the App's installation token) or `workspace_secret`. */
  source?: string | null
  /** The set it was minted for. */
  asked?: PermissionMap | null
  /** Exactly what GitHub's mint reply put on the token. */
  carried?: PermissionMap | null
  /** What was asked and is not on the token. */
  shortfall?: PermissionMap | null
  /** GitHub's own words for the part it would not put on the token. */
  shortfallWhy?: string | null
  /** The door's sentence when it refused. */
  error?: string | null
  /** Minted then, rather than served from the cache. */
  minted?: boolean | null
  /** Minted for the set the agent has now. False: the set changed since,
   *  so the token says nothing about it. */
  current?: boolean | null
  /** The workspace secret that answered, by name (`workspace_secret`). */
  secret?: string | null
  /** The host route's newest-across-repos only: which repo. */
  repo?: string | null
}

/** One granted repo on the host, with the installation that mints its token. */
export interface PermissionsRepo {
  repo: string
  installation?: string | null
  held?: PermissionMap | null
  shortfall?: PermissionMap | null
  /** Its newest credential. Null: none yet. Absent: the server does not say. */
  lastToken?: LastToken | null
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
  /** The workspace secret that answers git at this host for the agent ahead
   *  of the App, by name: then none of these permissions apply. */
  workspaceSecret?: string | null
}

export interface AgentGithubPermissions {
  agentId: string
  /** Whether this caller writes: admins only. */
  canEdit: boolean
  /** Whether this caller may Check token: admins only. */
  canCheck?: boolean
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
 *  repo there — the server's `held` measures against the same. Repos that
 *  no installation on record mints are none: what holds them is unknown,
 *  not some other account's installation. */
export function mintingInstallations(block: HostPermissions): PermissionsInstallation[] {
  const all = block.installations ?? []
  if (block.repos.length === 0) return all
  const ids = new Set(block.repos.map((r) => r.installation).filter((id): id is string => !!id))
  return all.filter((i) => ids.has(i.id))
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

// ── What the agent's token carries ──────────────────────────────────────────

/** "just now", "12 min ago", "3 h ago", "2 days ago". */
export function since(iso: string | null | undefined, nowMs: number): string {
  const at = iso ? Date.parse(iso) : Number.NaN
  if (Number.isNaN(at)) return 'at an unknown time'
  const minutes = Math.max(0, Math.round((nowMs - at) / 60_000))
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 48) return `${hours} h ago`
  return `${Math.round(hours / 24)} days ago`
}

/** Whether the server says anything about the agent's tokens on this host.
 *  False on a server that predates `lastToken`: the editor draws the record
 *  only, exactly as it did. */
export const tokenAware = (block: HostPermissions): boolean => block.repos.some((r) => r.lastToken !== undefined)

/** A repo's last token that speaks for the set the agent has now: the App's
 *  token (a workspace secret's says nothing about these permissions), minted
 *  for this very set, or a refusal no set is said to have outlived. */
const speaks = (t: LastToken | null | undefined): t is LastToken =>
  t != null && (t.source ?? 'git_host') === 'git_host' && (t.current === true || (!t.ok && t.current == null))

/** Why a key the agent's set names is not on its token. */
export type OffTokenWhy =
  /** The installation had not accepted it on GitHub when the token was
   *  minted, so it was never asked for. */
  | 'notAccepted'
  /** GitHub refused it on this token (its words in `detail`). */
  | 'githubRefused'
  /** GitHub minted the token without it. */
  | 'githubOmitted'
  /** No token was minted at all (the door's sentence in `detail`). */
  | 'mintFailed'

/** One key of the agent's set: is it on the agent's token? */
export type KeyState =
  /** The last token minted for this set carries it, and the record still
   *  holds it. */
  | { kind: 'onToken'; repo: string; at: string }
  /** A token minted for this set lacks it. `installations`: the ones that
   *  have not accepted it, where an owner accepts (`notAccepted`);
   *  `acceptedSince`: every installation that mints it holds it now, so the
   *  next token can. */
  | { kind: 'offToken'; repo: string; at: string; why: OffTokenWhy; detail: string | null; installations: PermissionsInstallation[]; acceptedSince: boolean }
  /** Tokens were minted, none known to be for the set the agent has now (the
   *  set changed since, or the rows predate saying what was asked), and the
   *  record holds it or does not say. */
  | { kind: 'notYet' }
  /** The record's word: nothing was minted (or the server does not say), or
   *  what the token says no longer stands because the installation does not
   *  hold the key now. */
  | { kind: 'record'; held: HeldState }

interface Minted {
  repo: PermissionsRepo
  token: LastToken
}

const newest = (xs: Minted[]): Minted | undefined =>
  [...xs].sort((a, b) => (Date.parse(b.token.at) || 0) - (Date.parse(a.token.at) || 0))[0]

const OMITTED = /(?:^|;\s*)(GitHub did not put .*)$/i

/** A token's `shortfallWhy` in its two parts. */
function splitWhy(why: string | null): { refused: string | null; omitted: string | null } {
  if (!why) return { refused: null, omitted: null }
  const m = OMITTED.exec(why)
  if (!m) return { refused: why, omitted: null }
  const refused = why.slice(0, m.index).replace(/;\s*$/, '').trim()
  return { refused: refused || null, omitted: m[1]!.trim() }
}

/** Whether GitHub's sentence names `key` (its sets read `key:level, …`). */
const names = (sentence: string, key: string): boolean => new RegExp(`(?:^|[\\s,(])${key.replace(/[^a-z0-9_]/gi, '')}:`).test(sentence)

/** The record's word for a key, when it is a warning: the installation does
 *  not hold it (or the App does not ask), so no token carries it, whatever
 *  an older token said. Null when the record holds it or does not say. */
function recordWarns(block: HostPermissions, key: string, level: PermissionLevel): KeyState | null {
  const held = heldState(block, key, level)
  return held.kind === 'held' || held.kind === 'unknown' ? null : { kind: 'record', held }
}

/** `scope`: the key's (`scopeOf`). GitHub refuses only organization keys on
 *  a token for one repository, and its 422 message names none of them, so
 *  the refused part of `shortfallWhy` is told to organization keys only. */
export function keyState(block: HostPermissions, key: string, level: PermissionLevel, scope: PermissionScope = scopeOf([], key)): KeyState {
  const record: KeyState = { kind: 'record', held: heldState(block, key, level) }
  if (!tokenAware(block)) return record
  if (!block.repos.some((r) => r.lastToken != null)) return record
  const current: Minted[] = block.repos.flatMap((repo) => (speaks(repo.lastToken) ? [{ repo, token: repo.lastToken }] : []))
  // No token speaks for this set: the record's warning stands (granted
  // before GitHub's acceptance, or never asked of the App), with its link.
  if (current.length === 0) return recordWarns(block, key, level) ?? { kind: 'notYet' }
  const lacking = current.filter(({ token }) => !(token.ok && holds(token.carried, key, level)))
  const cite = newest(lacking.length > 0 ? lacking : current)
  if (!cite) return { kind: 'notYet' }
  const repo = cite.token.repo ?? cite.repo.repo
  // Carried, unless an installation that mints the agent's tokens has lost
  // it since: the next token is narrower, whatever the last one carried.
  if (lacking.length === 0) return recordWarns(block, key, level) ?? { kind: 'onToken', repo, at: cite.token.at }

  const minting = (block.installations ?? []).filter((i) => i.id === cite.repo.installation)
  const lacked = minting.filter((i) => i.held != null && !holds(i.held, key, level))
  const why = cite.token.shortfallWhy?.trim() || null
  const off = (w: OffTokenWhy, detail: string | null, acceptedSince = false): KeyState => ({
    kind: 'offToken',
    repo,
    at: cite.token.at,
    why: w,
    detail,
    installations: w === 'notAccepted' ? lacked : [],
    acceptedSince,
  })
  if (!cite.token.ok) return off('mintFailed', cite.token.error?.trim() || null)
  // The installation does not hold it: it was never asked of GitHub.
  if (lacked.length > 0) return off('notAccepted', null)
  if (holds(cite.token.shortfall, key, level)) {
    // GitHub's words, as the mint writes them: its 422 refusing the
    // organization keys on a token for one repository (a message that names
    // no key), then "GitHub did not put <keys> on the token" for what it
    // left off; either part may stand alone.
    const { refused, omitted } = splitWhy(why)
    if (omitted && names(omitted, key)) return off('githubOmitted', omitted)
    if (refused && scope === 'organization') return off('githubRefused', refused)
    // Dropped before the mint with no word from GitHub: the installation
    // had not accepted it then. It has since only if every installation
    // that mints this repo's token is read and holds it now.
    return off('notAccepted', null, minting.length > 0 && minting.every((i) => i.held != null && holds(i.held, key, level)))
  }
  // Asked, held, and still not on what GitHub put on the token.
  return off('githubOmitted', null)
}

/** The sentence beside a key, from its state. */
export function keySentence(state: KeyState, nowMs: number): string {
  switch (state.kind) {
    case 'onToken':
      return `On the agent's token (${state.repo}, ${since(state.at, nowMs)})`
    case 'notYet':
      return 'Granted; no token checked against this set yet'
    case 'offToken':
      return `Not on the token (${state.repo}, ${since(state.at, nowMs)}): ${offTokenWhy(state)}`
    case 'record':
      return state.held.kind === 'held' ? 'The installation holds it; no token checked yet' : (heldSentence(state.held) ?? '')
  }
}

function offTokenWhy(state: Extract<KeyState, { kind: 'offToken' }>): string {
  switch (state.why) {
    case 'notAccepted':
      if (state.acceptedSince) return 'the installation had not accepted it when this token was minted; it has since, so the next token can carry it'
      // What the installation holds now is not on record: say only what
      // the token's mint knew.
      if (state.installations.length === 0) return 'the installation had not accepted it when this token was minted'
      return `the installation on ${accounts(state.installations)} has not accepted it on GitHub`
    case 'githubRefused':
      return `GitHub refused it: ${state.detail ?? 'it gave no reason'}`
    case 'githubOmitted':
      // "GitHub did not put workflows:write on the token" says no more.
      return state.detail && !/did not put/i.test(state.detail) ? `GitHub omitted it: ${state.detail}` : 'GitHub omitted it from the token'
    case 'mintFailed':
      return `no token was minted: ${state.detail ?? 'the door gave no reason'}`
  }
}

/** Keys that share a state, said once: the editor's lines under the chips. */
export interface KeyLine {
  keys: Array<{ key: string; level: PermissionLevel }>
  state: KeyState
  sentence: string
  /** Where an owner accepts what is not accepted yet. */
  installations: PermissionsInstallation[]
}

/** Each key of the set with what its token says, keys with the same sentence
 *  on one line, in the set's order. */
export function keyLines(block: HostPermissions, catalog: CatalogEntry[], nowMs: number): KeyLine[] {
  const rows = setRows(block.permissions, catalog)
  const lines: KeyLine[] = []
  for (const r of [...rows.repository, ...rows.organization]) {
    const state = keyState(block, r.key, r.level, r.scope)
    const sentence = keySentence(state, nowMs)
    const installations =
      state.kind === 'offToken'
        ? state.installations
        : state.kind === 'record' && (state.held.kind === 'awaitingApproval' || state.held.kind === 'notHeld')
          ? state.held.installations
          : []
    const same = lines.find((l) => l.sentence === sentence)
    if (same) same.keys.push({ key: r.key, level: r.level })
    else lines.push({ keys: [{ key: r.key, level: r.level }], state, sentence, installations })
  }
  return lines
}

/** The chip's tone for a key's state. The record's `held` is never green:
 *  only a minted token is. */
export function keyTone(state: KeyState): 'success' | 'warn' | 'neutral' {
  if (state.kind === 'onToken') return 'success'
  if (state.kind === 'offToken') return 'warn'
  if (state.kind === 'notYet') return 'neutral'
  return state.held.kind === 'held' || state.held.kind === 'unknown' ? 'neutral' : 'warn'
}

/** The workspace-secret notice: git at the host answers from a secret, and
 *  none of these permissions apply to it. Null when the App answers. */
export function workspaceSecretNotice(block: HostPermissions): string | null {
  const name = block.workspaceSecret?.trim()
  return name ? `Git at ${block.name} answers from the workspace secret ${name} for this agent; these permissions don't apply to it.` : null
}

// ── Check token ─────────────────────────────────────────────────────────────

/** Why the check found a key off the token. */
export type CheckWhy = 'notInSet' | 'installationLacks' | 'githubRefused' | 'githubOmitted'

export interface CheckReason {
  key: string
  level: PermissionLevel
  why: CheckWhy | string
  detail?: string | null
}

/** What `POST …/github-permissions/{agentId}/check` answers: the token the
 *  agent's next push gets for one repo (served from the cache, or minted
 *  now), never the token itself. */
export interface TokenCheck {
  ok: boolean
  source?: string | null
  secret?: string | null
  installation?: string | null
  asked?: PermissionMap | null
  carried?: PermissionMap | null
  shortfall?: PermissionMap | null
  shortfallWhy?: string | null
  minted?: boolean | null
  error?: string | null
  reasons?: CheckReason[] | null
  at?: string | null
}

/** The repos Check token offers, in the order the editor lists them. */
export const checkRepos = (block: HostPermissions): string[] => block.repos.map((r) => r.repo)

/** A reason that is not a fault: the installation holds a key the agent's
 *  set does not name. On an installation given every permission, that is
 *  most of the catalog, and the agent's token is still exactly its set. */
const isNotInSet = (r: CheckReason): boolean => r.why === 'notInSet'

/** Whether a check found the App's token carrying the whole set: no error,
 *  no reason a key of the set is off it, not a workspace secret answering
 *  instead. Keys the installation holds outside the set are not a fault. */
export const checkClean = (c: TokenCheck): boolean =>
  c.ok && !c.error?.trim() && !(c.reasons ?? []).some((r) => !isNotInSet(r)) && c.source !== 'workspace_secret'

/** One reason as a plain sentence. */
export function reasonSentence(r: CheckReason, account?: string | null): string {
  const what = `${r.key}: ${r.level}`
  const detail = r.detail?.trim()
  switch (r.why) {
    case 'notInSet':
      return `${what} is not in this agent's set, though the installation holds it. Add it here to put it on the token.`
    case 'installationLacks':
      return `${what} is in the agent's set, but the installation on ${account || 'the account'} has not accepted it on GitHub, so no token carries it.`
    case 'githubRefused':
      return `${what}: GitHub refused it on this token${detail ? `: ${detail}` : '.'}`
    case 'githubOmitted':
      return `${what}: GitHub minted the token without it${detail ? `: ${detail}` : '.'}`
    default:
      return `${what}: ${detail || r.why}`
  }
}

/** One line of a check's answer, with its own tone. */
export interface CheckLine {
  text: string
  tone: 'success' | 'warn' | 'neutral'
}

/** What a check says, as lines, never none: the error verbatim when it
 *  failed, the workspace secret when one answers, else what the token
 *  carries and each reason a key of the set is off it. What the
 *  installation holds outside the set is said once, as information. */
export function checkLines(c: TokenCheck, repo: string, host: string, account?: string | null): CheckLine[] {
  const out: CheckLine[] = []
  const reasons = c.reasons ?? []
  const faults = reasons.filter((r) => !isNotInSet(r))
  const outside = reasons.filter(isNotInSet)
  if (c.error?.trim()) out.push({ text: c.error.trim(), tone: 'warn' })
  if (c.source === 'workspace_secret') {
    out.push({ text: `Git at ${host} answers from the workspace secret ${c.secret || '(unnamed)'} for this agent; these permissions don't apply to it.`, tone: 'warn' })
  } else if (c.ok && c.carried) {
    out.push({
      text: `The token for ${repo} carries ${permissionSummary(c.carried)}${c.minted ? ' (minted now)' : ' (served from the cache)'}.`,
      tone: checkClean(c) ? 'success' : 'neutral',
    })
  }
  for (const r of faults) out.push({ text: reasonSentence(r, account), tone: 'warn' })
  if (outside.length === 1) out.push({ text: reasonSentence(outside[0]!, account), tone: 'neutral' })
  else if (outside.length > 1) {
    const keys = outside.map((r) => `${r.key}: ${r.level}`).join(', ')
    out.push({ text: `The installation also holds ${keys}; none is in this agent's set. Add any of them here to put it on the token.`, tone: 'neutral' })
  }
  if (out.length === 0)
    out.push(c.ok ? { text: `The token for ${repo} was checked; GitHub said nothing more.`, tone: 'neutral' } : { text: 'The check failed and gave no reason.', tone: 'warn' })
  return out
}

/** `checkLines`' words alone. */
export const checkSentences = (c: TokenCheck, repo: string, host: string, account?: string | null): string[] =>
  checkLines(c, repo, host, account).map((l) => l.text)

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
