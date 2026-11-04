/** Moving a ticket to another board — the dialog's pure half, split out so
 *  the destination list and the "what changes" wording are unit-testable
 *  without the app's module graph (the access-loss.ts pattern). The server
 *  decides everything (talaria-tasks `move_task`); this only says it. */

/** The server's answer for a move, previewed or done (MoveSummary). */
export interface MoveSummary {
  preview: boolean
  fromBoardId: string
  fromBoardName: string
  toBoardId: string
  toBoardName: string
  fromRef: string | null
  toRef: string | null
  fromStatus: string
  status: string
  statusLabel: string
  statusHow: 'requested' | 'same-name' | 'same-category' | 'intake' | 'kept-closed'
  labelsKept: string[]
  labelsDropped: string[]
  unassigned: string[]
  parentDetached: boolean
  approvalCleared: boolean
  dormantWatchers: string[]
  dependencies: number
}

/** The boards a person may move a ticket INTO: ones they can add tickets to
 *  (owner or editor), live, and not the one it is on. Sorted by name, so the
 *  picker reads the same way every time. The server asks the same question
 *  again; this only keeps the picker from offering a 403. */
export function moveDestinations<B extends { id: string; name: string; role: string; archivedAt: string | null }>(
  boards: B[],
  currentBoardId: string,
): B[] {
  return boards
    .filter((b) => b.id !== currentBoardId && !b.archivedAt && (b.role === 'owner' || b.role === 'editor'))
    .sort((a, b) => a.name.localeCompare(b.name))
}

const HOW: Record<MoveSummary['statusHow'], string> = {
  requested: '',
  'same-name': ' (same column there)',
  'same-category': ' (the closest column there)',
  intake: ' (its intake — no matching column)',
  'kept-closed': ' (stays closed)',
}

/** A person-readable label for an assignee id: `user:<id>` → the member's
 *  name when the caller can say it, an agent id as it is. */
export type AssigneeName = (id: string) => string

/** What the move changes, one line each, in the order a person checks:
 *  the ref, the column, then everything that does not travel. Every line is
 *  something the confirm must show — a dropped label or an unassigned person
 *  is not a detail to discover afterwards. */
export function moveChangeLines(m: MoveSummary, name: AssigneeName = (id) => id): string[] {
  const lines: string[] = []
  if (m.fromRef && m.toRef) lines.push(`Its ref changes from ${m.fromRef} to ${m.toRef} — the old ref still opens it.`)
  else if (m.toRef) lines.push(`It becomes ${m.toRef}.`)
  lines.push(`It lands in ${m.statusLabel}${HOW[m.statusHow] ?? ''}.`)
  if (m.labelsDropped.length) {
    const s = m.labelsDropped.length === 1 ? '' : 's'
    lines.push(`Label${s} dropped — ${m.toBoardName} has no ${m.labelsDropped.join(', ')}.`)
  }
  if (m.unassigned.length) lines.push(`Unassigned — not on ${m.toBoardName}: ${m.unassigned.map(name).join(', ')}.`)
  if (m.parentDetached) lines.push('It leaves its parent ticket behind and becomes a top-level ticket.')
  if (m.approvalCleared) lines.push(`Its sign-off is cleared — ${m.toBoardName}’s reviewers sign it off.`)
  if (m.dormantWatchers.length) {
    lines.push(`Watchers who cannot see ${m.toBoardName} stop hearing about it: ${m.dormantWatchers.join(', ')}.`)
  }
  return lines
}

/** Does the confirm warn? Something does not travel. */
export const moveLosesSomething = (m: MoveSummary) =>
  m.labelsDropped.length > 0 || m.unassigned.length > 0 || m.parentDetached || m.approvalCleared || m.dormantWatchers.length > 0
