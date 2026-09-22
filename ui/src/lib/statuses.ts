// Client view of a board's status set — the workflow columns. Server truth
// via /api/boards/:id/statuses (defaults + the system Blocked column when the
// board never customized). Helpers keep label/color lookups safe for custom
// keys and legacy constants alike.
import { resolve, type MaybeGetter } from '@/lib/reactive-arg'
import { createQuery } from '@tanstack/svelte-query'
import { delJson, getList, postJson, putJson } from '@/lib/fetch-json'
import { STATUS_LABEL } from './task-const'
import { LABEL_CSS } from '@/components/board/field-pills'
// THE COLUMN SHAPES live here — the client's wire vocabulary for the workflow
// columns, served by the Rust statuses engine (`api/src/statuses.rs`, whose
// `BoardStatus` is the authority this copies). 'blocked' stays OUT of
// StatusCategory — it is not a workflow category, no write ever assigns it,
// and every resolver excludes it — riding only on BoardStatus.category for
// the system column.
export type StatusCategory = 'open' | 'active' | 'review' | 'done'

export interface BoardStatus {
  id: string | null // null for virtual defaults + the system blocked row
  key: string
  label: string
  color: string
  category: StatusCategory | 'blocked'
  agentStart: boolean
  position: number
  system?: boolean
}

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */

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

/** THE STATUS PALETTE — the colour of every status key that no board row
 *  answers for: the virtual defaults before a board's status set arrives, and
 *  the OFF-BOARD terminals (`failed`, `cancelled`), which are legal on every
 *  board but never columns, so no row of `/api/boards/:id/statuses` carries
 *  them. Its keys are `TaskStatus`, across both lists in `@/lib/task-const`.
 *
 *  ONE MAP: column accents (Kanban), status pills (StatusPill) and filter
 *  facets (FilterBar) all read this, so no surface can disagree about what a
 *  status looks like. Call sites keep their own unknown-key guard and fall
 *  back to the muted theme colour. */
export const STATUS_COLOR: Record<string, string> = {
  inbox: 'var(--theme-muted)',
  assigned: 'var(--theme-accent)',
  in_progress: 'var(--theme-warning)',
  blocked: 'var(--theme-danger)',
  quality_review: 'var(--theme-accent-secondary)',
  done: 'var(--theme-success)',
  failed: 'var(--theme-danger)',
  cancelled: 'var(--theme-muted)',
}

export const statusColorOf = (key: string, statuses: BoardStatus[]): string => {
  const c = statuses.find((s) => s.key === key)?.color
  if (c) return LABEL_CSS[c as keyof typeof LABEL_CSS] ?? 'var(--theme-muted)'
  return STATUS_COLOR[key] ?? 'var(--theme-muted)'
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
