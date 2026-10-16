// The draft-proposal shape shared by PlanModal.svelte and ProposalCard.svelte —
// and the one place the batch's SHAPE is read: which proposals are epics, what
// hangs under what, and the order creation has to walk for the hierarchy to
// land. Pure functions, so the rules are pinned by plan-modal.test.ts rather
// than by driving a modal.
import { isHumanAssignee, type Effort, type Priority } from '@/lib/task-const'

export interface Proposal {
  title: string
  description: string
  priority: Priority
  effort: Effort | null
  /** Indices of proposals in this batch that must land first. */
  dependsOn: number[]
  /** Routing labels from the workflow map — trip dispatch classification. */
  tags: string[]
  /** The index IN THIS BATCH of the proposal that is this one's epic; null for
   *  a top-level ticket. There is no separate "is an epic" flag on the wire,
   *  deliberately: a proposal that other proposals name as parent IS the epic,
   *  so the badge, the creation order and the parentId all read this one field
   *  and cannot drift apart. */
  parent: number | null
  /** Board member user ids the drafter picked out of the discussion — ids,
   *  never names, because creation assigns by id. Empty when the conversation
   *  named nobody. */
  assignees: string[]
  /** The id of an OPEN ticket on the board this proposal re-proposes. A
   *  duplicate is a question for the reviewer, never something to drop
   *  quietly: it arrives unticked with the ticket it echoes named on its card,
   *  and the include checkbox is how the reviewer keeps it anyway. */
  duplicateOf: string | null
  include: boolean
}

/** The indices some INCLUDED proposal names as its parent — this batch's
 *  epics. A proposal named only by drafts the reviewer dropped is not an epic:
 *  nothing is left to hang under it. */
const epicsOf = (batch: Proposal[]): Set<number> => {
  const epics = new Set<number>()
  batch.forEach((p, i) => {
    if (!p.include || p.parent === null || p.parent === i) return
    if (batch[p.parent]) epics.add(p.parent)
  })
  return epics
}

/** Does anything still in this batch hang under proposal `i`? The card's
 *  Epic badge. */
export function isEpic(batch: Proposal[], i: number): boolean {
  return epicsOf(batch).has(i)
}

/** The epic proposal `i` will really be created under, or null for a top-level
 *  ticket. A named parent evaporates three ways, and each one files the ticket
 *  top-level rather than refusing it:
 *    - the proposal is itself an epic — tickets nest ONE level, so an epic
 *      ignores a parent of its own instead of building a third tier;
 *    - the reviewer dropped the parent — there is nothing left to hang under;
 *    - the index points at nothing, or at itself — a drafter's slip. */
export function parentIndex(batch: Proposal[], i: number): number | null {
  const p = batch[i]
  if (!p || p.parent === null || p.parent === i) return null
  if (!batch[p.parent]?.include) return null
  if (epicsOf(batch).has(i)) return null
  return p.parent
}

/** The order creation walks: epics first, then the children hanging under
 *  them, and dropped proposals not at all. A child is created WITH its
 *  parentId, so the ticket it names has to exist by the time its turn comes —
 *  and that must not depend on the order the drafter happened to emit. */
export function createOrder(batch: Proposal[]): number[] {
  const live = batch.map((_, i) => i).filter((i) => batch[i]?.include)
  return [
    ...live.filter((i) => parentIndex(batch, i) === null),
    ...live.filter((i) => parentIndex(batch, i) !== null),
  ]
}

/** A drafted assignee as a TICKET spells it. The drafter answers with bare
 *  board-member user ids — that is the vocabulary it was shown and the one its
 *  own verify checks — while a ticket spells a person `user:<id>` and reads a
 *  bare string as an agent MODEL id: a board that does not allow every agent
 *  refuses the create outright, and one that does files the ticket under a
 *  model nobody has. An id that already carries the prefix passes through, so
 *  the two spellings cannot stack. */
export function ticketAssignee(a: string): string {
  return isHumanAssignee(a) ? a : `user:${a}`
}
