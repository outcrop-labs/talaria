// The GitHub permissions agents asked for (`request_git_permission`), as
// Admin → Git hosts decides them: the shapes of
// `GET /api/workbench/permission-requests` and the words the section shows.
// PURE, so the node-env tests pin it (nothing under routes/ can be unit
// tested).
//
// A request a reader may not decide arrives as a FACT only (`content:
// false`): that one waits, on which host, since when. It was raised on a
// board the reader is not a member of, and the agent's words quote that
// board's work; the server never sends them, so nothing here can show them.

import type { PermissionLevel } from './git-permissions'

export type PermissionRequestStatus = 'pending' | 'awaiting_github'

/** One raise, as the server recorded it. */
export interface PermissionRequestEvidence {
  source: 'agent' | string
  verified: boolean
  repo?: string
  taskId?: string | null
  at?: string
}

/** The ticket a request was raised on: its column, and where Restart moves
 *  it when it is Blocked (null: not Blocked, or the board has no column to
 *  restart into). */
export interface PermissionRequestTicket {
  id: string
  ref: string | null
  title: string | null
  boardId: string
  status: string
  statusLabel: string
  blocked: boolean
  restartStatus: string | null
}

/** A request this admin may decide: all of it. */
export interface DecidablePermissionRequest {
  id: string
  content: true
  status: PermissionRequestStatus
  host: string
  hostName: string
  /** Where the need arose. */
  repo: string
  permission: string
  level: PermissionLevel
  agentId: string
  agentName: string
  agentModel: string
  /** The agent's words, every raise's. Plain text, never markup. */
  why: string
  evidence: PermissionRequestEvidence[]
  /** Whether the installation that mints the repo's token holds it. Null:
   *  never read, or no installation selected for the repo. */
  held: boolean | null
  installation: string | null
  /** Where an owner of the account accepts a permission on GitHub. */
  installationUrl: string | null
  /** Whether the agent's set on the host already carries it. */
  inSet: boolean | null
  /** Every repo the agent holds on the host: an approval applies to all. */
  appliesTo: string[]
  ticket: PermissionRequestTicket | null
  decidedBy: string | null
  decisionNote: string | null
  createdAt: string
  updatedAt: string
}

/** A request this admin may not decide: that it waits, nothing it says. */
export interface FactPermissionRequest {
  id: string
  content: false
  status: PermissionRequestStatus
  host: string
  hostName: string
  createdAt: string
}

export type PermissionRequest = DecidablePermissionRequest | FactPermissionRequest

export const isDecidable = (r: PermissionRequest): r is DecidablePermissionRequest => r.content

/** `workflows write` — the way every line about a request says it. */
export const askedLabel = (r: Pick<DecidablePermissionRequest, 'permission' | 'level'>): string =>
  `${r.permission} ${r.level}`

/** Where a request stands, in a chip's words. */
export const statusLabel = (status: PermissionRequestStatus): string =>
  status === 'awaiting_github' ? 'approved, waiting on GitHub' : 'waiting on you'

/** What GitHub's side says, in a chip's words. */
export function heldLabel(held: boolean | null): string {
  if (held === true) return 'the installation holds it'
  if (held === false) return 'the installation has not accepted it'
  return 'what the installation holds is not known'
}

/** The approve confirmation: what it gives, to whom, and every repo it
 *  applies to, by name. An approval is per agent and host, so naming the
 *  repo the need arose on alone would understate it. */
export function approveConfirmMessage(r: DecidablePermissionRequest): string {
  const repos = r.appliesTo.length ? r.appliesTo.join(', ') : r.repo
  const lines = [
    `Give ${r.agentName} ${askedLabel(r)} on ${r.hostName}?`,
    `It applies to every repo ${r.agentName} holds there: ${repos}. Its git token for each of them carries it, and code running in its sandbox can use it until the token expires.`,
  ]
  if (r.held === false) {
    lines.push(
      "The App's installation has not accepted it yet, so no token carries it until an owner of the GitHub account does. The request stays open, waiting on GitHub, until then.",
    )
  } else if (r.held === null) {
    lines.push('What the installation holds is not known yet; the request stays open until a refresh shows GitHub holds it.')
  }
  return lines.join('\n\n')
}

/** One evidence entry, in a line: who reported it, where, and when. */
export function evidenceLine(e: PermissionRequestEvidence): string {
  const who = e.source === 'agent' && !e.verified ? 'reported by the agent (unverified)' : `reported by ${e.source}`
  const where = e.repo ? ` on ${e.repo}` : ''
  const when = e.at ? `, ${e.at.slice(0, 16).replace('T', ' ')} UTC` : ''
  return `${who}${where}${when}`
}

/** The ticket's link in the app. */
export const ticketHref = (t: PermissionRequestTicket): string => `/boards/${t.boardId}/${t.id}`
