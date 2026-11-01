// The person's firehose: ONE EventSource per BROWSER, shared.
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
//
// AND SINCE WAVE 2, ONE PER BROWSER, NOT PER TAB: this module runs the tab's
// notification runtime — the leader lock and tab registry (notify-leader.ts)
// and the hub (notify-hub.ts). The leader tab opens the only stream and
// rebroadcasts its frames; every other tab hears them on the channel. A
// subscriber here cannot tell which it is: frames and opens arrive the same
// way. The stream opens with `?since=<the arrival record's cursor>` (I-19),
// read at every reconnect, so a reopened stream replays what it missed.
import { openStream, type StreamState } from '@/lib/sse'
import { untrack } from 'svelte'
import { useQueryClient } from '@tanstack/svelte-query'
import { lastInputAt, trackInput } from '@/lib/looking'
import { startHub, type Hub, type HubRole } from '@/lib/notify-hub'
import { browserLeaderEdges, startNotifyLeader, type NotifyLeader } from '@/lib/notify-leader'
import { clientId } from '@/lib/notify-presence'
import { noteStreamEvent, setHubRole, setStreamState } from '@/lib/notify-status.svelte'
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

interface Runtime {
  leader: NotifyLeader
  hub: Hub
  stop: () => void
}

let runtime: Runtime | null = null
let refs = 0
const listeners = new Set<Listener>()
const openListeners = new Set<OpenListener>()
const runtimeListeners = new Set<(r: { leader: NotifyLeader; hub: Hub } | null) => void>()
let sinceFn: (() => number | null) | null = null
let pageHooked = false

/** Where the stream's replay starts: the arrival tracker registers its
 *  cursor here (NotificationToasts). Returns the unregister. */
export function setStreamSince(fn: () => number | null): () => void {
  sinceFn = fn
  return () => {
    if (sinceFn === fn) sinceFn = null
  }
}

function streamUrl(): string {
  let since: number | null = null
  try {
    since = sinceFn?.() ?? null
  } catch {
    since = null
  }
  return typeof since === 'number' && Number.isFinite(since) && since >= 0
    ? `/api/me/events?since=${Math.floor(since)}`
    : '/api/me/events'
}

function dispatch(data: string): void {
  // A frame we cannot parse is a frame we ignore — every consumer's own
  // poll floor is what makes that safe rather than silent.
  let parsed: UserEvent
  try {
    parsed = JSON.parse(data) as UserEvent
  } catch {
    return
  }
  if (!parsed || typeof parsed.type !== 'string') return
  status(() => noteStreamEvent())
  for (const fn of listeners) fn(parsed)
}

const statusOf = (s: StreamState): 'connecting' | 'open' | 'reconnecting' | 'closed' => (s === 'paused' ? 'closed' : s)

/** The status panel's setters read the state they write: called inside a
 *  component's $effect (the runtime starts from one) they would make that
 *  effect depend on the status and loop. Every call goes through here. */
const status = (fn: () => void) => untrack(fn)

function start(): void {
  if (runtime || typeof window === 'undefined' || typeof document === 'undefined') return
  hookPage()
  const releaseInput = trackInput()
  const leader = startNotifyLeader({
    ...browserLeaderEdges(),
    self: () => ({ visible: document.visibilityState === 'visible', focused: safeFocus(), clientId: clientId() }),
    lastInputAt,
    hub: true,
  })
  // The connection itself goes through the shared door, which RECONNECTS it:
  // a deploy or an api restart is a 502/524 at the edge, and the firehose
  // comes back on a backoff instead of staying dead until a reload. Every
  // (re)open tells the open-listeners, which refetch whatever the stream may
  // have missed while it was down.
  const hub = startHub({
    leader,
    open: (onFrame, cb) => openStream(streamUrl, onFrame, { reconnect: true, onOpen: cb.onOpen, onState: cb.onState }),
  })
  const offs = [
    hub.onFrame(dispatch),
    hub.onOpen((info) => {
      for (const fn of openListeners) fn(info)
    }),
    hub.onRole((r: HubRole) => status(() => setHubRole(r))),
    hub.onState((s) => status(() => setStreamState(statusOf(s)))),
  ]
  status(() => {
    setHubRole(hub.role())
    setStreamState(statusOf(hub.state()))
  })
  runtime = {
    leader,
    hub,
    stop: () => {
      for (const off of offs) off()
      hub.close()
      leader.close()
      releaseInput()
      status(() => {
        setStreamState('closed')
        setHubRole('unknown')
      })
    },
  }
  for (const fn of runtimeListeners) fn({ leader, hub })
}

function stop(): void {
  if (!runtime) return
  const r = runtime
  runtime = null
  r.stop()
  for (const fn of runtimeListeners) fn(null)
}

function safeFocus(): boolean {
  try {
    return document.hasFocus()
  } catch {
    return false
  }
}

/** Leaving the page: release the lock and say goodbye, so the next tab leads
 *  now. A page restored from the back/forward cache comes back with the
 *  runtime closed and starts a new one (a new tab id, a new place in the
 *  lock's queue). A frozen page that resumes refetches as after a reopen. */
function hookPage(): void {
  if (pageHooked) return
  pageHooked = true
  window.addEventListener('pagehide', () => stop())
  window.addEventListener('pageshow', (e: PageTransitionEvent) => {
    if (e.persisted && refs > 0) start()
  })
  document.addEventListener('resume', () => runtime?.hub.resumed())
}

function connect(): void {
  start()
}

function disconnect(): void {
  stop()
}

/** Hold the tab's notification runtime (the leader lock, the hub) for as long
 *  as the caller lives, even with no event subscriber; returns the release.
 *  NotificationToasts holds it: it decides with the leader. */
export function acquireNotifyRuntime(): () => void {
  refs++
  connect()
  let done = false
  return () => {
    if (done) return
    done = true
    refs--
    if (refs <= 0) {
      refs = 0
      disconnect()
    }
  }
}

/** The tab's leader (null while the runtime is down). */
export function notifyLeader(): NotifyLeader | null {
  return runtime?.leader ?? null
}

/** The hub's role in this tab (diagnostics; the status panel reads
 *  notify-status). */
export function hubRole(): HubRole | null {
  return runtime?.hub.role() ?? null
}

/** Hear the runtime start (with its leader) and stop (null). */
export function onNotifyRuntime(fn: (r: { leader: NotifyLeader; hub: Hub } | null) => void): () => void {
  runtimeListeners.add(fn)
  return () => runtimeListeners.delete(fn)
}

/** Subscribe to the firehose. Returns the unsubscribe; the LAST unsubscribe
 *  in the tab closes the shared connection. Call from an $effect's setup and
 *  return the handle as its cleanup, and the socket's life is exactly the
 *  union of its consumers' lives. */
export function onUserEvent(fn: Listener): () => void {
  listeners.add(fn)
  const release = acquireNotifyRuntime()
  return () => {
    listeners.delete(fn)
    release()
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
