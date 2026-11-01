// The global presence heartbeat (KTD6, I-17). While Talaria is open, every
// tab reports itself on `PUT /api/me/presence`; mounted once, by AppLayout.
//
// Since the notification contract the beat carries a BODY — `{clientId,
// kind, state, osIdleMs?}` — computed by notify-presence.ts: which tab this
// is, whether it is the browser or the desktop shell, and whether the person
// is looking at it, active on it, idle, or elsewhere. The server keeps the
// person's clients, names the one they are on, and announces it (the
// decider reads that: only the device you are on alerts). The same route
// still sets the directory's `online` key — for a web tab unless it is
// hidden, and for the shell only while its OS is in use — so "online" keeps
// meaning what it meant. The old no-body ping is gone.
//
// BACKGROUND TABS STILL BEAT (as `hidden`): the store holds every client the
// person has open, and a hidden tab must not vanish from it.
//
// Sent at once, on every state change (at most one per 5 s), and every 30 s.
import { untrack } from 'svelte'
import { putJson } from '@/lib/fetch-json'
import { inDesktopShell, shellOsIdleMs } from '@/lib/desktop-shell'
import { lastInputAt, onInput, trackInput } from '@/lib/looking'
import { clientId, notePresence, onClientIdChange, presenceView, startHeartbeat, BEAT_MS, type Heartbeat } from '@/lib/notify-presence'
import { setPresence } from '@/lib/notify-status.svelte'
import type { PresenceBody } from '@/lib/notify-wire'

export const PRESENCE_INTERVAL_MS = BEAT_MS

/** The slice of `document` the heartbeat reads — injectable for tests. */
export interface VisibilitySource {
  visibilityState: DocumentVisibilityState
  hasFocus?: () => boolean
  addEventListener(type: 'visibilitychange', fn: () => void): void
  removeEventListener(type: 'visibilitychange', fn: () => void): void
}

export interface PresenceOptions {
  doc?: VisibilitySource
  /** Where the beat goes (tests). Default: `PUT /api/me/presence` with the body. */
  send?: (body: PresenceBody) => Promise<unknown>
  /** The last beat, as the page goes away (tests). Default: the same PUT
   *  with `keepalive`, so it outlives the page. */
  sendFinal?: (body: PresenceBody) => void
  /** Window events (focus/blur/pageshow); tests pass a stub (and then the
   *  page's input listeners are not installed). */
  win?: EventTarget
  intervalMs?: number
  now?: () => number
  /** The shell's OS idle source; default `shellOsIdleMs` inside the shell. */
  osIdle?: (() => Promise<number | null>) | null
  kind?: 'web' | 'desktop'
  clientId?: string
}

const defaultSend = async (body: PresenceBody) => {
  const res = await putJson<{ ok: true; looking?: string | null; active?: string | null }>('/api/me/presence', body)
  // The answer names the winner as of this beat: a tab keeps its view fresh
  // without waiting for a frame (frames come only when the winner changes).
  if (res && ('looking' in res || 'active' in res)) {
    notePresence({ looking: res.looking ?? null, active: res.active ?? null })
    const v = presenceView()
    untrack(() => setPresence({ looking: v.looking, active: v.active, clients: v.clients, self: body.clientId }))
  }
  return res
}

const defaultFinal = (body: PresenceBody) => {
  void putJson('/api/me/presence', body, { keepalive: true }).catch(() => {})
}

function safeFocus(doc: VisibilitySource): boolean {
  try {
    return doc.hasFocus ? doc.hasFocus() : true
  } catch {
    return false
  }
}

/** Start the heartbeat; returns the stop function. */
export function startPresenceHeartbeat(opts: PresenceOptions = {}): () => void {
  const doc = opts.doc ?? document
  const win = opts.win ?? (typeof window === 'undefined' ? undefined : window)
  const shell = inDesktopShell()
  const kind = opts.kind ?? (shell ? 'desktop' : 'web')
  // Read at each beat: a duplicated tab re-mints its id (notify-presence.ts).
  const id = () => opts.clientId ?? clientId()
  // Untracked: the setter reads the status it writes, and this runs inside
  // AppLayout's $effect.
  untrack(() => setPresence({ self: id() }))
  const releaseInput = opts.win ? () => {} : trackInput()
  const hb: Heartbeat = startHeartbeat({
    clientId: id,
    kind,
    read: () => {
      const visible = doc.visibilityState === 'visible'
      return { visible, focused: visible && safeFocus(doc), lastInputAt: lastInputAt() }
    },
    osIdle: opts.osIdle === undefined ? (shell ? shellOsIdleMs : undefined) : (opts.osIdle ?? undefined),
    send: opts.send ?? defaultSend,
    now: opts.now,
    beatMs: opts.intervalMs,
  })
  const poke = () => hb.poke()
  // THE TAB GOES AWAY (closed, navigated off, put in the back/forward cache):
  // a last beat says `hidden`, so the server's winner moves to the person's
  // other device now rather than when this entry ages out (75 s). A page
  // restored from the cache beats again on `pageshow`.
  const bye = () => (opts.sendFinal ?? defaultFinal)({ clientId: id(), kind, state: 'hidden' })
  // A re-minted id beats at once, under its new name (the twin keeps the old).
  const offId = onClientIdChange((next) => {
    untrack(() => setPresence({ self: next }))
    hb.poke(true)
  })
  doc.addEventListener('visibilitychange', poke)
  win?.addEventListener('focus', poke)
  win?.addEventListener('blur', poke)
  win?.addEventListener('pageshow', poke)
  win?.addEventListener('pagehide', bye)
  // Input moves the state only toward looking; the heartbeat's own rate limit
  // keeps a typing person from beating more than once per 5 s.
  const offInput = onInput(() => {
    if (hb.state() !== 'looking') poke()
  })
  return () => {
    hb.stop()
    offId()
    offInput()
    releaseInput()
    doc.removeEventListener('visibilitychange', poke)
    win?.removeEventListener('focus', poke)
    win?.removeEventListener('blur', poke)
    win?.removeEventListener('pageshow', poke)
    win?.removeEventListener('pagehide', bye)
  }
}

/** Mount the heartbeat for the component's lifetime. Call during init. */
export function usePresenceHeartbeat(opts?: PresenceOptions): void {
  $effect(() => startPresenceHeartbeat(opts))
}
