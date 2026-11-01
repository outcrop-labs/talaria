// WHICH DEVICE IS THE PERSON ON — this tab's half of cross-device presence
// (I-17), and the ack that goes with it (I-18).
//
// Every Talaria tab reports itself on `PUT /api/me/presence`:
// `{clientId, kind, state, osIdleMs?}`. The server keeps the person's clients
// for 75 s and names a WINNER — the most recent client that is `looking`,
// else the most recent `active` one — and publishes it as a `presence` frame
// whenever it changes (and answers it on every heartbeat). The decider
// (notify-decide.ts) reads it: when the winner is a client of ANOTHER device,
// this browser stays quiet, because the person is over there.
//
// THE STATE IS COMPUTED HERE, not on the server, from what only the page
// knows (O-13):
//   · looking — shown, focused, and touched in the last 60 s;
//   · active  — shown and touched in the last 120 s (a window on a second
//               monitor); inside the desktop shell, OS input in the last
//               120 s, window or no window (the person is at that machine);
//   · idle    — shown, untouched;
//   · hidden  — not shown.
// It is sent on every change (at most one beat per 5 s) and every 30 s, so a
// person who walks away goes `idle` within ~2 minutes and their other device
// takes over.
//
// THE ACK. A tab that toasted a row to a looking person, or posted an OS
// banner for it, tells the server it surfaced (`POST /api/notifications/ack`,
// batched for 500 ms): the server's deferred push (W2-E) fires only for rows
// still unread and un-acked, so the phone does not buzz for what the laptop
// already showed.
//
// Framework-free over injected edges: the state table, the schedule and the
// batcher are unit tests (notify-presence.test.ts).

import { findCurrentInstance } from './desktop-instance'
import { ACTIVE_INPUT_MS, LOOKING_INPUT_MS } from './looking'
import type { ClientKind, ClientState, PresenceBody } from './notify-wire'

/** A beat at least this often, changed or not (the server prunes at 75 s). */
export const BEAT_MS = 30_000
/** At most one beat per this long, however often the state flips. */
export const MIN_GAP_MS = 5_000
/** How often the state is re-evaluated (time alone moves looking → active → idle). */
export const TICK_MS = 5_000
/** The per-tab client id's sessionStorage key. */
export const CLIENT_KEY = 'talaria.notify.client'

export interface ClientSignals {
  visible: boolean
  focused: boolean
  /** Last input in this page, ms epoch (0 = none). */
  lastInputAt: number
  now: number
  kind: ClientKind
  /** The desktop shell's OS idle time, when it can say (I-22). */
  osIdleMs?: number | null
}

/** The I-17 state of one client. Pure. */
export function clientState(s: ClientSignals): ClientState {
  const pageAge = s.lastInputAt > 0 ? s.now - s.lastInputAt : Number.POSITIVE_INFINITY
  // Inside the shell the OS knows about input everywhere, the page only about
  // its own: the freshest of the two is how long the person has been away.
  const os = s.kind === 'desktop' && typeof s.osIdleMs === 'number' && s.osIdleMs >= 0 ? s.osIdleMs : null
  const age = os === null ? pageAge : Math.min(pageAge, os)
  if (s.visible && s.focused && age <= LOOKING_INPUT_MS) return 'looking'
  if (os !== null) {
    if (os <= ACTIVE_INPUT_MS) return 'active'
  } else if (s.visible && pageAge <= ACTIVE_INPUT_MS) return 'active'
  return s.visible ? 'idle' : 'hidden'
}

// ── The client id ────────────────────────────────────────────────────────────

export interface IdStore {
  get(key: string): string | null
  set(key: string, value: string): void
}

const sessionStore = (): IdStore | null => {
  try {
    const s = typeof sessionStorage === 'undefined' ? null : sessionStorage
    if (!s) return null
    return { get: (k) => s.getItem(k), set: (k, v) => s.setItem(k, v) }
  } catch {
    return null
  }
}

const VALID_ID = /^[A-Za-z0-9_-]{8,64}$/

/** A fresh id in the server's alphabet (`[A-Za-z0-9_-]{8,64}`). */
export function newClientId(rand: (n: number) => Uint8Array = defaultRandom): string {
  const bytes = rand(16)
  let s = ''
  for (const b of bytes) s += String.fromCharCode(b)
  const b64 = btoa(s)
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '')
  return `t${b64}`
}

function defaultRandom(n: number): Uint8Array {
  const out = new Uint8Array(n)
  if (typeof crypto !== 'undefined' && 'getRandomValues' in crypto) crypto.getRandomValues(out)
  else for (let i = 0; i < n; i++) out[i] = Math.floor(Math.random() * 256)
  return out
}

let memoId: string | null = null
const idListeners = new Set<(id: string) => void>()

/** This tab's client id: stable across reloads of the tab (sessionStorage),
 *  new for a new tab. Without storage, stable for the page's life.
 *
 *  A DUPLICATED tab starts with a copy of its original's sessionStorage (the
 *  browser's "Duplicate tab", or a page opened with an opener), so both tabs
 *  would report as one client and overwrite each other's state on the
 *  server — the person looking at one twin reads as `hidden` whenever the
 *  other beats last. The tab registry spots the twin (notify-leader.ts) and
 *  one of the two calls `remintClientId`. */
export function clientId(store: IdStore | null = sessionStore()): string {
  if (memoId) return memoId
  let id: string | null = null
  try {
    id = store?.get(CLIENT_KEY) ?? null
  } catch {
    id = null
  }
  if (!id || !VALID_ID.test(id)) {
    id = newClientId()
    try {
      store?.set(CLIENT_KEY, id)
    } catch {
      /* the id lives for this page only */
    }
  }
  memoId = id
  return id
}

/** Take a fresh client id for this tab (its twin keeps the old one), and
 *  tell whoever reports it (the heartbeat beats under the new id at once). */
export function remintClientId(store: IdStore | null = sessionStore()): string {
  const id = newClientId()
  try {
    store?.set(CLIENT_KEY, id)
  } catch {
    /* the id lives for this page only */
  }
  memoId = id
  for (const fn of idListeners) fn(id)
  return id
}

/** Hear this tab's client id change (a re-mint). Returns the unsubscribe. */
export function onClientIdChange(fn: (id: string) => void): () => void {
  idListeners.add(fn)
  return () => idListeners.delete(fn)
}

/** Test hook. */
export function resetClientIdForTests(): void {
  memoId = null
  idListeners.clear()
}

// ── The desktop shell's own watcher ──────────────────────────────────────────

/** The presence client the desktop shell's watcher beats under for THIS
 *  page's instance: `desk-<local instance id>` (desktop/src-tauri/src/
 *  watch.rs `client_id`, where the local id is the shell registry entry's
 *  `id`, the suffix of the webview's `instance-<id>` label). The entry is
 *  this page's instance by the beacon's instance id, else by origin
 *  (desktop-instance.ts `findCurrentInstance`, as the switcher finds it).
 *  The watcher beats while the page is suspended, and its last `active`
 *  outlives a restore by up to 75 s: the decider counts it as this device
 *  (notify-decide.ts `sameDevice`). Null when the shell does not list this
 *  instance, or its id is not one the store takes. */
export function deskClientId(
  instances: ReadonlyArray<{ id: string; instanceId: string; url: string }>,
  beaconId: string | null | undefined,
  origin: string,
): string | null {
  const hit = findCurrentInstance([...instances], beaconId, origin)
  if (!hit?.id) return null
  const id = `desk-${hit.id}`
  return VALID_ID.test(id) ? id : null
}

// ── The presence view: who the server says the person is on ─────────────────

export interface PresenceView {
  looking: string | null
  active: string | null
  clients: number
  /** When this tab last heard it (ms), 0 = never. */
  at: number
}

let view: PresenceView = { looking: null, active: null, clients: 0, at: 0 }
const viewListeners = new Set<(v: PresenceView) => void>()

/** The last word on the winner: a `presence` frame, a heartbeat's answer, or
 *  `GET /api/me/presence`. */
export function notePresence(p: { looking?: string | null; active?: string | null; clients?: number }, at = Date.now()): void {
  const next: PresenceView = {
    looking: p.looking ?? null,
    active: p.active ?? null,
    clients: typeof p.clients === 'number' ? p.clients : view.clients,
    at,
  }
  const changed = next.looking !== view.looking || next.active !== view.active || next.clients !== view.clients
  view = next
  if (changed) for (const fn of viewListeners) fn(view)
}

export function presenceView(): PresenceView {
  return view
}

export function onPresenceView(fn: (v: PresenceView) => void): () => void {
  viewListeners.add(fn)
  return () => viewListeners.delete(fn)
}

/** The client the person is on, or null: looking first, else active (I-17). */
export function presenceWinner(v: PresenceView = view): string | null {
  return v.looking ?? v.active ?? null
}

/** Test hook. */
export function resetPresenceForTests(): void {
  view = { looking: null, active: null, clients: 0, at: 0 }
  viewListeners.clear()
}

// ── The heartbeat ────────────────────────────────────────────────────────────

export interface HeartbeatEnv {
  /** The client id, or a getter read at each beat (it can be re-minted). */
  clientId: string | (() => string)
  kind: ClientKind
  /** The page's own signals, read at each evaluation. */
  read: () => { visible: boolean; focused: boolean; lastInputAt: number }
  /** The shell's OS idle time (I-22); omitted or null → page signals only. */
  osIdle?: () => Promise<number | null>
  send: (body: PresenceBody) => Promise<unknown>
  now?: () => number
  beatMs?: number
  minGapMs?: number
  tickMs?: number
}

export interface Heartbeat {
  /** Re-evaluate now (a focus, a visibility change, an input); `force`
   *  beats even if the state did not change (a re-minted id). */
  poke(force?: boolean): void
  /** The state as last computed. */
  state(): ClientState | null
  stop(): void
}

/** Start beating: once at once, on every state change (≤ 1 per `minGapMs`),
 *  and every `beatMs`. A failed send is one missed beat. */
export function startHeartbeat(env: HeartbeatEnv): Heartbeat {
  const now = env.now ?? Date.now
  const beatMs = env.beatMs ?? BEAT_MS
  const minGap = env.minGapMs ?? MIN_GAP_MS
  const tickMs = env.tickMs ?? TICK_MS
  let stopped = false
  let last: ClientState | null = null
  let sentState: ClientState | null = null
  let sentAt = Number.NEGATIVE_INFINITY
  let pending: ReturnType<typeof setTimeout> | null = null
  let evaluating = false
  let again = false

  const idOf = () => (typeof env.clientId === 'function' ? env.clientId() : env.clientId)
  const send = (state: ClientState, osIdleMs: number | null) => {
    sentState = state
    sentAt = now()
    const body: PresenceBody = { clientId: idOf(), kind: env.kind, state }
    if (typeof osIdleMs === 'number') body.osIdleMs = Math.max(0, Math.round(osIdleMs))
    void env.send(body).catch(() => {})
  }

  let forced = false
  const evaluate = async (force = false) => {
    if (stopped) return
    if (force) forced = true
    if (evaluating) {
      again = true
      return
    }
    evaluating = true
    try {
      let osIdleMs: number | null = null
      if (env.osIdle) {
        try {
          osIdleMs = await env.osIdle()
        } catch {
          osIdleMs = null
        }
      }
      if (stopped) return
      const sig = env.read()
      const state = clientState({ ...sig, now: now(), kind: env.kind, osIdleMs })
      last = state
      const t = now()
      const due = forced || state !== sentState || t - sentAt >= beatMs
      if (!due) return
      const wait = sentAt + minGap - t
      if (wait > 0) {
        // Changed too soon after the last beat: send when the gap opens, with
        // whatever the state is then.
        if (!pending) {
          pending = setTimeout(() => {
            pending = null
            void evaluate()
          }, wait)
        }
        return
      }
      forced = false
      send(state, osIdleMs)
    } finally {
      evaluating = false
      if (again && !stopped) {
        again = false
        void evaluate()
      }
    }
  }

  void evaluate()
  const tick = setInterval(() => void evaluate(), tickMs)
  return {
    poke: (force) => void evaluate(!!force),
    state: () => last,
    stop: () => {
      stopped = true
      clearInterval(tick)
      if (pending) clearTimeout(pending)
      pending = null
    },
  }
}

// ── The ack (I-18) ───────────────────────────────────────────────────────────

const ROW_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
/** The route's own cap per call. */
export const ACK_MAX = 200
export const ACK_DELAY_MS = 500

export interface AckBatcher {
  /** Queue these row ids (anything that is not a row id is dropped). */
  add(ids: readonly string[]): void
  /** Send what is queued now. */
  flush(): Promise<void>
  /** What is queued (tests). */
  queued(): string[]
}

/** Batch acks for `delayMs`; each id is acked once per page. */
export function createAckBatcher(post: (ids: string[]) => Promise<unknown>, delayMs = ACK_DELAY_MS): AckBatcher {
  const queue = new Set<string>()
  const done = new Set<string>()
  let timer: ReturnType<typeof setTimeout> | null = null
  const flush = async () => {
    if (timer) clearTimeout(timer)
    timer = null
    const ids = [...queue]
    queue.clear()
    for (let i = 0; i < ids.length; i += ACK_MAX) {
      const chunk = ids.slice(i, i + ACK_MAX)
      for (const id of chunk) done.add(id)
      try {
        await post(chunk)
      } catch {
        // An ack that did not land costs one push the person did not need;
        // retrying it would cost more than it saves.
      }
    }
    if (done.size > 2000) done.clear()
  }
  return {
    add: (ids) => {
      for (const id of ids) if (ROW_ID.test(id) && !done.has(id)) queue.add(id)
      if (queue.size && !timer) timer = setTimeout(() => void flush(), delayMs)
    },
    flush,
    queued: () => [...queue],
  }
}
