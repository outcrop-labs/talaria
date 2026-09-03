// The person's firehose: ONE EventSource per tab, shared.
//
// `/api/me/events` carries the id-shaped UserEvents (api/src/realtime.rs):
// run transitions, notification and brief landings, and — since the rails
// went live — channel and conversation turns. Every payload is ids only, by
// server contract, so the client's whole job on any event is "refetch
// through the ordinary route": no event ever patches a page's content.
//
// Until this module, each surface opened its OWN EventSource — toasts had
// one, the brief had one — and every stream is a dedicated Redis subscriber
// on the server. A firehose with one reader per subscriber is a connection
// farm; this is the same shape as the toast store: module-level singleton,
// callers subscribe, the last unsubscribe closes the socket.
import { useQueryClient } from '@tanstack/svelte-query'

/** The wire's id-shaped events — camelCase, tagged `type`, nothing else. */
export type UserEvent =
  | { type: 'run'; runId: string; state: string }
  | { type: 'notification'; notificationId: string }
  | { type: 'brief'; briefId: string; seq: number }
  | { type: 'channel'; channelId: string }
  | { type: 'conversation'; conversationId: string }

type Listener = (event: UserEvent) => void

let source: EventSource | null = null
let refs = 0
const listeners = new Set<Listener>()

function connect(): void {
  if (source) return
  source = new EventSource('/api/me/events')
  source.onmessage = (event: MessageEvent<string>) => {
    // A frame we cannot parse is a frame we ignore — every consumer's own
    // poll floor is what makes that safe rather than silent.
    let parsed: UserEvent
    try {
      parsed = JSON.parse(event.data) as UserEvent
    } catch {
      return
    }
    if (!parsed || typeof parsed.type !== 'string') return
    for (const fn of listeners) fn(parsed)
  }
}

function disconnect(): void {
  source?.close()
  source = null
}

/** Subscribe to the firehose. Returns the unsubscribe; the LAST unsubscribe
 *  in the tab closes the shared connection. Call from an $effect's setup and
 *  return the handle as its cleanup, and the socket's life is exactly the
 *  union of its consumers' lives. */
export function onUserEvent(fn: Listener): () => void {
  listeners.add(fn)
  refs++
  connect()
  return () => {
    listeners.delete(fn)
    refs--
    if (refs <= 0) disconnect()
  }
}

/** The app-wide invalidation map, one line per event: what refetches when
 *  the firehose ticks. Mounted once (AppLayout) so a badge or a list moves
 *  no matter WHICH page the person is on — a room they haven't entered, a
 *  thread they walked away from, a run that finished while they read email.
 *
 *  `run` is here deliberately: a research run's runs-engine id IS its
 *  research_runs id, and before this the research list learned a run had
 *  finished only on its own page's poll — which pauses in a hidden tab. */
export function useUserEventInvalidation(): void {
  const qc = useQueryClient()
  $effect(() =>
    onUserEvent((event) => {
      switch (event.type) {
        case 'notification':
          void qc.invalidateQueries({ queryKey: ['notifications'] })
          void qc.invalidateQueries({ queryKey: ['home'] })
          break
        case 'channel':
          void qc.invalidateQueries({ queryKey: ['channels'] })
          break
        case 'conversation':
          void qc.invalidateQueries({ queryKey: ['conversations'] })
          break
        case 'run':
          void qc.invalidateQueries({ queryKey: ['research-runs'] })
          void qc.invalidateQueries({ queryKey: ['research-run', event.runId] })
          break
        case 'brief':
          void qc.invalidateQueries({ queryKey: ['daily-brief'] })
          break
      }
    }),
  )
}
