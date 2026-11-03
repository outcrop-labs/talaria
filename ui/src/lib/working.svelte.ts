// The tab bar's agent signal: one query over /api/working, which answers
// which views an agent is working in right now. A glyph is drawn only from
// this read. A view the server has no arm for has no key here, so nothing can
// light it.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'
import { onUserStreamOpen, useUserEvents } from '@/lib/user-events.svelte'

/** One view's working set: how many things an agent is on there, and the
 *  freshest of them to name and link. */
export interface Lead {
  count: number
  label: string
  href: string
}

/** What /api/working answers. An arm is null when nothing is working there. */
export interface Working {
  comms: Lead | null
  work: Lead | null
  boards: Lead | null
}

const KEY = ['working'] as const

/** Room replies and ticket sessions announce their start on the firehose but
 *  never their end, so while anything is lit the read polls at the pace the
 *  sidebar and the ticket ticker poll their own working rows. Idle, the 30s
 *  floor covers a dropped stream, as it does for /api/unreads. */
const LIT_POLL_MS = 5_000
const IDLE_POLL_MS = 30_000

function lit(w: Working | undefined): boolean {
  return !!(w?.comms || w?.work || w?.boards)
}

export const workingQuery = {
  queryKey: KEY,
  queryFn: (): Promise<Working> => getJson<Working>('/api/working'),
  refetchInterval: (query: { state: { data: Working | undefined } }) =>
    lit(query.state.data) ? LIT_POLL_MS : IDLE_POLL_MS,
}

/** The reading a surface may draw from: null when the read failed or has not
 *  answered. A failure is never "nothing working", and a failed refetch does
 *  not keep an old glyph lit, so the data held from an earlier success is
 *  dropped too. */
export function workingOf(query: { isError: boolean; data: Working | undefined }): Working | null {
  return query.isError ? null : (query.data ?? null)
}

export function useWorking(): { readonly current: Working | null } {
  const qc = useQueryClient()
  const query = createQuery(() => workingQuery)
  const refetch = () => void qc.invalidateQueries({ queryKey: KEY })
  useUserEvents((event) => {
    if (event.type === 'run' || event.type === 'conversation' || event.type === 'channel') refetch()
  })
  $effect(() => onUserStreamOpen(refetch))
  return {
    get current() {
      return workingOf(query)
    },
  }
}
