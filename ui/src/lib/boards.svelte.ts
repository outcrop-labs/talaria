import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { delJson, getJson, getJsonOr404, getList, patchJson, postJson, putJson } from '@/lib/fetch-json'
import type { Effort, Priority, Task, TaskActivity, TaskComment, TaskLink, TaskStatus } from '@/lib/task-const'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

/** Subscribe to a board's live event stream — multiplayer. On any event, refetch
 *  the board's tasks (and the open task) so all viewers stay in sync. */
export function useBoardLive(boardId: MaybeGetter<string | null>) {
  const qc = useQueryClient()
  $effect(() => {
    const id = resolve(boardId)
    if (!id) return
    const es = new EventSource(`/api/boards/${id}/events`)
    es.onmessage = () => {
      // Refresh the board (cards) live. We deliberately do NOT refetch an open
      // ticket here — that would thrash its editors mid-edit; the detail refetches
      // on the viewer's own actions.
      void qc.invalidateQueries({ queryKey: ['board-tasks', id] })
    }
    return () => es.close()
  })
}

export type BoardRole = 'owner' | 'editor' | 'viewer'
export interface Board {
  id: string
  name: string
  ownerId: string
  teamId: string | null
  teamName: string | null
  role: BoardRole
  judgeMode?: 'inherit' | 'off' | 'advisory' | 'enforcing'
  createdAt: string
  updatedAt: string
  archivedAt: string | null
}
export interface BoardMember {
  userId: string
  email: string | null
  name: string | null
  role: BoardRole
}

// Mutations live in fetch-json now. The local `j`/`post` pair below was that
// door re-implemented (it even documents the same evolution: it used to reject
// with `new Error('403')` until it learned to carry the server's sentence) —
// and it had drifted: `post` on its own resolves a failed Response as success,
// which `shareBoard` shipped to callers.

export function useBoards() {
  return createQuery(() => ({
    queryKey: ['boards'],
    // A 500 here is what convinced an owner his company's boards were deleted.
    // It must reach the surface as an error, not as an empty list.
    queryFn: (): Promise<Board[]> => getList<Board>('/api/boards', 'boards'),
  }))
}

/** Archived boards — for the "Archived" section on the boards index. */
export function useArchivedBoards() {
  return createQuery(() => ({
    queryKey: ['boards', 'archived'],
    queryFn: (): Promise<Board[]> => getList<Board>('/api/boards?archived=1', 'boards'),
  }))
}

export function useBoardTasks(boardId: MaybeGetter<string | null>, includeArchived: MaybeGetter<boolean> = false) {
  return createQuery(() => {
    const id = resolve(boardId)
    const archived = resolve(includeArchived)
    return {
      queryKey: ['board-tasks', id, archived],
      enabled: !!id,
      // Liveness comes from SSE (useBoardLive → invalidate). This is just a slow
      // safety-net in case the event stream drops; a fast poll would churn
      // re-renders behind an open ticket modal.
      refetchInterval: 30_000,
      queryFn: (): Promise<Task[]> =>
        getList<Task>(`/api/boards/${id}/tasks${archived ? '?archived=1' : ''}`, 'tasks'),
    }
  })
}

export interface BoardAgentConfig {
  allowAll: boolean
  models: string[]
}

/** The board's agent policy (restrictive by default: allowAll off, no models). */
export function useBoardAgents(boardId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(boardId)
    return {
      queryKey: ['board-agents', id],
      enabled: !!id,
      // The restrictive default belongs to a board that HAS no policy (a 200),
      // never to a request that failed — that would silently hide every agent.
      queryFn: (): Promise<BoardAgentConfig> => getJson<BoardAgentConfig>(`/api/boards/${id}/agents`),
    }
  })
}

export const setBoardAgents = (boardId: string, allowAll: boolean, models: string[]) =>
  putJson<{ ok: true }>(`/api/boards/${boardId}/agents`, { allowAll, models })

export interface BoardViewConfig {
  view?: 'board' | 'list' | 'gantt'
  group?: string
  q?: string
  status?: string
  assignee?: string
  priority?: string
  label?: string
  due?: string
}
export interface BoardView {
  id: string
  boardId: string
  name: string
  config: BoardViewConfig
  createdBy: string
  position: number
  createdAt: string
  updatedAt: string
}

export type LabelColor =
  | 'slate' | 'bronze' | 'green' | 'amber' | 'red' | 'blue' | 'purple' | 'teal'
  | 'pink' | 'orange' | 'lime' | 'cyan' | 'indigo' | 'magenta' | 'olive' | 'brown'
export interface BoardLabel {
  id: string
  boardId: string
  name: string
  color: LabelColor
  position: number
}

/** The board's label registry (first-class, colored, manageable). */
export function useBoardLabels(boardId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(boardId)
    return {
      queryKey: ['board-labels', id],
      enabled: !!id,
      queryFn: (): Promise<BoardLabel[]> => getList<BoardLabel>(`/api/boards/${id}/labels`, 'labels'),
    }
  })
}
export const createBoardLabel = (boardId: string, name: string, color?: LabelColor) =>
  postJson<{ label: BoardLabel }>(`/api/boards/${boardId}/labels`, { name, color })
export const updateBoardLabel = (boardId: string, labelId: string, patch: { name?: string; color?: LabelColor }) =>
  putJson<{ label: BoardLabel }>(`/api/boards/${boardId}/labels`, { labelId, ...patch })
export const deleteBoardLabel = (boardId: string, labelId: string) =>
  delJson<{ ok: true }>(`/api/boards/${boardId}/labels`, { labelId })

/** Saved views: named filter/layout presets shared with the board. */
export function useBoardViews(boardId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(boardId)
    return {
      queryKey: ['board-views', id],
      enabled: !!id,
      queryFn: (): Promise<BoardView[]> => getList<BoardView>(`/api/boards/${id}/views`, 'views'),
    }
  })
}
export const createBoardView = (boardId: string, name: string, config: BoardViewConfig) =>
  postJson<{ view: BoardView }>(`/api/boards/${boardId}/views`, { name, config })
export const updateBoardView = (boardId: string, viewId: string, patch: { name?: string; config?: BoardViewConfig }) =>
  putJson<{ view: BoardView }>(`/api/boards/${boardId}/views`, { viewId, ...patch })
export const deleteBoardView = (boardId: string, viewId: string) =>
  delJson<{ ok: true }>(`/api/boards/${boardId}/views`, { viewId })

export function useBoardMembers(boardId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(boardId)
    return {
      queryKey: ['board-members', id],
      enabled: !!id,
      queryFn: (): Promise<BoardMember[]> => getList<BoardMember>(`/api/boards/${id}/members`, 'members'),
    }
  })
}

export interface TaskUsage {
  promptTokens: number
  completionTokens: number
  cost: number
  unpricedTokens: number
  perModel: Array<{ llmModel: string | null; tokens: number; cost: number | null }>
}

export interface TaskFull {
  task: Task
  comments: TaskComment[]
  activity: TaskActivity[]
  watchers: string[]
  reviews: import('@/lib/task-const').QualityReview[]
  judgeReviews: JudgeReview[]
  blockedBy: TaskLink[]
  blocks: TaskLink[]
  usage: TaskUsage
}

export interface JudgeReview {
  id: string
  model: string | null
  verdict: 'pass' | 'revise' | 'escalate'
  summary: string
  issues: string[]
  createdAt: string
}

/** Warm the ticket cache from a card hover/press: by the time the modal's
 *  entrance finishes, the content is usually already there — the modal opens
 *  full, not skeleton-then-pop ("half-animated"). Same key+fn as useTask, so
 *  the modal's query resolves from cache. Fire-and-forget; staleTime keeps a
 *  hover from spamming the endpoint. */
export function prefetchTask(qc: import('@tanstack/svelte-query').QueryClient, taskId: string) {
  void qc.prefetchQuery({
    queryKey: ['task', taskId],
    queryFn: (): Promise<TaskFull | null> => getJsonOr404<TaskFull>(`/api/tasks/${taskId}`),
    staleTime: 15_000,
  })
}

export function useTask(taskId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(taskId)
    return {
      queryKey: ['task', id],
      enabled: !!id,
      // 404 is a real answer here: the ticket was deleted, or the URL is stale.
      // Anything else (403/500) is a failure and must surface as one.
      queryFn: (): Promise<TaskFull | null> => getJsonOr404<TaskFull>(`/api/tasks/${id}`),
    }
  })
}

// ── Actions ──────────────────────────────────────────────────────────────────
export const createBoard = (name: string, teamId?: string | null) => postJson<{ board: Board }>('/api/boards', { name, teamId })
/** Move a board between teams (null → personal). Owner only.
 *
 *  Resolves to `{ error }` rather than throwing, because the caller is a drag
 *  handler that shows the message in an alert. It used to swallow the status
 *  entirely — a 403 resolved to `{}`, the rail said nothing, and the board
 *  quietly snapped back to its old team on the next refetch. */
export const moveBoardToTeam = async (boardId: string, teamId: string | null): Promise<{ error?: string }> => {
  try {
    await patchJson(`/api/boards/${boardId}`, { teamId })
    return {}
  } catch (e) {
    return { error: e instanceof Error ? e.message : `The server refused the move.` }
  }
}
export const createTask = (
  boardId: string,
  input: {
    title: string
    description?: string
    priority?: Priority
    effort?: Effort | null
    assignees?: string[]
    dueDate?: string | null
    startDate?: string | null
    color?: string | null
    estimatedHours?: number | null
    parentId?: string | null
    tags?: string[]
  },
) => postJson<{ task: Task }>(`/api/boards/${boardId}/tasks`, input)

export const addComment = (taskId: string, content: string) => postJson<{ comment: TaskComment }>(`/api/tasks/${taskId}/comments`, { content })
export const deleteTask = (taskId: string) => delJson<{ ok: true }>(`/api/tasks/${taskId}`)

export const watchTask = (taskId: string, watcher: string) => postJson<{ ok: true }>(`/api/tasks/${taskId}/watchers`, { watcher })
export const unwatchTask = (taskId: string, watcher: string) => delJson<{ ok: true }>(`/api/tasks/${taskId}/watchers`, { watcher })
export const reviewTask = (taskId: string, status: 'approved' | 'rejected', notes?: string) =>
  postJson<{ ok: true }>(`/api/tasks/${taskId}/review`, { status, notes })

export const addDependency = (taskId: string, dependsOnId: string) =>
  postJson<{ ok: true }>(`/api/tasks/${taskId}/dependencies`, { dependsOnId })
export const removeDependency = (taskId: string, dependsOnId: string) =>
  delJson<{ ok: true }>(`/api/tasks/${taskId}/dependencies`, { dependsOnId })

export const updateTask = (
  taskId: string,
  patch: {
    title?: string
    description?: string | null
    status?: TaskStatus
    priority?: Priority
    effort?: Effort | null
    assignees?: string[]
    dueDate?: string | null
    startDate?: string | null
    color?: string | null
    tags?: string[]
    outcome?: string | null
    resolution?: string | null
    errorMessage?: string | null
    archived?: boolean
    estimatedHours?: number | null
    parentId?: string | null
    addTimeSpentSeconds?: number
    attachmentIds?: string[]
    refs?: Array<{ type: 'kb-doc' | 'artifact'; id: string }>
  },
) => putJson<{ task: Task }>(`/api/tasks/${taskId}`, patch)

export const renameBoard = (boardId: string, name: string) => patchJson<{ board: Board }>(`/api/boards/${boardId}`, { name })
export const setBoardJudgeMode = (boardId: string, judgeMode: 'inherit' | 'off' | 'advisory' | 'enforcing') =>
  patchJson<{ board: Board }>(`/api/boards/${boardId}`, { judgeMode })
export const archiveBoard = (boardId: string, archived: boolean) =>
  patchJson<{ board: Board }>(`/api/boards/${boardId}`, { archived })
export const deleteBoard = (boardId: string) => delJson<{ ok: true }>(`/api/boards/${boardId}`)
export const archiveTask = (taskId: string, archived: boolean) => updateTask(taskId, { archived })
export const shareBoard = (boardId: string, email: string, role: 'editor' | 'viewer') =>
  postJson<{ ok: true }>(`/api/boards/${boardId}/members`, { email, role })
export const unshareBoard = (boardId: string, userId: string) =>
  delJson<{ ok: true }>(`/api/boards/${boardId}/members`, { userId })
