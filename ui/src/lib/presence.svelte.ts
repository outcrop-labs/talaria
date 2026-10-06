// The global presence heartbeat (KTD6). While Talaria is open, the client
// pings `PUT /api/me/presence`; the server keeps a 90s TTL key per person and
// the directory reports `online` from it. Mounted once, by AppLayout.
//
// BACKGROUND TABS COUNT: by default the heartbeat pings on mount and every 30s
// whether or not the tab is visible, so "online" means "had Talaria open
// within 90 seconds" — a teammate with Talaria behind another window still
// reads as online. `visibleOnly: true` narrows it to visible tabs (pings on
// mount, every 30s, and once on regaining visibility; hidden tabs skip).
import { putJson } from '@/lib/fetch-json'

export const PRESENCE_INTERVAL_MS = 30_000

/** The slice of `document` the heartbeat reads — injectable for tests. */
export interface VisibilitySource {
  visibilityState: DocumentVisibilityState
  addEventListener(type: 'visibilitychange', fn: () => void): void
  removeEventListener(type: 'visibilitychange', fn: () => void): void
}

export interface PresenceOptions {
  doc?: VisibilitySource
  ping?: () => Promise<unknown>
  /** Ping only while the tab is visible. Default false: background tabs ping. */
  visibleOnly?: boolean
  intervalMs?: number
}

const defaultPing = () => putJson<{ ok: true }>('/api/me/presence')

/** Start the heartbeat; returns the stop function. */
export function startPresenceHeartbeat(opts: PresenceOptions = {}): () => void {
  const doc = opts.doc ?? document
  const ping = opts.ping ?? defaultPing
  const visibleOnly = opts.visibleOnly ?? false
  const intervalMs = opts.intervalMs ?? PRESENCE_INTERVAL_MS

  const beat = () => {
    if (visibleOnly && doc.visibilityState !== 'visible') return
    // Presence is best-effort: a failed ping just means one missed beat.
    void ping().catch(() => {})
  }
  // Regaining visibility pings at once, so a returning person shows online
  // without waiting for the next tick.
  const onVisibility = () => {
    if (doc.visibilityState === 'visible') beat()
  }

  beat()
  const timer = setInterval(beat, intervalMs)
  if (visibleOnly) doc.addEventListener('visibilitychange', onVisibility)
  return () => {
    clearInterval(timer)
    if (visibleOnly) doc.removeEventListener('visibilitychange', onVisibility)
  }
}

/** Mount the heartbeat for the component's lifetime. Call during init. */
export function usePresenceHeartbeat(opts?: PresenceOptions): void {
  $effect(() => startPresenceHeartbeat(opts))
}
