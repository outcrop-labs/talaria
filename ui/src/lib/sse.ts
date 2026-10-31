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

import { getJson } from '@/lib/fetch-json'

export interface StreamOptions {
  /** Reopen a CLOSED stream with backoff (see above). */
  reconnect?: boolean
  /** Called on every open; `first` is the initial connection. */
  onOpen?: (info: { first: boolean }) => void
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
 *  effect's teardown, where the four call sites used to write `es.close()`. */
export function openStream(
  url: string,
  onMessage: (data: string) => void,
  opts: StreamOptions = {},
): () => void {
  let es: EventSource | null = null
  let stopped = false
  let first = true
  let failures = 0
  let retryTimer: ReturnType<typeof setTimeout> | null = null
  let stableTimer: ReturnType<typeof setTimeout> | null = null

  const clearTimers = () => {
    if (retryTimer) clearTimeout(retryTimer)
    if (stableTimer) clearTimeout(stableTimer)
    retryTimer = null
    stableTimer = null
  }

  const connect = () => {
    if (stopped) return
    clearTimers()
    const source = new EventSource(url)
    es = source
    source.onmessage = (e: MessageEvent<string>) => onMessage(e.data)
    source.onopen = () => {
      if (es !== source) return
      const wasFirst = first
      first = false
      if (opts.reconnect) {
        if (stableTimer) clearTimeout(stableTimer)
        stableTimer = setTimeout(() => {
          failures = 0
        }, STABLE_MS)
      }
      opts.onOpen?.({ first: wasFirst })
    }
    if (!opts.reconnect) return
    source.onerror = () => {
      if (es !== source) return
      // CONNECTING is the browser's own retry (the stream ended cleanly and
      // `retry:` is counting down) — let it. CLOSED is a non-200 (502, 524,
      // 401): the browser has given up, so this door takes over.
      if (source.readyState !== EventSource.CLOSED) return
      source.close()
      es = null
      if (stableTimer) clearTimeout(stableTimer)
      stableTimer = null
      failures++
      schedule()
    }
  }

  const schedule = () => {
    if (stopped || retryTimer) return
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
    if (stopped || es) return
    if (retryTimer) clearTimeout(retryTimer)
    retryTimer = null
    connect()
  }
  const onVisible = () => {
    if (document.visibilityState === 'visible') kick()
  }

  const stop = () => {
    stopped = true
    clearTimers()
    es?.close()
    es = null
    if (opts.reconnect && typeof window !== 'undefined') {
      window.removeEventListener('online', kick)
      document.removeEventListener('visibilitychange', onVisible)
    }
  }

  if (opts.reconnect && typeof window !== 'undefined') {
    window.addEventListener('online', kick)
    document.addEventListener('visibilitychange', onVisible)
  }
  connect()
  return stop
}
