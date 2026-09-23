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

/** Find the exact edge (or undefined). Direction matters: the wire is the
 *  pair, not the unordered join. */
export function findEdge(
  edges: WorkchainEdge[],
  fromTaskId: string,
  toTaskId: string,
): WorkchainEdge | undefined {
  return edges.find((e) => e.fromTaskId === fromTaskId && e.toTaskId === toTaskId)
}

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
