// Workchain rules (TALA-30): the pure shapes every lens of the workchains
// view needs, kept here so the components stay markup and the rules stay
// testable in plain TS. The wire types and the query/mutation client live in
// workchain-client.ts (svelte-query runtime — not importable from node).
import type { Effort, TaskStatus } from '@/lib/task-const'

/** The derived step state — assigned by the api from the chain's read
 *  (api/src/workchains.rs): the first non-archived step whose task is not
 *  terminal is 'head', terminal steps are 'done', the rest wait, and an
 *  archived ticket keeps 'archived' (the chain reads past it). */
export type WorkchainState = 'done' | 'head' | 'waiting' | 'archived'

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
}

/** The contiguous 0-based `positions` payload for a full reorder — the
 *  order the caller shows, renumbered, gaps squeezed out. */
export function buildPositions(taskIds: string[]): Array<{ taskId: string; position: number }> {
  return taskIds.map((taskId, position) => ({ taskId, position }))
}

/** `"done/total"` for a chain's header — the glanceable progress read.
 *  Terminal steps count as done; the head never does (it is the work in
 *  flight); archived steps count in the total only — they are structure the
 *  chain reads past, not progress. */
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
