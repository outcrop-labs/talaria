import { useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import type { Effort, Priority, Task, TaskActivity, TaskComment, TaskLink, TaskStatus } from '@/lib/task-const'

/** Subscribe to a board's live event stream — multiplayer. On any event, refetch
 *  the board's tasks (and the open task) so all viewers stay in sync. */
export function useBoardLive(boardId: string | null) {
  const qc = useQueryClient()
  useEffect(() => {
    if (!boardId) return
    const es = new EventSource(`/api/boards/${boardId}/events`)
    es.onmessage = () => {
      // Refresh the board (cards) live. We deliberately do NOT refetch an open
      // ticket here — that would thrash its editors mid-edit; the detail refetches
      // on the viewer's own actions.
      void qc.invalidateQueries({ queryKey: ['board-tasks', boardId] })
    }
    return () => es.close()
  }, [boardId, qc])
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

const j = async (r: Response) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status))))
const post = (url: string, body: unknown) =>
  fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, credentials: 'same-origin', body: JSON.stringify(body) })

export function useBoards() {
  return useQuery({
    queryKey: ['boards'],
    queryFn: async (): Promise<Board[]> => {
      const r = await fetch('/api/boards', { credentials: 'same-origin' })
      if (!r.ok) return []
      return (await r.json()).boards
    },
  })
}

/** Archived boards — for the "Archived" section on the boards index. */
export function useArchivedBoards() {
  return useQuery({
    queryKey: ['boards', 'archived'],
    queryFn: async (): Promise<Board[]> => {
      const r = await fetch('/api/boards?archived=1', { credentials: 'same-origin' })
      if (!r.ok) return []
      return (await r.json()).boards
    },
  })
}

export function useBoardTasks(boardId: string | null, includeArchived = false) {
  return useQuery({
    queryKey: ['board-tasks', boardId, includeArchived],
    enabled: !!boardId,
    // Liveness comes from SSE (useBoardLive → invalidate). This is just a slow
    // safety-net in case the event stream drops; a fast poll would churn
    // re-renders behind an open ticket modal.
    refetchInterval: 30_000,
    queryFn: async (): Promise<Task[]> => {
      const r = await fetch(`/api/boards/${boardId}/tasks${includeArchived ? '?archived=1' : ''}`, { credentials: 'same-origin' })
      if (!r.ok) return []
      return (await r.json()).tasks
    },
  })
}

export interface BoardAgentConfig {
  allowAll: boolean
  models: string[]
}

/** The board's agent policy (restrictive by default: allowAll off, no models). */
export function useBoardAgents(boardId: string | null) {
  return useQuery({
    queryKey: ['board-agents', boardId],
    enabled: !!boardId,
    queryFn: async (): Promise<BoardAgentConfig> => {
      const r = await fetch(`/api/boards/${boardId}/agents`, { credentials: 'same-origin' })
      if (!r.ok) return { allowAll: false, models: [] }
      return r.json()
    },
  })
}

export const setBoardAgents = (boardId: string, allowAll: boolean, models: string[]) =>
  fetch(`/api/boards/${boardId}/agents`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ allowAll, models }),
  }).then(j)

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

/** Saved views: named filter/layout presets shared with the board. */
export function useBoardViews(boardId: string | null) {
  return useQuery({
    queryKey: ['board-views', boardId],
    enabled: !!boardId,
    queryFn: async (): Promise<BoardView[]> => {
      const r = await fetch(`/api/boards/${boardId}/views`, { credentials: 'same-origin' })
      if (!r.ok) return []
      return (await r.json()).views
    },
  })
}
export const createBoardView = (boardId: string, name: string, config: BoardViewConfig) =>
  post(`/api/boards/${boardId}/views`, { name, config }).then(j)
export const updateBoardView = (boardId: string, viewId: string, patch: { name?: string; config?: BoardViewConfig }) =>
  fetch(`/api/boards/${boardId}/views`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ viewId, ...patch }),
  }).then(j)
export const deleteBoardView = (boardId: string, viewId: string) =>
  fetch(`/api/boards/${boardId}/views`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ viewId }),
  }).then(j)

export function useBoardMembers(boardId: string | null) {
  return useQuery({
    queryKey: ['board-members', boardId],
    enabled: !!boardId,
    queryFn: async (): Promise<BoardMember[]> => {
      const r = await fetch(`/api/boards/${boardId}/members`, { credentials: 'same-origin' })
      if (!r.ok) return []
      return (await r.json()).members
    },
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

export function useTask(taskId: string | null) {
  return useQuery({
    queryKey: ['task', taskId],
    enabled: !!taskId,
    queryFn: async (): Promise<TaskFull | null> => {
      const r = await fetch(`/api/tasks/${taskId}`, { credentials: 'same-origin' })
      if (!r.ok) return null
      return r.json()
    },
  })
}

// ── Actions ──────────────────────────────────────────────────────────────────
export const createBoard = (name: string, teamId?: string | null) => post('/api/boards', { name, teamId }).then(j)
/** Move a board between teams (null → personal). Owner only. */
export const moveBoardToTeam = (boardId: string, teamId: string | null) =>
  fetch(`/api/boards/${boardId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ teamId }),
  }).then((r) => r.json().catch(() => ({})))
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
    estimatedHours?: number | null
    parentId?: string | null
  },
) => post(`/api/boards/${boardId}/tasks`, input).then(j)

export const addComment = (taskId: string, content: string) => post(`/api/tasks/${taskId}/comments`, { content }).then(j)
export const deleteTask = (taskId: string) =>
  fetch(`/api/tasks/${taskId}`, { method: 'DELETE', credentials: 'same-origin' })

export const watchTask = (taskId: string, watcher: string) => post(`/api/tasks/${taskId}/watchers`, { watcher })
export const unwatchTask = (taskId: string, watcher: string) =>
  fetch(`/api/tasks/${taskId}/watchers`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ watcher }),
  })
export const reviewTask = (taskId: string, status: 'approved' | 'rejected', notes?: string) =>
  post(`/api/tasks/${taskId}/review`, { status, notes }).then(j)

export const addDependency = (taskId: string, dependsOnId: string) =>
  post(`/api/tasks/${taskId}/dependencies`, { dependsOnId })
export const removeDependency = (taskId: string, dependsOnId: string) =>
  fetch(`/api/tasks/${taskId}/dependencies`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ dependsOnId }),
  })

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
) =>
  fetch(`/api/tasks/${taskId}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify(patch),
  }).then(j)

export const renameBoard = (boardId: string, name: string) =>
  fetch(`/api/boards/${boardId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ name }),
  })
export const setBoardJudgeMode = (boardId: string, judgeMode: 'inherit' | 'off' | 'advisory' | 'enforcing') =>
  fetch(`/api/boards/${boardId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ judgeMode }),
  })
export const archiveBoard = (boardId: string, archived: boolean) =>
  fetch(`/api/boards/${boardId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ archived }),
  })
export const deleteBoard = (boardId: string) =>
  fetch(`/api/boards/${boardId}`, { method: 'DELETE', credentials: 'same-origin' })
export const archiveTask = (taskId: string, archived: boolean) => updateTask(taskId, { archived })
export const shareBoard = (boardId: string, email: string, role: 'editor' | 'viewer') =>
  post(`/api/boards/${boardId}/members`, { email, role })
export const unshareBoard = (boardId: string, userId: string) =>
  fetch(`/api/boards/${boardId}/members`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ userId }),
  })
