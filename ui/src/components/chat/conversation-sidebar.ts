// Shared types for the session rail (see ConversationSidebar.svelte), and the
// arithmetic the ORGANISED rail runs on: which section a row belongs to, and
// what a drop has to write to put it where it was dropped.
//
// It all lives here rather than in the component because every one of these is
// a pure function of the list, and the component is the one place a test
// cannot reach (vitest is node-only, and a rail is DOM).

import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
import type { Conversation } from '@/lib/conversations.svelte'

/** A read that FAILED, handed down from the route that owns the query. This
 *  component only ever sees already-defaulted arrays (`= []`), so an outage and
 *  a genuinely empty list arrive here looking exactly alike — the caller has to
 *  tell it which one it is, or "No sessions yet." goes out over a
 *  500 and a person's plans read as never having existed. */
export type SidebarFailure = { error: unknown; retry: () => void } | null

/** One run of rows under one heading.
 *
 *  `label` is null for the ungrouped tail on purpose: those rows are the rail
 *  as it has always looked, and giving them a header ("Other", "Sessions")
 *  would label the default — the one thing placement already says. */
export interface RailSection {
  /** Stable across renders, and distinct from a group whose NAME happens to be
   *  "Pinned": `group:` prefixes the user's own strings so they cannot collide
   *  with the two fixed sections. */
  key: string
  label: string | null
  /** The group this section IS, for a drop onto its header and for the
   *  collapse flag. Null on both fixed sections — neither is a group, and
   *  neither can be dropped into. */
  group: string | null
  items: Conversation[]
}

/** A position write a drop implies. One of these in the common case; one per
 *  row when the list has to be renumbered first (see `reorderPosition`). */
export interface PositionWrite {
  id: string
  position: number
}

/** Sparse by design: a fresh renumber leaves nine free slots between every
 *  pair of rows, so the next few hundred drops are each a single write. */
export const POSITION_STEP = 10

/** The far end of a hand-ordered list — mirrors `MAX_POSITION` in
 *  `talaria-conversation-layout`, which refuses anything past it. Reached, a
 *  drop renumbers rather than sending a write the server would reject. */
export const MAX_POSITION = 100_000

const time = (iso: string | undefined): number => {
  const t = iso ? Date.parse(iso) : NaN
  return Number.isNaN(t) ? 0 : t
}

/** THE SERVER'S ORDER, in the client's hands: hand-placed slots first in
 *  ascending order, rows with no slot after them, newest activity first among
 *  equals. It mirrors `list_conversations_in`'s
 *  `order by … l.position asc nulls last, c.updated_at desc` — the rows arrive
 *  in it already, and re-applying it here is what lets a section be sorted
 *  after an optimistic write has moved one row's position under it. */
export function railOrder(a: Conversation, b: Conversation): number {
  const pa = a.position ?? null
  const pb = b.position ?? null
  if (pa !== pb) {
    // Nulls last: a row nobody placed sits below every row somebody did.
    if (pa === null) return 1
    if (pb === null) return -1
    return pa - pb
  }
  return time(b.updatedAt) - time(a.updatedAt)
}

const byName = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: 'base' })

/** Split the rail into its sections: the viewer's pinned rows, then each group
 *  they have filed rows under (alphabetical, case-insensitive), then
 *  everything else.
 *
 *  A PINNED ROW IS ONLY PINNED. It leads the rail however old it is and
 *  whatever group it is in, so it lists once — at the top — rather than in
 *  both places, which would read as two sessions with one name.
 *
 *  Empty sections are omitted, including the two fixed ones: a group name only
 *  exists because some row carries it, so an empty group cannot be filed under
 *  anything, and a "Pinned" header over nothing is chrome claiming a feature
 *  the viewer has not used. */
export function railSections(convs: Conversation[]): RailSection[] {
  const pinned: Conversation[] = []
  const ungrouped: Conversation[] = []
  const groups = new Map<string, Conversation[]>()

  for (const c of convs) {
    if (c.pinned) {
      pinned.push(c)
      continue
    }
    // Trimmed, because a group name arrives from a prompt: " Billing" and
    // "Billing" are one shelf, not two.
    const group = c.group?.trim()
    if (group) {
      const list = groups.get(group)
      if (list) list.push(c)
      else groups.set(group, [c])
    } else ungrouped.push(c)
  }

  const sections: RailSection[] = []
  if (pinned.length > 0)
    sections.push({ key: 'pinned', label: 'Pinned', group: null, items: pinned.sort(railOrder) })
  for (const name of [...groups.keys()].sort(byName))
    sections.push({
      key: `group:${name}`,
      label: name,
      group: name,
      // Built above, so sorting in place mutates nothing the caller owns.
      items: groups.get(name)!.sort(railOrder),
    })
  if (ungrouped.length > 0)
    sections.push({ key: 'ungrouped', label: null, group: null, items: ungrouped.sort(railOrder) })
  return sections
}

/** Every group the viewer has filed something under — the "Move to group"
 *  submenu's contents, in the rail's own order. */
export function railGroups(convs: Conversation[]): string[] {
  const names = new Set<string>()
  for (const c of convs) {
    const group = c.group?.trim()
    if (group) names.add(group)
  }
  return [...names].sort(byName)
}

/** The free slot between two neighbours, or null when there is none to take.
 *  A null neighbour is a row with no position at all: above, that means
 *  nothing in the list is placed yet; below, it means the moved row lands at
 *  the end of the placed rows and ahead of the unplaced ones, which `+ STEP`
 *  says exactly. */
function slotBetween(before: number | null, after: number | null): number | null {
  if (before === null && after === null) return null
  if (before === null) return after! >= 1 ? Math.floor(after! / 2) : null
  if (after === null) return before + POSITION_STEP <= MAX_POSITION ? before + POSITION_STEP : null
  // `>= 2` is the whole condition: one integer has to fit strictly between.
  return after - before >= 2 ? before + Math.floor((after - before) / 2) : null
}

/** Renumber the whole run by tens, skipping rows already on their number. */
function renumber(ordered: Conversation[]): PositionWrite[] {
  const writes: PositionWrite[] = []
  ordered.forEach((c, i) => {
    // From STEP, not 0 — the first slot has to stay droppable-above.
    const position = (i + 1) * POSITION_STEP
    if ((c.position ?? null) !== position) writes.push({ id: c.id, position })
  })
  return writes
}

/** WHAT A DROP HAS TO WRITE. `fromId` was dragged onto `toId` inside one
 *  section; the answer is the position writes that leave the section reading
 *  the way it now looks.
 *
 *  Dragging DOWN lands after the row you dropped on, dragging UP lands before
 *  it — either way the dragged row takes the target's visible place, which is
 *  what the drop indicator drew.
 *
 *  ONE WRITE WHEN IT CAN, which is nearly always: a sparse list has a free
 *  integer between any two neighbours, so the drop PATCHes the moved row and
 *  nothing else. It renumbers only when there is no slot to take — the first
 *  drop in a list nobody has ordered yet (every position null), or a gap that
 *  earlier drops have halved away — and then returns a write per row that
 *  actually moves. An empty array means the drop changes nothing. */
export function reorderPosition(
  items: Conversation[],
  fromId: string,
  toId: string,
): PositionWrite[] {
  if (fromId === toId) return []
  const ordered = [...items].sort(railOrder)
  const fromIdx = ordered.findIndex((c) => c.id === fromId)
  const toIdx = ordered.findIndex((c) => c.id === toId)
  const moved = ordered[fromIdx]
  if (!moved || toIdx < 0) return []

  const rest = ordered.filter((c) => c.id !== fromId)
  const anchor = rest.findIndex((c) => c.id === toId)
  const at = fromIdx < toIdx ? anchor + 1 : anchor

  const slot = slotBetween(rest[at - 1]?.position ?? null, rest[at]?.position ?? null)
  if (slot !== null) return [{ id: fromId, position: slot }]
  return renumber([...rest.slice(0, at), moved, ...rest.slice(at)])
}

/** The row menu's LAYOUT half: pin, file, unfile. Built in TS so the rail's
 *  hover menu and any context menu a host wires are the same list of actions
 *  rather than two drifting copies (a context menu is a shortcut, never the
 *  only home of an action).
 *
 *  The thread's current group is left out of the submenu — "move it where it
 *  already is" is not an action — and "Remove from group" only appears on a
 *  row that is in one. */
export function layoutMenuItems(
  conv: Conversation,
  groups: string[],
  actions: {
    pin: () => void
    moveTo: (group: string) => void
    newGroup: () => void
    ungroup: () => void
  },
): ContextMenuEntry[] {
  const current = conv.group?.trim() || null
  const others = groups.filter((g) => g !== current)
  const items: ContextMenuEntry[] = [
    { label: conv.pinned ? 'Unpin' : 'Pin', onSelect: actions.pin },
    {
      label: 'Move to group',
      children: [
        ...others.map((g) => ({ label: g, onSelect: () => actions.moveTo(g) })),
        ...(others.length > 0 ? (['sep'] as ContextMenuEntry[]) : []),
        { label: 'New group', onSelect: actions.newGroup },
      ],
    },
  ]
  if (current) items.push({ label: 'Remove from group', onSelect: actions.ungroup })
  return items
}
