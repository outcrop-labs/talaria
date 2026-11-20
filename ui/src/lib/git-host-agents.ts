// Each agent's GitHub token on one git host, as Admin → Git hosts' Permissions
// dialog lists them (GET /api/workbench/git-hosts/{id}/agents). The
// installation's accepted set beside it is the most any token can carry; this
// is what each agent's token does carry: its set (or the default) as far as
// the installation holds it, and its last token, from GitHub's own mint reply.
// `unclaimed` is what the installation accepted that no agent's set names: no
// token carries it, however green the installation's chips are. PURE, so the
// node-env tests pin it.
//
// The route is newer than this dialog's other reads. A server without it
// answers 404, and the dialog draws what it always did.

import { since, type LastToken } from './agent-github-permissions'
import { pendingPermissions, permissionEntries, permissionSummary, type PermissionEntry, type PermissionMap } from './git-permissions'

/** The read's query key: under the host's own, so refreshing the host
 *  re-reads it too. */
export const gitHostAgentsKey = (id: string) => ['git-host', id, 'agents']

/** One agent with a set or a repo grant on the host. */
export interface HostAgent {
  agentId: string
  name: string
  /** Whether it is a Developer agent (it works repos at all). */
  developer?: boolean
  /** On the default set: no admin set one. Null on a host whose own token
   *  decides. */
  isDefault: boolean | null
  /** Its set on the host, or the default when `isDefault`. */
  set: PermissionMap | null
  /** Its granted repos on the host. */
  repos?: string[]
  /** Its set as far as the installations that mint its tokens hold it, plus
   *  metadata. Null: what they hold is not on record. */
  carries: PermissionMap | null
  /** Its newest credential across its repos on the host, with `repo`. */
  lastToken?: LastToken | null
}

export interface HostAgents {
  agents: HostAgent[]
  /** What the installations accepted, beyond metadata, that no Developer
   *  agent's set names. A map or a key list; absent on an older server. */
  unclaimed?: PermissionMap | string[] | null
}

/** The unclaimed keys, in the server's order, metadata never among them
 *  (every token carries it). */
export function unclaimedKeys(body: HostAgents | null | undefined): string[] {
  const u = body?.unclaimed
  if (!u) return []
  const keys = Array.isArray(u) ? u : Object.keys(u)
  return keys.filter((k) => typeof k === 'string' && k !== '' && k !== 'metadata')
}

/** The line the dialog says when the installation accepted something no
 *  agent carries. Null when there is nothing. */
export function unclaimedSentence(keys: string[]): string | null {
  if (keys.length === 0) return null
  return `Accepted by the installation, carried by no agent: ${keys.join(', ')}. An admin grants them per agent (Fleet → agent → Workbench → GitHub permissions).`
}

export type TokenTone = 'success' | 'warn' | 'neutral'

/** One agent, ready to draw. */
export interface HostAgentRow {
  agentId: string
  name: string
  isDefault: boolean
  /** False when its Developer switch is off or it is disabled: the door
   *  gives it no git credential at all, whatever its set. */
  developer: boolean
  /** Its set, by key. */
  set: PermissionEntry[]
  /** What its tokens can carry, by key; null when not known. */
  carries: PermissionEntry[] | null
  /** What its set names that its tokens cannot carry. */
  beyond: PermissionEntry[]
  /** Its last token in one line. */
  token: string
  tone: TokenTone
}

/** The last token in one line: where, when, and what it carried or why it
 *  was refused. A token minted for an earlier set says so; a row from before
 *  rows said what was asked says that instead, since its set is unknown. */
export function lastTokenLine(t: LastToken | null | undefined, nowMs: number): { text: string; tone: TokenTone } {
  if (!t) return { text: 'No token yet', tone: 'neutral' }
  const where = `${t.repo ? `${t.repo}, ` : ''}${since(t.at, nowMs)}`
  if (!t.ok) return { text: `Refused (${where}): ${t.error?.trim() || 'the door gave no reason'}`, tone: 'warn' }
  if (t.source === 'workspace_secret') return { text: `A workspace secret answered (${where})${t.secret ? `: ${t.secret}` : ''}`, tone: 'neutral' }
  const short = permissionEntries(t.shortfall)
  const stale = t.current !== false ? '' : t.asked == null ? ', recorded before Talaria noted what it asked for' : ', minted for an earlier set'
  const carried = t.carried ? `carried ${permissionSummary(t.carried)}` : 'carried what is not on record'
  const lacks = short.length > 0 ? `; not ${permissionSummary(t.shortfall)}` : ''
  const tone: TokenTone = short.length > 0 ? 'warn' : t.current === true ? 'success' : 'neutral'
  return { text: `Last token (${where}${stale}): ${carried}${lacks}`, tone }
}

export function hostAgentRow(a: HostAgent, nowMs: number): HostAgentRow {
  const set = permissionEntries(a.set)
  const carries = a.carries == null ? null : permissionEntries(a.carries)
  const beyond = a.carries == null ? [] : permissionEntries(pendingPermissions(a.set, a.carries))
  const { text, tone } = lastTokenLine(a.lastToken, nowMs)
  const developer = a.developer !== false
  return {
    agentId: a.agentId,
    name: a.name || a.agentId,
    isDefault: a.isDefault === true,
    developer,
    set,
    // No credential at all: nothing is carried, and nothing falls short.
    carries: developer ? carries : null,
    beyond: developer ? beyond : [],
    token: text,
    tone,
  }
}

/** The agents in the order the dialog lists them: Developer agents first,
 *  then by name. */
export function hostAgentRows(body: HostAgents | null | undefined, nowMs: number): HostAgentRow[] {
  return [...(body?.agents ?? [])]
    .sort((a, b) => Number(b.developer !== false) - Number(a.developer !== false) || (a.name || '').localeCompare(b.name || ''))
    .map((a) => hostAgentRow(a, nowMs))
}
