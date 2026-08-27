// Client view of a board's status set — the workflow columns. Server truth
// via /api/boards/:id/statuses (defaults + the system Blocked column when the
// board never customized). Helpers keep label/color lookups safe for custom
// keys and legacy constants alike.
import { createQuery } from '@tanstack/svelte-query'
import { delJson, getList, postJson, putJson } from '@/lib/fetch-json'
import { STATUS_LABEL } from './task-const'
import { LABEL_CSS } from '@/components/board/field-pills'
import type { BoardStatus, StatusCategory } from '@/server/statuses'

// ONE OWNER for the column shapes. This file used to re-declare both, and the
// copies had drifted exactly the way copies do: the server keeps 'blocked' OUT
// of StatusCategory — it is not a workflow category, no write ever assigns it,
// and every resolver excludes it — adding it only on BoardStatus.category for
// the system column; this copy had folded it into the union. The wire type was
// the same either way, but the two files spelled two different vocabularies,
// and a reader comparing them had no way to know which was the accident.
// Re-exported so every existing '@/lib/statuses' import keeps working.
export type { BoardStatus, StatusCategory }

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export function useBoardStatuses(boardId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(boardId)
    return {
      queryKey: ['board-statuses', id],
      enabled: !!id,
      // An empty status set collapses the board to zero columns — that must only
      // ever happen because the server said so.
      queryFn: (): Promise<BoardStatus[]> => getList<BoardStatus>(`/api/boards/${id}/statuses`, 'statuses'),
    }
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

export const createBoardStatus = (boardId: string, input: { label: string; color?: string; category?: string; agentStart?: boolean }) =>
  postJson<{ status: BoardStatus }>(`/api/boards/${boardId}/statuses`, input)
export const updateBoardStatus = (
  boardId: string,
  statusKey: string,
  patch: { label?: string; color?: string; category?: string; agentStart?: boolean },
) => putJson<{ ok: true }>(`/api/boards/${boardId}/statuses`, { statusKey, ...patch })
/** Order = status KEYS (stable across virtual defaults + materialized rows). */
export const reorderBoardStatuses = (boardId: string, order: string[]) =>
  putJson<{ ok: true }>(`/api/boards/${boardId}/statuses`, { order })
export const deleteBoardStatus = (boardId: string, statusKey: string, reassignTo: string) =>
  delJson<{ ok: true }>(`/api/boards/${boardId}/statuses`, { statusKey, reassignTo })
