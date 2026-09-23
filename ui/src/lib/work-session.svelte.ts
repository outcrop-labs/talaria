// The LIVE WORK SESSION on a ticket: the "someone is on it right now"
// surface. One query over /api/tasks/{id}/work-session, polled while a
// session is live (the modal attaches the run's own SSE for instant phase —
// this poll is the ticker's floor, not the stream). A packing refuse is a
// separate `wait` object — never a fake session with a null runId.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { stopWorkSession } from '@/lib/boards.svelte'
import { getJson } from '@/lib/fetch-json'

export interface LiveWorkSession {
  runId: string
  state: string
  phase: string | null
  agentModel: string | null
  turn: number | null
  lastTail?: string | null
}

export interface WorkWait {
  agentModel: string
  reason: string
  position: number
  phase: string
  queuedAt: number
}

export function useWorkSession(taskId: () => string | null) {
  return createQuery(() => ({
    queryKey: ['work-session', taskId()],
    enabled: !!taskId(),
    refetchInterval: 5_000,
    queryFn: (): Promise<{ session: LiveWorkSession | null; wait: WorkWait | null }> =>
      getJson<{ session: LiveWorkSession | null; wait: WorkWait | null }>(
        `/api/tasks/${taskId()}/work-session`,
      ),
  }))
}

/** Invalidate from anywhere a run transition lands (the watch modal's SSE,
 *  the ticket's own activity refresh) so the ticker flips the moment the
 *  session ends. */
export function useInvalidateWorkSession() {
  const qc = useQueryClient()
  return (taskId: string) => void qc.invalidateQueries({ queryKey: ['work-session', taskId] })
}

/** One row of the ticket's work-session history — live and finished alike. */
export interface WorkSessionHistoryRow {
  runId: string
  state: string
  phase: string | null
  agentModel: string
  turn: number | null
  finishedAt: string | null
}

/** The ticket's work log: every session it has seen, most recent first
 *  (live included, capped server-side at 20). A settled log is a read-once
 *  record, so the poll only runs while some row has yet to finish. */
export function useWorkSessionHistory(taskId: () => string | null) {
  return createQuery(() => ({
    queryKey: ['work-sessions-history', taskId()],
    enabled: !!taskId(),
    refetchInterval: (query) =>
      ((query.state.data as { sessions: WorkSessionHistoryRow[] } | undefined)?.sessions ?? []).some(
        (s) => !s.finishedAt,
      )
        ? 30_000
        : false,
    queryFn: (): Promise<{ sessions: WorkSessionHistoryRow[] }> =>
      getJson<{ sessions: WorkSessionHistoryRow[] }>(`/api/tasks/${taskId()}/work-sessions`),
  }))
}

/** The brake, wired: POST the stop, then drop every work-session read for
 *  the ticket so the ticker, the board's chips and the log all flip on the
 *  next paint instead of waiting out their polls. */
export function useStopWorkSession() {
  const qc = useQueryClient()
  return async (taskId: string) => {
    await stopWorkSession(taskId)
    void qc.invalidateQueries({ queryKey: ['work-session', taskId] })
    void qc.invalidateQueries({ queryKey: ['board-work-sessions'] })
    void qc.invalidateQueries({ queryKey: ['work-sessions-history', taskId] })
  }
}

/** The board's live work sessions, one map read for every card. The list
 *  view's question — which tickets are being worked right now — answered in
 *  one poll rather than one per row. `waits` is the packing queue. */
export function useBoardWorkSessions(boardId: () => string | null) {
  return createQuery(() => ({
    queryKey: ['board-work-sessions', boardId()],
    enabled: !!boardId(),
    refetchInterval: 5_000,
    queryFn: (): Promise<{
      sessions: Record<string, LiveWorkSession>
      waits: Record<string, WorkWait>
    }> => getJson(`/api/boards/${boardId()}/work-sessions`),
  }))
}
