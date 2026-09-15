// The LIVE WORK SESSION on a ticket: the "someone is on it right now"
// surface. One query over /api/tasks/{id}/work-session, polled while a
// session is live (the modal attaches the run's own SSE for instant phase —
// this poll is the ticker's floor, not the stream).
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface LiveWorkSession {
  runId: string
  state: string
  phase: string | null
  agentModel: string | null
  turn: number | null
  /** The tail of the agent's last reply, from the session's checkpoint. */
  lastTail: string | null
}

export function useWorkSession(taskId: () => string | null) {
  return createQuery(() => ({
    queryKey: ['work-session', taskId()],
    enabled: !!taskId(),
    // While live, the ticker should never be more than a few seconds stale;
    // while idle the route answers null and this cadence costs one cheap row
    // lookup.
    refetchInterval: 5_000,
    queryFn: (): Promise<{ session: LiveWorkSession | null }> =>
      getJson<{ session: LiveWorkSession | null }>(`/api/tasks/${taskId()}/work-session`),
  }))
}

/** Invalidate from anywhere a run transition lands (the watch modal's SSE,
 *  the ticket's own activity refresh) so the ticker flips the moment the
 *  session ends. */
export function useInvalidateWorkSession() {
  const qc = useQueryClient()
  return (taskId: string) => void qc.invalidateQueries({ queryKey: ['work-session', taskId] })
}

/** The board's live work sessions, one map read for every card. The list
 *  view's question — which tickets are being worked right now — answered in
 *  one poll rather than one per row. */
export function useBoardWorkSessions(boardId: () => string | null) {
  return createQuery(() => ({
    queryKey: ['board-work-sessions', boardId()],
    enabled: !!boardId(),
    refetchInterval: 5_000,
    queryFn: (): Promise<{ sessions: Record<string, LiveWorkSession> }> =>
      getJson<{ sessions: Record<string, LiveWorkSession> }>(`/api/boards/${boardId()}/work-sessions`),
  }))
}
