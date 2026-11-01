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
import { openStream } from '@/lib/sse'
import { useQueryClient } from '@tanstack/svelte-query'
import type { PresenceEvent, ResyncEvent, SessionEvent } from './notify-wire'

/** The wire's id-shaped events — camelCase, tagged `type`, nothing else. The
 *  contract's additions (notify-wire.ts) are optional fields on two of them
 *  and three new members; the switch below ignores what it does not handle. */
export type UserEvent =
  | { type: 'run'; runId: string; state: string }
  /** `seq`: the person's notify seq the write took (I-10). */
  | { type: 'notification'; notificationId: string; seq?: number }
  /** Something was READ (another tab, a channel read to its end): the count
   *  after it. The bell drops now instead of on its next poll. Since the
   *  contract: the For-you count, exactly which rows (≤ 100; past that
   *  `resync`), and whether it was a mark-all (I-16). */
  | {
      type: 'notifications'
      unread: number
      unreadForYou?: number
      readIds?: string[]
      all?: boolean
      resync?: boolean
    }
  | { type: 'brief'; briefId: string; seq: number }
  | { type: 'channel'; channelId: string }
  | { type: 'conversation'; conversationId: string }
  | { type: 'typing'; channelId: string; userId: string; typing: boolean }
  | PresenceEvent
  | ResyncEvent
  | SessionEvent

type Listener = (event: UserEvent) => void
type OpenListener = (info: { first: boolean }) => void

let close: (() => void) | null = null
let refs = 0
const listeners = new Set<Listener>()
const openListeners = new Set<OpenListener>()

function connect(): void {
  if (close) return
  // The singleton subscriber registry stays here — this is the one stream with
  // N consumers — but the connection itself goes through the shared door,
  // which RECONNECTS it: a deploy or an api restart is a 502/524 at the edge,
  // and the firehose comes back on a backoff instead of staying dead until a
  // reload. Every (re)open tells the open-listeners, which refetch whatever
  // the stream may have missed while it was down (it carries no replay).
  close = openStream(
    '/api/me/events',
    (data) => {
      // A frame we cannot parse is a frame we ignore — every consumer's own
      // poll floor is what makes that safe rather than silent.
      let parsed: UserEvent
      try {
        parsed = JSON.parse(data) as UserEvent
      } catch {
        return
      }
      if (!parsed || typeof parsed.type !== 'string') return
      for (const fn of listeners) fn(parsed)
    },
    {
      reconnect: true,
      onOpen: (info) => {
        for (const fn of openListeners) fn(info)
      },
    },
  )
}

function disconnect(): void {
  close?.()
  close = null
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

/** Hear every (re)open of the firehose — the moment to refetch what a gap
 *  may have dropped. Does not open the stream on its own; returns the
 *  unsubscribe. */
export function onUserStreamOpen(fn: OpenListener): () => void {
  openListeners.add(fn)
  return () => openListeners.delete(fn)
}

/** A component's subscription to the firehose, for exactly its lifetime:
 *  the shared connection opens with the first subscriber and closes with the
 *  last. Call during component init. Events carry ids only — the listener's
 *  job is to refetch (invalidate a query), never to patch content. This is
 *  the door the SDK hands apps, so an app surface that shows run state does
 *  not open a second EventSource (a second Redis subscriber) of its own. */
export function useUserEvents(fn: Listener): void {
  $effect(() => onUserEvent((event) => fn(event)))
}

/** Every query key the map below drives — refetched on each reopen. */
export const USER_STREAM_KEYS = [
  'notifications',
  'unreads',
  'home',
  'channels',
  'conversations',
  'research-runs',
  'research-run',
  'daily-brief',
] as const

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
  // EVERY KEY THIS MAP DRIVES refetches when the stream (re)opens: whatever
  // was published while it was down (a deploy, a laptop asleep, the edge's
  // idle cut) is gone from the wire, and the ordinary reads are the replay.
  // The FIRST open refetches only the inbox — the other reads were made
  // moments ago, and the inbox is what the arrival record surfaces from.
  $effect(() =>
    onUserStreamOpen(({ first }) => {
      void qc.invalidateQueries({ queryKey: ['notifications'] })
      if (first) return
      for (const key of USER_STREAM_KEYS) void qc.invalidateQueries({ queryKey: [key] })
    }),
  )
  $effect(() =>
    onUserEvent((event) => {
      switch (event.type) {
        case 'notification':
        case 'notifications':
          void qc.invalidateQueries({ queryKey: ['notifications'] })
          void qc.invalidateQueries({ queryKey: ['home'] })
          void qc.invalidateQueries({ queryKey: ['unreads'] })
          break
        case 'channel':
          void qc.invalidateQueries({ queryKey: ['channels'] })
          void qc.invalidateQueries({ queryKey: ['unreads'] })
          break
        case 'conversation':
          void qc.invalidateQueries({ queryKey: ['conversations'] })
          void qc.invalidateQueries({ queryKey: ['unreads'] })
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
