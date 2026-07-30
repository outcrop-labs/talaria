// Client view of a board's status set — the workflow columns. Server truth
// via /api/boards/:id/statuses (defaults + the system Blocked column when the
// board never customized). Helpers keep label/color lookups safe for custom
// keys and legacy constants alike.
import { useQuery } from '@tanstack/react-query'
import { STATUS_LABEL } from './task-const'
import { LABEL_CSS } from '@/components/board/field-pills'

export type StatusCategory = 'open' | 'active' | 'review' | 'done' | 'blocked'

export interface BoardStatus {
  id: string | null
  key: string
  label: string
  color: string
  category: StatusCategory
  agentStart: boolean
  position: number
  system?: boolean
}

export function useBoardStatuses(boardId: string | null) {
  return useQuery({
    queryKey: ['board-statuses', boardId],
    enabled: !!boardId,
    queryFn: async (): Promise<BoardStatus[]> => {
      const r = await fetch(`/api/boards/${boardId}/statuses`, { credentials: 'same-origin' })
      if (!r.ok) return []
      return (await r.json()).statuses
    },
  })
}

export const statusLabelOf = (key: string, statuses: BoardStatus[]): string =>
  statuses.find((s) => s.key === key)?.label ?? STATUS_LABEL[key as keyof typeof STATUS_LABEL] ?? key

export const statusColorOf = (key: string, statuses: BoardStatus[]): string => {
  const c = statuses.find((s) => s.key === key)?.color
  if (c) return LABEL_CSS[c as keyof typeof LABEL_CSS] ?? 'var(--theme-muted)'
  const FALLBACK: Record<string, string> = {
    inbox: 'var(--theme-muted)',
    assigned: 'var(--theme-accent)',
    in_progress: 'var(--theme-warning)',
    blocked: 'var(--theme-danger)',
    quality_review: 'var(--theme-accent-secondary)',
    done: 'var(--theme-success)',
    failed: 'var(--theme-danger)',
    cancelled: 'var(--theme-muted)',
  }
  return FALLBACK[key] ?? 'var(--theme-muted)'
}

const statusMutate = (boardId: string, method: string, body: unknown) =>
  fetch(`/api/boards/${boardId}/statuses`, {
    method,
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify(body),
  }).then(async (r) => (r.ok ? r.json() : Promise.reject(new Error(((await r.json().catch(() => ({}))) as { error?: string }).error ?? String(r.status)))))

export const createBoardStatus = (boardId: string, input: { label: string; color?: string; category?: string; agentStart?: boolean }) =>
  statusMutate(boardId, 'POST', input)
export const updateBoardStatus = (
  boardId: string,
  statusId: string,
  patch: { label?: string; color?: string; category?: string; agentStart?: boolean },
) => statusMutate(boardId, 'PUT', { statusId, ...patch })
export const reorderBoardStatuses = (boardId: string, order: string[]) => statusMutate(boardId, 'PUT', { order })
export const deleteBoardStatus = (boardId: string, statusId: string, reassignTo: string) =>
  statusMutate(boardId, 'DELETE', { statusId, reassignTo })
