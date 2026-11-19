// GitHub App permissions as Admin → Git hosts draws them: what the App's
// registration asks for, what one installation has accepted, and the
// difference — what is still waiting for an owner of the installation's
// account to accept on GitHub. PURE, so the node-env tests pin it (nothing
// under routes/ can be unit tested).
//
// The server sends each set as a `{key: level}` object (talaria-git-host-types'
// PermissionSet), and its `pending` for an installation is the App's ask at
// the installation's last check minus what it held then — the record the
// alerts read. The card measures against the App's ask read live with the
// host's status instead, when there is one: an admin who just added a
// permission to the App sees it waiting at once, not at the next refresh.
// `pendingPermissions` is the same arithmetic as the server's `missing_from`,
// so the two agree whenever the App has not changed in between.

export type PermissionLevel = 'read' | 'write' | 'admin'

/** A permission set as the wire carries it. */
export type PermissionMap = Record<string, PermissionLevel>

export interface PermissionEntry {
  key: string
  level: PermissionLevel
}

const RANK: Record<PermissionLevel, number> = { read: 0, write: 1, admin: 2 }

/** A level the server would never send ranks as nothing held, never as a
 *  guess. */
const rank = (level: string | undefined): number => (level && level in RANK ? RANK[level as PermissionLevel] : -1)

/** The set as entries, by key. Null (unknown) is no entries. */
export function permissionEntries(set: PermissionMap | null | undefined): PermissionEntry[] {
  return Object.entries(set ?? {})
    .map(([key, level]) => ({ key, level }))
    .sort((a, b) => a.key.localeCompare(b.key))
}

/** What `asked` names that `held` does not cover: each key `held` lacks, or
 *  holds below the asked level, at the asked level. */
export function pendingPermissions(asked: PermissionMap | null | undefined, held: PermissionMap | null | undefined): PermissionMap {
  const out: PermissionMap = {}
  for (const { key, level } of permissionEntries(asked)) {
    if (rank(held?.[key]) < rank(level)) out[key] = level
  }
  return out
}

/** One installation measured against the App's ask, ready to draw. */
export interface InstallationDiff {
  /** What the App asks for. */
  asked: PermissionEntry[]
  /** What the installation accepted. */
  accepted: PermissionEntry[]
  /** What the App asks for that the installation has not accepted. */
  waiting: PermissionEntry[]
  /** Whether both sides are known. When one is not, nothing is said to be
   *  waiting: the card says "not checked" instead. */
  known: boolean
}

export function installationDiff(asked: PermissionMap | null | undefined, held: PermissionMap | null | undefined): InstallationDiff {
  const known = asked != null && held != null
  return {
    asked: permissionEntries(asked),
    accepted: permissionEntries(held),
    waiting: known ? permissionEntries(pendingPermissions(asked, held)) : [],
    known,
  }
}

/** `pull_requests: write` — GitHub's key, spaced for reading. */
export const permissionLabel = (e: PermissionEntry): string => `${e.key.replaceAll('_', ' ')}: ${e.level}`

/** One line for a set: `contents: write, metadata: read`, or `nothing`. */
export const permissionSummary = (set: PermissionMap | null | undefined): string => {
  const entries = permissionEntries(set)
  return entries.length ? entries.map(permissionLabel).join(', ') : 'nothing'
}

/** "checked 4 min ago", "checked just now", or "never checked", from an ISO
 *  instant and now (ms). */
export function checkedAgo(iso: string | null | undefined, nowMs: number): string {
  if (!iso) return 'never checked'
  const at = Date.parse(iso)
  if (Number.isNaN(at)) return 'never checked'
  const minutes = Math.max(0, Math.round((nowMs - at) / 60_000))
  if (minutes < 1) return 'checked just now'
  if (minutes < 60) return `checked ${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 48) return `checked ${hours} h ago`
  return `checked ${Math.round(hours / 24)} days ago`
}
