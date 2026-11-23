// One agent's repository grants and each one's branch rules (GET/PUT
// /api/workbench/repos/{agentId}), as its GitHub access editor reads and
// writes them. Pure but for `saveRepoGrants`, the one write, so the node-env
// tests pin both the shapes and the conflict path.
//
// The write is a compare-and-set. It sends back what the editor read as
// `from` (the GET's `granted` and `rules`). A server that finds the agent
// holding anything else, because another manager wrote meanwhile or an
// approval added a repo, answers 409 with what is there now and writes
// nothing. The editor then re-reads and says what changed, the way the
// permissions editor does, so two editors never silently revert each other.

import { putJsonOr } from './fetch-json'

/** One repository on one host: the same owner/name on two hosts is two. */
export interface HostRepo {
  host: string
  repo: string
}

export type PushMode = 'branches_only' | 'free'

/** One grant's branch law: the base a human merges, whether the agent may
 *  push it, and the prefix its own branches carry. */
export interface RepoRule extends HostRepo {
  baseBranch: string | null
  pushMode: PushMode
  branchPrefix: string | null
}

/** GET /api/workbench/repos/{agentId}. */
export interface ReposBody {
  hosts: Array<{ id: string; name: string; kind: string; error: string | null }>
  available: HostRepo[]
  granted: HostRepo[]
  rules?: RepoRule[]
  /** Keyed `host:owner/name`. */
  branches?: Record<string, string[]>
}

/** What an editor read, sent back as the write's `from`; a 409's `current`
 *  is the same shape. */
export interface GrantsRead {
  repos: HostRepo[]
  rules: RepoRule[]
}

export const repoKey = (r: HostRepo): string => `${r.host}:${r.repo}`
export const sameRepo = (a: HostRepo, b: HostRepo): boolean => a.host === b.host && a.repo === b.repo

/** The rules a grant starts with: the repo's default branch, branches only,
 *  no prefix. */
export const defaultRule = (r: HostRepo): RepoRule => ({ host: r.host, repo: r.repo, baseBranch: null, pushMode: 'branches_only', branchPrefix: null })

/** The `from` a write sends: exactly what the GET said. */
export const readOf = (d: Pick<ReposBody, 'granted' | 'rules'>): GrantsRead => ({
  repos: d.granted.map((r) => ({ host: r.host, repo: r.repo })),
  rules: d.rules ?? [],
})

/** Rule edits not saved yet, keyed `host:owner/name`, and the read the first
 *  of them was made against (null while there are none). */
export interface PendingRules {
  base: Pick<ReposBody, 'granted' | 'rules'> | null
  edits: Record<string, RepoRule>
}

/**
 * One write: the whole grant set, every granted repo's rules, and the read
 * it is `from`. `read` is the read the action was built on (an add or a
 * remove carries its own, taken before any confirm). Pending rule edits were
 * made against `pending.base`, which then wins, and the grants and untouched
 * rules come from that same read: a write is from the oldest read anything
 * in it came from. A refetch between an edit and its save (a window focus,
 * an approval re-reading the repos) therefore never hands a stale body a
 * fresh `from`, which would pass the compare and put back what another
 * manager changed meanwhile; it gets the 409 instead.
 */
export function grantsWrite(
  read: Pick<ReposBody, 'granted' | 'rules'>,
  pending: PendingRules,
  change: (granted: HostRepo[]) => HostRepo[] = (granted) => granted,
): { from: GrantsRead; repos: HostRepo[]; rules: RepoRule[] } {
  const base = pending.base ?? read
  const repos = change(base.granted).map((r) => ({ host: r.host, repo: r.repo }))
  const rules = repos.map((r) => pending.edits[repoKey(r)] ?? base.rules?.find((x) => sameRepo(x, r)) ?? defaultRule(r))
  return { from: readOf(base), repos, rules }
}

/** A rule as the server stores it, so a prefix typed "agent/" and the stored
 *  "agent" are one rule: a blank base is the default, a mode that is not
 *  'free' is 'branches_only', a prefix is trimmed of spaces and '/'. */
export function normalizedRule(r: RepoRule): RepoRule {
  const prefix = (r.branchPrefix ?? '').trim().replace(/^\/+|\/+$/g, '')
  return {
    host: r.host,
    repo: r.repo,
    baseBranch: r.baseBranch ? r.baseBranch : null,
    pushMode: r.pushMode === 'free' ? 'free' : 'branches_only',
    branchPrefix: prefix === '' ? null : prefix,
  }
}

const ruleIn = (read: GrantsRead, r: HostRepo): RepoRule => normalizedRule(read.rules.find((x) => sameRepo(x, r)) ?? defaultRule(r))
const sameRule = (a: RepoRule, b: RepoRule): boolean =>
  a.baseBranch === b.baseBranch && a.pushMode === b.pushMode && a.branchPrefix === b.branchPrefix

/** At most three names, then how many more. */
function names(list: string[]): string {
  const shown = list.slice(0, 3)
  const more = list.length - shown.length
  if (more > 0) return `${shown.join(', ')} and ${more} more`
  if (shown.length <= 1) return shown.join('')
  return `${shown.slice(0, -1).join(', ')} and ${shown[shown.length - 1]}`
}

/** "A", "A and B", "A, B and C". */
function joined(clauses: string[]): string {
  if (clauses.length <= 1) return clauses.join('')
  return `${clauses.slice(0, -1).join(', ')} and ${clauses[clauses.length - 1]}`
}

/**
 * What changed between what an editor read and what is there now, one
 * clause per kind: the repos granted since, the repos removed since, and
 * the repos whose branch rules changed. A repo is named by owner/name, with
 * its host's name when the two reads span more than one host. Empty when
 * nothing an editor can see moved.
 */
export function grantChanges(read: GrantsRead, now: GrantsRead, hostName: (id: string) => string = (id) => id): string[] {
  const hosts = new Set([...read.repos, ...now.repos].map((r) => r.host))
  const label = (r: HostRepo) => (hosts.size > 1 ? `${r.repo} on ${hostName(r.host)}` : r.repo)
  const has = (list: HostRepo[], r: HostRepo) => list.some((x) => sameRepo(x, r))
  const added = now.repos.filter((r) => !has(read.repos, r))
  const removed = read.repos.filter((r) => !has(now.repos, r))
  const ruled = now.repos.filter((r) => has(read.repos, r) && !sameRule(ruleIn(read, r), ruleIn(now, r)))
  const clauses: string[] = []
  if (added.length) clauses.push(`${names(added.map(label))} ${added.length === 1 ? 'was' : 'were'} added`)
  if (removed.length) clauses.push(`${names(removed.map(label))} ${removed.length === 1 ? 'was' : 'were'} removed`)
  if (ruled.length) clauses.push(`the branch rules of ${names(ruled.map(label))} changed`)
  return clauses
}

const CHANGED = 'These repositories changed since you read them'

/** A 409 toast's title: that the repositories changed, and what did. It is
 *  the line a toast wraps (the body is one line, cut short), so what moved
 *  goes here. */
export function changedTitle(read: GrantsRead, now: GrantsRead, hostName?: (id: string) => string): string {
  const clauses = grantChanges(read, now, hostName)
  return clauses.length ? `${CHANGED}: ${joined(clauses)}` : CHANGED
}

/** A 409 toast's body, the permissions editor's words. */
export const CHANGED_BODY = 'Showing what is set now. Make your change again if it still applies.'

export type SaveOutcome =
  | { kind: 'saved'; granted: HostRepo[] }
  | { kind: 'changed'; current: GrantsRead; title: string; body: string }

/**
 * Write the agent's grants and the rules of each, from what the editor read.
 * `saved` when it landed; `changed` when the agent held something else by
 * then, with what is there now and the words that say what moved (nothing
 * was written). Any other failure throws with the server's sentence.
 */
export async function saveRepoGrants(
  agentId: string,
  read: GrantsRead,
  next: { repos: HostRepo[]; rules: RepoRule[] },
  hostName?: (id: string) => string,
): Promise<SaveOutcome> {
  const res = await putJsonOr<{ granted?: HostRepo[]; current?: GrantsRead; error?: unknown }>(
    `/api/workbench/repos/${encodeURIComponent(agentId)}`,
    { repos: next.repos.map((r) => ({ host: r.host, repo: r.repo })), rules: next.rules, from: read },
    [409],
  )
  if (res.current) return { kind: 'changed', current: res.current, title: changedTitle(read, res.current, hostName), body: CHANGED_BODY }
  // Only a write that landed answers with its grants. A 409 that brought no
  // `current` (a proxy's page, a body cut short) says nothing landed, and
  // must never read as saved: that would clear the edits and say nothing.
  if (!Array.isArray(res.granted)) {
    throw new Error(typeof res.error === 'string' && res.error.trim() ? res.error : 'The server did not say whether the repositories were saved.')
  }
  return { kind: 'saved', granted: res.granted }
}
