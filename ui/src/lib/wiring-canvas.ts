// The wiring editor's graph decisions (TALA-34), pure and testable: what a
// drop means, which steps are legal targets mid-drag, and how a wire is
// found for selection/deletion. The DOM half (pointer capture, the overlay
// element, the highlight classes) lives in WorkchainCanvas.svelte; the pure
// geometry lives in wiring-overlay.ts. Keep this file DOM-free so the node
// vitest suite can import it (the repo's client-module convention).
import { wouldCycle } from '@/lib/workchain-rules'
import type { WorkchainEdge, WorkchainStep } from '@/lib/workchain-rules'

/** The fields of a step the wiring editor reads — WorkchainStep minus the
 *  display extras, so tests can build minimal stand-ins. */
export type WiringStepLike = Pick<WorkchainStep, 'taskId' | 'state'>

/** Distance from the pointer to a wire for it to count as a click/select —
 *  tighter than a port's touch target: wires run close together, ports don't. */
export const WIRE_HIT_RADIUS = 8

/** The dataTransfer type a ticket dragged out of the workchain sidebar carries.
 *  A TYPED payload, not `text/plain`: the canvas must be able to tell "a ticket
 *  is being dragged in" from any other drag crossing it, and `types` is all
 *  dragover is allowed to see (the data itself is only readable on drop). The
 *  Gantt chart's unscheduled list sets the precedent with `text/gantt-task`. */
export const SIDEBAR_TICKET_MIME = 'text/workchain-task'

/** The steps a drag from `fromTaskId` may legally land on: not the source,
 *  not a step already wired FROM the source (the api 400s a duplicate — the
 *  canvas refuses the gesture instead), not one that would close a cycle. */
export function edgeCandidateSteps(
  steps: WiringStepLike[],
  edges: WorkchainEdge[],
  fromTaskId: string,
): WiringStepLike[] {
  const wired = new Set(edges.filter((e) => e.fromTaskId === fromTaskId).map((e) => e.toTaskId))
  return steps.filter(
    (s) => s.taskId !== fromTaskId && !wired.has(s.taskId) && !wouldCycle(edges, fromTaskId, s.taskId),
  )
}

/** One write against the chain's graph. The canvas plans a gesture as a list
 *  of these and then executes it, which is what makes both the plan and its
 *  INVERSE testable without a server: `add` is POST /edges, `cut` is DELETE. */
export interface WireOp {
  kind: 'add' | 'cut'
  fromTaskId: string
  toTaskId: string
}

/** Dropping a ticket ON a wire splices it into that connection: A → new → B,
 *  and the A → B the reader dropped onto goes. The api has a `after:` wedge
 *  that looks similar and is NOT this — it moves every one of the anchor's
 *  successors onto the new step, which is right for "insert into a line" and
 *  wrong for "insert into this one wire". Three explicit edges say exactly
 *  what the gesture meant.
 *
 *  Order matters: the two adds land before the cut, so a failure part-way
 *  leaves the reader with a graph that has MORE structure than they drew,
 *  never a chain silently severed in the middle. */
export function spliceIntoWire(
  edge: { fromTaskId: string; toTaskId: string },
  taskId: string,
): WireOp[] {
  return [
    { kind: 'add', fromTaskId: edge.fromTaskId, toTaskId: taskId },
    { kind: 'add', fromTaskId: taskId, toTaskId: edge.toTaskId },
    { kind: 'cut', fromTaskId: edge.fromTaskId, toTaskId: edge.toTaskId },
  ]
}

/** The undo of a list of wire ops: every add becomes a cut and back, applied
 *  in REVERSE order so the graph passes back through the same intermediate
 *  states it came forward through. */
export function invertWireOps(ops: readonly WireOp[]): WireOp[] {
  return [...ops]
    .reverse()
    .map((op) => ({ ...op, kind: op.kind === 'add' ? ('cut' as const) : ('add' as const) }))
}

/** What a drop resolves to. The component ships `create` to the api, and
 *  quietly ends the drag on `cancel` (a mis-drop costs nothing). */
export type EdgeDropOutcome =
  | { kind: 'create'; toTaskId: string }
  | { kind: 'cancel' }
  | { kind: 'cycle' }

/** Resolve a drop. `hit` is the pointer's target: a port (side set), a card
 *  (side null), or nothing (null) — a miss cancels, and the drop-anywhere-else
 *  rule is just that. The caller carries the empty-canvas composer itself. */
export function edgeDropOutcome(
  steps: WiringStepLike[],
  edges: WorkchainEdge[],
  fromTaskId: string,
  hit: { taskId: string; side: 'in' | 'out' | null } | null,
): EdgeDropOutcome {
  if (!hit) return { kind: 'cancel' }
  if (hit.side === 'out') return { kind: 'cancel' }
  if (hit.taskId === fromTaskId) return { kind: 'cancel' }
  const target = steps.find((s) => s.taskId === hit.taskId)
  if (!target) return { kind: 'cancel' }
  if (edges.some((e) => e.fromTaskId === fromTaskId && e.toTaskId === hit.taskId))
    return { kind: 'cancel' }
  if (wouldCycle(edges, fromTaskId, hit.taskId)) return { kind: 'cycle' }
  return { kind: 'create', toTaskId: hit.taskId }
}
