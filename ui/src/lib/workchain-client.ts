// Workchain client (TALA-30): the HTTP half of the workchains lens — the
// query, the mutations, the wire types. Reads through the one HTTP door;
// writes invalidate ['board-workchains', boardId] at the call site (the lens
// owns the query key). Every workchain write the api makes bumps the board's
// SSE event, and useBoardLive invalidates this same key on receipt — so the
// rails stay live for everyone.
//
// This file imports the svelte-query runtime, so it stays OUT of the node
// vitest suite (unknown .svelte extension in node's module loader). The pure
// rules the view derives from live in workchain-rules.ts with the tests.
import { resolve, type MaybeGetter } from '@/lib/reactive-arg'
import { createQuery } from '@tanstack/svelte-query'
import { delJson, getList, patchJson, postJson } from '@/lib/fetch-json'
import type { Effort, TaskStatus } from '@/lib/task-const'

/** A reactive argument: pass a plain value, or a getter for values that
 *  change over a component's life (route params). */

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
  /** Free canvas placement (TALA-35), null on both = the view lays out. */
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

/** The board's workchains with their steps and derived states. Any member. */
export function useBoardWorkchains(boardId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(boardId)
    return {
      queryKey: ['board-workchains', id],
      enabled: !!id,
      queryFn: (): Promise<Workchain[]> => getList<Workchain>(`/api/boards/${id}/workchains`, 'workchains'),
    }
  })
}

/** Create a chain, positioned after the last one. Owner/editor. */
export const createWorkchain = (boardId: string, name: string) =>
  postJson<{ workchain: Workchain }>(`/api/boards/${boardId}/workchains`, { name })

/** Rename, pause/unpause, reorder (a linear rebuild over edges), or move
 *  canvas nodes. `positions` is the FULL intended order; every taskId must
 *  already be a step of this chain. `nodes` moves steps on the canvas
 *  (TALA-35) — a node may carry x, y, or both. */
export const updateWorkchain = (
  id: string,
  patch: {
    name?: string
    paused?: boolean
    positions?: Array<{ taskId: string; position: number }>
    nodes?: Array<{ taskId: string; x?: number; y?: number }>
  },
) => patchJson<{ ok: true }>(`/api/workchains/${id}`, patch)

/** Append a step (or wedge it after the named step). 409 when the ticket is
 *  already in ANY chain — one chain per task is the v1 invariant. */
export const addWorkchainStep = (id: string, taskId: string, after?: string) =>
  postJson<{ ok: true }>(`/api/workchains/${id}/steps`, { taskId, ...(after ? { after } : {}) })

/** Remove a step without touching the task. A miss is a quiet ok. */
export const removeWorkchainStep = (id: string, taskId: string) =>
  delJson<{ ok: true }>(`/api/workchains/${id}/steps/${taskId}`)

/** Delete the chain. Its tickets are unlinked, never deleted. */
export const deleteWorkchain = (id: string) => delJson<{ ok: true }>(`/api/workchains/${id}`)

/** Draw a wire (an edge) between two steps of the chain — the canvas's
 *  connect verb's write half (the drag interaction itself is TALA-34). 400
 *  when an endpoint is not a step, or when the wire would close a cycle. */
export const addWorkchainEdge = (id: string, fromTaskId: string, toTaskId: string) =>
  postJson<{ ok: true }>(`/api/workchains/${id}/edges`, { fromTaskId, toTaskId })

/** Cut a wire. A miss is a quiet ok. */
export const removeWorkchainEdge = (id: string, fromTaskId: string, toTaskId: string) =>
  delJson<{ ok: true }>(`/api/workchains/${id}/edges/${fromTaskId}/${toTaskId}`)
