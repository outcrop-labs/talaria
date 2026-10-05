// The global presence heartbeat (KTD6). While Talaria is open, the client
// pings `PUT /api/me/presence`; the server keeps a 90s TTL key per person and
// the directory reports `online` from it. Mounted once, by AppLayout.
//
// VISIBILITY GATE: by default the heartbeat pings only while the tab is
// visible — on mount, every 30s, and once on regaining visibility — so
// "online" means "had Talaria in a visible tab within 90 seconds". The cost
// is that someone with Talaria in a background tab reads as offline. That is
// one flag (`visibleOnly`); flip it to keep pinging from hidden tabs.
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
  /** Ping only while the tab is visible. Default true (the plan's rule). */
  visibleOnly?: boolean
  intervalMs?: number
}

const defaultPing = () => putJson<{ ok: true }>('/api/me/presence')

/** Start the heartbeat; returns the stop function. */
export function startPresenceHeartbeat(opts: PresenceOptions = {}): () => void {
  const doc = opts.doc ?? document
  const ping = opts.ping ?? defaultPing
  const visibleOnly = opts.visibleOnly ?? true
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
