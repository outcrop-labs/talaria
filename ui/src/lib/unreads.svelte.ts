// The rail badges' client state: one query over /api/unreads, and research's
// seen-gesture. The counts are the same predicates the pills ride (the
// server's route keeps them in lockstep), so a badge is a summary of its own
// surface, never a second opinion.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson, putJson } from '@/lib/fetch-json'

export interface Unreads {
  comms: number
  plan: number
  research: number
  notifications: number
}

export function useUnreads() {
  return createQuery(() => ({
    queryKey: ['unreads'],
    queryFn: (): Promise<Unreads> => getJson<Unreads>('/api/unreads'),
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
