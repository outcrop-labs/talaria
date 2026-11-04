// The LIVE WORK SESSION on a ticket: the "someone is on it right now"
// surface. One query over /api/tasks/{id}/work-session, polled while a
// session is live (the modal attaches the run's own SSE for instant phase —
// this poll is the ticker's floor, not the stream). A packing refuse is a
// separate `wait` object — never a fake session with a null runId.
import { createQuery, useQueryClient, type QueryClient } from '@tanstack/svelte-query'
import type { LedgerRow, OutsideSummary, RewindRow, RunAttempt, TicketMove } from '@/lib/agent-checkpoints'
import { stopWorkSession } from '@/lib/boards.svelte'
import { getJson, postJson } from '@/lib/fetch-json'

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
  lineageId: string
  attemptNo: number
  rewoundFromRunId: string | null
  rewoundFromSeq: number | null
  outside: OutsideSummary
}

export interface WorkSessionHistory {
  sessions: WorkSessionHistoryRow[]
  lineages: { lineageId: string; attempts: string[] }[]
  rewinds: RewindRow[]
  ticketMove: TicketMove | null
}

/** The ticket's work log: every session it has seen, most recent first
 *  (live included, capped server-side at 20). A settled log is a read-once
 *  record, so the poll only runs while some row has yet to finish. */
export function useWorkSessionHistory(taskId: () => string | null) {
  return createQuery(() => ({
    queryKey: ['work-sessions-history', taskId()],
    enabled: !!taskId(),
    refetchInterval: (query) =>
      ((query.state.data as WorkSessionHistory | undefined)?.sessions ?? []).some((s) => !s.finishedAt)
        ? 30_000
        : false,
    queryFn: () => getJson<WorkSessionHistory>(`/api/tasks/${taskId()}/work-sessions`),
  }))
}

export interface RunLineage {
  attempts: RunAttempt[]
  rewinds: RewindRow[]
  ticketMove: TicketMove | null
}

/** One run's whole lineage in one read, so no poll can show an attempt with
 *  another moment's rows. The history comes through its own cache key, and the
 *  work log sees what this saw. */
export async function readRunLineage(qc: QueryClient, taskId: string, runId: string): Promise<RunLineage> {
  const history = await qc.fetchQuery({
    queryKey: ['work-sessions-history', taskId],
    queryFn: () => getJson<WorkSessionHistory>(`/api/tasks/${taskId}/work-sessions`),
    staleTime: 0,
  })
  const runs = history.lineages.find((l) => l.attempts.includes(runId))?.attempts ?? [runId]
  const attempts = await Promise.all(
    runs.map(async (id): Promise<RunAttempt> => {
      const { checkpoints } = await getJson<{ checkpoints: LedgerRow[] }>(`/api/runs/${id}/checkpoints`)
      const session = history.sessions.find((s) => s.runId === id)
      return {
        runId: id,
        attemptNo: session?.attemptNo ?? 1,
        state: session?.state,
        rewoundFromRunId: session?.rewoundFromRunId ?? null,
        rewoundFromSeq: session?.rewoundFromSeq ?? null,
        outside: session?.outside,
        rows: checkpoints,
      }
    }),
  )
  return { attempts, rewinds: history.rewinds, ticketMove: history.ticketMove }
}

export const startRewind = (runId: string, seq: number) =>
  postJson<{ rewindRunId: string }>(`/api/runs/${runId}/rewind`, { seq })

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
