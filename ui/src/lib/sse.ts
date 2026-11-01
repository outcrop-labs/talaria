// The one SSE door.
//
// Four surfaces open an EventSource: the board rail (invalidate on any board
// event), the channel rail (parse the frame, invalidate what it names), the
// account's `/api/me/events` fan (one shared subscription, reference-counted),
// and the work-watch panel (parse phases and the terminal state). Each grew its
// own `new EventSource(...)`, its own `onmessage`, and its own teardown — and
// the one thing they disagreed about is the part that matters when it breaks:
// what happens to a frame that does not parse. Two swallow it silently, one
// lets JSON.parse throw inside the listener (which the browser reports as an
// unhandled error and keeps the stream open), and the newest returns early.
//
// The door does not decide any of that: it hands the RAW data over and lets the
// caller parse, because "a frame I cannot parse is a frame I ignore" is a
// per-surface judgment (every consumer here has a poll floor behind it). What
// the door owns is the connection: one EventSource, one `onmessage` wiring, and
// a close that cannot be forgotten.
//
// AND, FOR A CALLER THAT OPTS IN, THE CONNECTION'S RECOVERY. Every real
// instance sits behind an edge (Cloudflare): a deploy or an api restart
// reaches the browser as a 502 or a 524, and an EventSource that gets a
// non-200 does not retry — it goes CLOSED for good, and the surface goes
// silently stale until a reload. With `reconnect`, a CLOSED stream is reopened
// on a backoff (1 s, 2 s, 4 s … 30 s, ±20 %), at once when the network comes
// back or the tab comes back into view, and the backoff resets once a
// connection has held for 30 s. After five failures in a row it asks whether
// the person is still signed in, and stops if not (the session gate takes it
// from there). `onOpen` runs on EVERY open, the first included, so a caller
// refetches whatever it may have missed while the stream was down — the
// stream carries no replay.
//
// AND, FOR A TOPIC STREAM, SLEEP WHILE HIDDEN (`pauseWhenHidden`). A board,
// a channel or a run's stream in a tab nobody can see is a socket and a
// Redis subscriber spent on nothing — and on a plain-http dev origin, where
// the browser allows six connections per host, eight such tabs starve every
// fetch (BT-3). So a paused stream closes after the tab has been hidden for
// 30 s; when the tab is shown again, `onResume` runs first (refetch what the
// stream would have said) and then the stream reopens. On an HTTP/1.x page
// (a dev stack: real instances are HTTP/2 at the edge) the wait is 5 s, so
// the six-socket budget frees up as fast as the person moves between tabs.
// The person's own `/api/me/events` never pauses: it is what alerts.

import { getJson } from '@/lib/fetch-json'

export type StreamState = 'connecting' | 'open' | 'reconnecting' | 'paused' | 'closed'

export interface StreamOptions {
  /** Reopen a CLOSED stream with backoff (see above). */
  reconnect?: boolean
  /** Called on every open; `first` is the initial connection, `resumed` an
   *  open that follows a hidden-tab pause (its `onResume` already ran). */
  onOpen?: (info: { first: boolean; resumed?: boolean }) => void
  /** Close while the tab is hidden (see above): `true` for the default wait,
   *  or a wait in ms. */
  pauseWhenHidden?: boolean | number
  /** Runs when a paused stream's tab is shown again, BEFORE it reopens. */
  onResume?: () => unknown
  /** Every change of the connection's state (the status panel's feed). */
  onState?: (state: StreamState) => void
}

/** How long a hidden tab keeps a paused stream open. */
export const HIDDEN_PAUSE_MS = 30_000
/** …and on an HTTP/1.x page, whose browser allows six sockets per host. */
export const HIDDEN_PAUSE_H1_MS = 5_000

/** The page's own transport: `http/1.1` on a dev stack, `h2`/`h3` at an edge. */
export function pageProtocol(): string {
  try {
    if (typeof performance === 'undefined' || typeof performance.getEntriesByType !== 'function') return ''
    const nav = performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming | undefined
    return nav?.nextHopProtocol ?? ''
  } catch {
    return ''
  }
}

/** The wait before a hidden tab's topic stream closes. */
export function hiddenPauseMs(protocol: string = pageProtocol()): number {
  return /^http\/1/i.test(protocol) ? HIDDEN_PAUSE_H1_MS : HIDDEN_PAUSE_MS
}

const BASE_MS = 1_000
const CAP_MS = 30_000
const JITTER = 0.2
/** A connection that held this long resets the backoff. */
export const STABLE_MS = 30_000
/** Consecutive failures after which the session is checked. */
export const PROBE_AFTER = 5

/** The wait before reconnect attempt `failures` (1-based): 1 s, 2 s, 4 s …
 *  capped at 30 s, each ±20 %. `rand` in [0, 1). */
export function backoffDelay(failures: number, rand: () => number = Math.random): number {
  const step = Math.min(CAP_MS, BASE_MS * 2 ** Math.max(0, failures - 1))
  return Math.round(step * (1 - JITTER + 2 * JITTER * rand()))
}

/** Is the person still signed in? A failed probe (the api is down) is "yes,
 *  as far as we know" — that is exactly the outage reconnecting rides out.
 *  Only the session route's own answer, `{user: null}`, is "signed out". */
async function stillSignedIn(): Promise<boolean> {
  try {
    const body = await getJson<{ user?: unknown }>('/api/auth/session')
    return body.user !== null
  } catch {
    return true
  }
}

/** Subscribe to an SSE endpoint. Returns the unsubscribe — call it from the
 *  effect's teardown, where the four call sites used to write `es.close()`.
 *  `url` may be a getter: it is read at every (re)connect, so a reconnect can
 *  carry a fresh cursor (`?since=`). */
export function openStream(
  url: string | (() => string),
  onMessage: (data: string) => void,
  opts: StreamOptions = {},
): () => void {
  let es: EventSource | null = null
  let stopped = false
  let first = true
  let failures = 0
  let retryTimer: ReturnType<typeof setTimeout> | null = null
  let stableTimer: ReturnType<typeof setTimeout> | null = null
  let hideTimer: ReturnType<typeof setTimeout> | null = null
  let paused = false
  let resumed = false
  let state: StreamState | null = null
  const pauseMs =
    typeof opts.pauseWhenHidden === 'number' ? opts.pauseWhenHidden : opts.pauseWhenHidden ? hiddenPauseMs() : 0
  const setState = (s: StreamState) => {
    if (s === state) return
    state = s
    opts.onState?.(s)
  }

  const clearTimers = () => {
    if (retryTimer) clearTimeout(retryTimer)
    if (stableTimer) clearTimeout(stableTimer)
    retryTimer = null
    stableTimer = null
  }

  const connect = () => {
    // One source at a time. A kick() (online, the tab back in view) while the
    // fifth failure's session probe is in flight connects; the probe's own
    // answer must not open a SECOND source — stop() would never close the
    // first, and every frame would be dispatched twice.
    if (stopped || es || paused) return
    clearTimers()
    const source = new EventSource(typeof url === 'function' ? url() : url)
    es = source
    setState(first ? 'connecting' : 'reconnecting')
    source.onmessage = (e: MessageEvent<string>) => onMessage(e.data)
    source.onopen = () => {
      if (es !== source) return
      const wasFirst = first
      const wasResumed = resumed
      first = false
      resumed = false
      setState('open')
      if (opts.reconnect) {
        if (stableTimer) clearTimeout(stableTimer)
        stableTimer = setTimeout(() => {
          failures = 0
        }, STABLE_MS)
      }
      opts.onOpen?.(wasResumed ? { first: wasFirst, resumed: true } : { first: wasFirst })
    }
    source.onerror = () => {
      if (es !== source) return
      if (!opts.reconnect) {
        if (source.readyState === EventSource.CLOSED) setState('closed')
        else setState('reconnecting')
        return
      }
      // CONNECTING is the browser's own retry (the stream ended cleanly and
      // `retry:` is counting down) — let it. CLOSED is a non-200 (502, 524,
      // 401): the browser has given up, so this door takes over.
      if (source.readyState !== EventSource.CLOSED) {
        setState('reconnecting')
        return
      }
      source.close()
      es = null
      if (stableTimer) clearTimeout(stableTimer)
      stableTimer = null
      failures++
      setState('reconnecting')
      schedule()
    }
  }

  // ── The hidden-tab pause ──────────────────────────────────────────────────
  const pause = () => {
    hideTimer = null
    if (stopped || paused) return
    paused = true
    clearTimers()
    es?.close()
    es = null
    // A paused stream is not a failing one: the next open starts afresh.
    failures = 0
    setState('paused')
  }
  const resume = () => {
    if (!paused || stopped) return
    paused = false
    resumed = true
    let ran: unknown
    try {
      ran = opts.onResume?.()
    } catch {
      ran = undefined
    }
    // Reopen after the caller's refetch has gone out (and, when it answers a
    // promise, after it settles) — what the stream missed is read first.
    void Promise.resolve(ran)
      .catch(() => {})
      .then(() => connect())
  }
  const hidden = () => typeof document !== 'undefined' && document.visibilityState === 'hidden'
  const armPause = () => {
    if (!pauseMs || paused || hideTimer || stopped) return
    hideTimer = setTimeout(pause, pauseMs)
  }

  const schedule = () => {
    if (stopped || retryTimer || paused) return
    const wait = backoffDelay(failures)
    retryTimer = setTimeout(() => {
      retryTimer = null
      if (failures >= PROBE_AFTER && failures % PROBE_AFTER === 0) {
        void stillSignedIn().then((ok) => {
          if (stopped) return
          if (ok) connect()
          else stop()
        })
        return
      }
      connect()
    }, wait)
  }

  /** Reconnect NOW if the stream is down and waiting: the network came back,
   *  or the person came back to the tab. */
  const kick = () => {
    if (stopped || es || paused) return
    if (retryTimer) clearTimeout(retryTimer)
    retryTimer = null
    connect()
  }
  const onVisible = () => {
    if (pauseMs) {
      if (hidden()) armPause()
      else {
        if (hideTimer) clearTimeout(hideTimer)
        hideTimer = null
        if (paused) return resume()
      }
    }
    if (opts.reconnect && document.visibilityState === 'visible') kick()
  }

  const watchVisibility = (opts.reconnect || pauseMs > 0) && typeof window !== 'undefined'
  const stop = () => {
    stopped = true
    clearTimers()
    if (hideTimer) clearTimeout(hideTimer)
    hideTimer = null
    es?.close()
    es = null
    setState('closed')
    if (watchVisibility) {
      if (opts.reconnect) window.removeEventListener('online', kick)
      document.removeEventListener('visibilitychange', onVisible)
    }
  }

  if (watchVisibility) {
    if (opts.reconnect) window.addEventListener('online', kick)
    document.addEventListener('visibilitychange', onVisible)
  }
  connect()
  // Opened in a tab that is already hidden (a background tab's first load):
  // the clock starts now.
  if (pauseMs && hidden()) armPause()
  return stop
}
