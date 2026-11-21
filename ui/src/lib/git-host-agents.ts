// Each agent's GitHub token on one git host (GET
// /api/workbench/git-hosts/{id}/agents), as Agent policy › GitHub access and
// the Git hosts dialog read it. The installation's accepted set is the most
// an admin may grant an agent, never what any token carries. `unclaimed` is
// what the installation accepted that no agent's set names: no token carries
// it until an admin grants it to an agent. PURE, so the node-env tests pin it.
//
// The route is newer than the dialog's other reads. A server without it
// answers 404, and the readers draw no unclaimed notice.

import { addedOf, since, type LastToken } from './agent-github-permissions'
import { permissionEntries, permissionSummary, type PermissionMap } from './git-permissions'

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

/** What the installation accepted that no agent's set names, said where an
 *  admin grants it. With `host`, the overview's form, which names the host
 *  among several; without, the host dialog's own. Null when there is
 *  nothing. */
export function unclaimedSentence(keys: string[], host?: string): string | null {
  if (keys.length === 0) return null
  if (host) return `Accepted by the installation on ${host}, granted to no agent: ${keys.join(', ')}. Open an agent to grant them.`
  return `Accepted by the installation, granted to no agent: ${keys.join(', ')}. An admin grants them per agent in Agent policy › GitHub access.`
}

export type TokenTone = 'success' | 'warn' | 'neutral'

/** The last token in one line: where, when, and what it carried or why it
 *  was refused, and what GitHub added to it unasked. A token minted for an
 *  earlier set says so; a row from before rows said what was asked says that
 *  instead, since its set is unknown. */
export function lastTokenLine(t: LastToken | null | undefined, nowMs: number): { text: string; tone: TokenTone } {
  if (!t) return { text: 'No token yet', tone: 'neutral' }
  const where = `${t.repo ? `${t.repo}, ` : ''}${since(t.at, nowMs)}`
  if (!t.ok) return { text: `Refused (${where}): ${t.error?.trim() || 'the door gave no reason'}`, tone: 'warn' }
  if (t.source === 'workspace_secret') return { text: `A workspace secret answered (${where})${t.secret ? `: ${t.secret}` : ''}`, tone: 'neutral' }
  const short = permissionEntries(t.shortfall)
  const stale = t.current !== false ? '' : t.asked == null ? ', recorded before Talaria noted what it asked for' : ', minted for an earlier set'
  const carried = t.carried ? `carried ${permissionSummary(t.carried)}` : 'carried what is not on record'
  const lacks = short.length > 0 ? `; not ${permissionSummary(t.shortfall)}` : ''
  const extra = permissionEntries(addedOf(t))
  const added = extra.length > 0 ? `; GitHub added ${extra.map((e) => `${e.key}: ${e.level}`).join(', ')}` : ''
  const tone: TokenTone = short.length > 0 ? 'warn' : t.current === true ? 'success' : 'neutral'
  return { text: `Last token (${where}${stale}): ${carried}${lacks}${added}`, tone }
}
