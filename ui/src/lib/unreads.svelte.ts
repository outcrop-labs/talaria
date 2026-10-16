// The rail badges' client state: one query over /api/unreads, and research's
// seen-gesture. The counts are the same predicates the pills ride (the
// server's route keeps them in lockstep), so a badge is a summary of its own
// surface, never a second opinion.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson, putJson } from '@/lib/fetch-json'
import { sumSessions } from '@/lib/unreads-sum'

/** What /api/unreads answers: one arm per surface that counts. */
interface UnreadsWire {
  comms: number
  plan: number
  work: number
  research: number
  notifications: number
}

export interface Unreads extends UnreadsWire {
  /** WORK SESSIONS, PLANS AND RESEARCH THREADS AS ONE COUNT — the rail has one
   *  Work entry now and it holds all three, so this is the number it carries.
   *  Summed here rather than server-side because the route's arms are still
   *  the honest per-kind reads (a pill, a surface, a future rail can want one
   *  of them alone); what the merged entry needs is their total, and a badge
   *  that counted only work sessions would read "nothing waiting" with an
   *  unread plan turn sitting in the same rail. */
  sessions: number
}

export function useUnreads() {
  return createQuery(() => ({
    queryKey: ['unreads'],
    // A FAILED READ IS NOT A ZERO: the route answers 500 when any arm fails
    // (never a silent 0), the query lands in error, and the badge renders `!`
    // rather than a number — so the sum below only ever runs over real counts.
    queryFn: async (): Promise<Unreads> => {
      const wire = await getJson<UnreadsWire>('/api/unreads')
      return { ...wire, sessions: sumSessions(wire) }
    },
    // The firehose invalidates on every landing (a room, a thread, a run); this
    // 30s floor is for the dropped stream and the sleeping laptop — the same
    // two-legged liveness the notifications query itself rides.
    refetchInterval: 30_000,
  }))
}

/** Opening a run is the "seen" gesture for research: the notifications that
 *  pointed at it (its completion, its questions) mark read by href, and the
 *  badge and the bell clear together. */
export function useMarkResearchRunSeen() {
  const qc = useQueryClient()
  return async (runId: string) => {
    try {
      await putJson<{ ok: true }>('/api/notifications', { href: `/research/${runId}` })
    } catch {
      // Deliberately quiet, unlike the bell's mark-read: this fires on page
      // open, where a toast would greet a person walking in, and a failed
      // seen costs nothing permanent — the badge stays, and the next open
      // fires the same gesture again.
      return
    }
    await qc.invalidateQueries({ queryKey: ['notifications'] })
    await qc.invalidateQueries({ queryKey: ['unreads'] })
    await qc.invalidateQueries({ queryKey: ['research-runs'] })
  }
}
