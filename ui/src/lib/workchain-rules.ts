// Workchain rules (TALA-30 linear; TALA-35 the graph): the pure shapes every
// lens of the workchains view needs, kept here so the components stay markup
// and the rules stay testable in plain TS. The wire types and the
// query/mutation client live in workchain-client.ts (svelte-query runtime —
// not importable from node).
import type { Effort, TaskStatus } from '@/lib/task-const'

/** The derived step state — assigned by the api from the chain's graph read
 *  (api/crates/talaria-workchains): terminal steps are 'done', a step with
 *  no predecessors is 'head', one whose predecessors are ALL satisfied is
 *  'ready' (only branches produce it — the fan-out made it this step's
 *  turn), the rest are 'blocked' (the v1 wire said 'waiting'), and an
 *  archived ticket keeps 'archived' (the chain reads past it). */
export type WorkchainState = 'done' | 'head' | 'ready' | 'blocked' | 'archived'

export interface WorkchainStep {
  taskId: string
  position: number
  state: WorkchainState
  ticketRef: string | null
  title: string
  assignees: string[]
  effort: Effort | null
  dueDate: string | null
  status: TaskStatus
  archived: boolean
  /** Free canvas placement (TALA-35), null on both = auto-layout. */
  x: number | null
  y: number | null
}

export interface WorkchainEdge {
  fromTaskId: string
  toTaskId: string
}

export interface Workchain {
  id: string
  boardId: string
  name: string
  createdBy: string | null
  paused: boolean
  position: number
  createdAt: string
  updatedAt: string
  steps: WorkchainStep[]
  /** The graph: wires as task-id pairs, from → to (TALA-35). */
  edges: WorkchainEdge[]
}

/** The contiguous 0-based `positions` payload for a full reorder — the
 *  order the caller shows, renumbered, gaps squeezed out. */
export function buildPositions(taskIds: string[]): Array<{ taskId: string; position: number }> {
  return taskIds.map((taskId, position) => ({ taskId, position }))
}

/** `\"done/total\"` for a chain's header — the glanceable progress read.
 *  Terminal steps count as done; the head and ready steps do not (they are
 *  the work in flight); archived steps count in the total only — they are
 *  structure the chain reads past, not progress. */
export function chainProgress(w: Pick<Workchain, 'steps'>): string {
  const total = w.steps.length
  const done = w.steps.filter((s) => s.state === 'done').length
  return `${done}/${total}`
}

/** Every task id living in ANY chain — archived steps' tasks included (they
 *  are still chained; the unique index says so). Pickers exclude these, and
 *  the Unchained section is the filtered tasks without them. */
export function chainedTaskIds(workchains: Array<Pick<Workchain, 'steps'>>): Set<string> {
  const ids = new Set<string>()
  for (const w of workchains) for (const s of w.steps) ids.add(s.taskId)
  return ids
}

/** The task's new position order after moving it `delta` slots (−1 earlier,
 *  +1 later) — or null when the move would leave the chain. Pure: the caller
 *  ships the result through `updateWorkchain`'s `positions`. */
export function moveStepOrder(taskIds: string[], taskId: string, delta: -1 | 1): string[] | null {
  const i = taskIds.indexOf(taskId)
  const j = i + delta
  if (i === -1 || j < 0 || j >= taskIds.length) return null
  // Bounds-checked above; noUncheckedIndexedAccess just can't see it.
  const a = taskIds[i] as string
  const b = taskIds[j] as string
  return taskIds.map((id, k) => (k === i ? b : k === j ? a : id))
}

/** A ticket the "+ Add ticket" picker may offer — the board's UNCHAINED
 *  tasks, reshaped by the caller (Workchains) from its filtered tasks. */
export interface WorkchainCandidate {
  id: string
  ticketRef: string | null
  title: string
  effort?: Effort | null
}

/** The "+ Add ticket" picker's rows — the candidates whose ticketRef or
 *  title matches the draft, case-insensitively; a blank draft shows every
 *  candidate. The caller's list is already unchained-only — the filter is
 *  the only pure shape here. */
export function filterCandidates(
  candidates: WorkchainCandidate[],
  draft: string,
): WorkchainCandidate[] {
  const q = draft.trim().toLowerCase()
  if (!q) return candidates
  return candidates.filter(
    (c) => c.title.toLowerCase().includes(q) || (c.ticketRef ?? '').toLowerCase().includes(q),
  )
}

/** The chain a focus-holding view should show: the focused id while it
 *  still names a chain in the list, else the FIRST chain in the list's
 *  own order — null only when the list is empty. Null focus, an unknown
 *  id, and the stale id a delete leaves behind all land on the first
 *  chain: a view with chains never sits focused on nothing. */
export function pickFocusedChain(
  chains: Array<Pick<Workchain, 'id'>>,
  focusedId: string | null,
): string | null {
  const first = chains[0]
  if (!first) return null // empty list: nothing to focus
  if (focusedId !== null && chains.some((w) => w.id === focusedId)) return focusedId
  return first.id // null, unknown, or stale focus: the first chain in list order
}

// ── TALA-35: the canvas ──────────────────────────────────────────────────────

/** A chain BRANCHES when any step has two or more outgoing wires — the
 *  condition that makes the canvas (not the rails) the honest render. A
 *  straight line renders either way; the lens picks rails for it. */
export function chainBranches(w: Pick<Workchain, 'edges'>): boolean {
  const out = new Map<string, number>()
  for (const e of w.edges) out.set(e.fromTaskId, (out.get(e.fromTaskId) ?? 0) + 1)
  for (const n of out.values()) if (n > 1) return true
  return false
}

/** Successor list per task — the adjacency the canvas and the derive walk. */
export function successorsOf(edges: WorkchainEdge[]): Map<string, string[]> {
  const m = new Map<string, string[]>()
  for (const e of edges) {
    const list = m.get(e.fromTaskId) ?? []
    list.push(e.toTaskId)
    m.set(e.fromTaskId, list)
  }
  return m
}

/** Would adding from → to close a cycle? Walk `to`'s successors looking
 *  for `from`: the wire runs backwards iff the target already REACHES the
 *  source — the same direction the Rust `would_cycle` walks ("can to_task
 *  already REACH from_task"). The api re-derives this on write (the 400 is
 *  its answer); the canvas pre-asks the same question so the drag feedback
 *  says NO before the request does. */
export function wouldCycle(edges: WorkchainEdge[], from: string, to: string): boolean {
  if (from === to) return true
  const succ = successorsOf(edges)
  const seen = new Set<string>([to])
  const queue = [to]
  while (queue.length) {
    const node = queue.pop() as string
    for (const next of succ.get(node) ?? []) {
      if (next === from) return true
      if (!seen.has(next)) {
        seen.add(next)
        queue.push(next)
      }
    }
  }
  return false
}

/** Auto-layout for steps the user has never placed (x/y both null): levels
 *  by longest-path-from-an-entry (the classic layered DAG walk), x fixed
 *  per column, y stacked within a column. Returns a position per taskId;
 *  placed steps keep their saved spot. NODE_W/NODE_H mirror the card
 *  box so wires land on real corners. */
export const NODE_W = 208
export const NODE_H = 104
export const COL_GAP = 64
export const ROW_GAP = 24

export function autoLayout(steps: Array<Pick<WorkchainStep, 'taskId' | 'x' | 'y' | 'position'>>, edges: WorkchainEdge[]): Map<string, { x: number; y: number }> {
  const placed = new Map<string, { x: number; y: number }>()
  for (const s of steps) if (s.x !== null && s.y !== null) placed.set(s.taskId, { x: s.x, y: s.y })
  const need = steps.filter((s) => !placed.has(s.taskId))
  if (need.length === 0) return placed
  // Level = longest edge-distance from any no-pred node (entry points).
  const preds = new Map<string, string[]>()
  for (const e of edges) {
    const list = preds.get(e.toTaskId) ?? []
    list.push(e.fromTaskId)
    preds.set(e.toTaskId, list)
  }
  const ids = new Set(need.map((s) => s.taskId))
  const level = new Map<string, number>()
  const visit = (id: string, stack: Set<string>): number => {
    if (level.has(id)) return level.get(id) as number
    if (stack.has(id)) return 0 // cycle: the api refuses writes like this; derive defensively anyway
    stack.add(id)
    // All predecessors count, placed or not: a placed one is a satisfied
    // earlier column (contributes 1); an unplaced one contributes 1 + its
    // own level. Edges naming ids outside the chain stay ignored, as before.
    const ps = (preds.get(id) ?? []).filter((p) => ids.has(p) || placed.has(p))
    const l = ps.length === 0 ? 0 : 1 + Math.max(...ps.map((p) => (placed.has(p) ? 0 : visit(p, stack))))
    stack.delete(id)
    level.set(id, l)
    return l
  }
  for (const id of ids) visit(id, new Set())
  // Column x: level * (node + gap). Rows y: stack within a column, in the
  // chain's own read order (position) so the layout is stable across reads.
  const byLevel = new Map<number, string[]>()
  for (const s of [...need].sort((a, b) => a.position - b.position)) {
    const l = level.get(s.taskId) ?? 0
    const list = byLevel.get(l) ?? []
    list.push(s.taskId)
    byLevel.set(l, list)
  }
  for (const [l, idsInLevel] of [...byLevel.entries()].sort((a, b) => a[0] - b[0])) {
    idsInLevel.forEach((id, row) => {
      placed.set(id, { x: l * (NODE_W + COL_GAP), y: row * (NODE_H + ROW_GAP) })
    })
  }
  return placed
}

/** The wire's svg path — a cubic bezier leaving `from`'s right edge and
 *  arriving at `to`'s left edge. Horizontal control points keep the curve
 *  reading as flow (left to right), the n8n look. Straight when the nodes
 *  already line up. */
export function wirePath(
  from: { x: number; y: number },
  to: { x: number; y: number },
): string {
  const x1 = from.x + NODE_W
  const y1 = from.y + NODE_H / 2
  const x2 = to.x
  const y2 = to.y + NODE_H / 2
  const dx = Math.max(40, Math.abs(x2 - x1) / 2)
  return `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`
}

/** The wire's state off its endpoints — idle (both live), fired (the source
 *  finished, the handoff rode it), done (both finished). A wire whose
 *  source is done 'lights up': the fan-out visibly moved along it. */
export type WireState = 'idle' | 'fired' | 'done'
export function wireState(from: WorkchainStep, to: WorkchainStep): WireState {
  const fromDone = from.state === 'done' || from.state === 'archived'
  const toDone = to.state === 'done' || to.state === 'archived'
  if (toDone) return 'done'
  if (fromDone) return 'fired'
  return 'idle'
}
