// The tabs of one browser, coordinating who shows a notification.
//
// Two primitives every engine Talaria supports has:
//
//   · A Web Lock, `talaria-notify-leader`. Every tab asks for it; one holds it
//     for its whole life, and when that tab closes (or crashes, or is
//     discarded) the browser hands it to the next tab in line at once. The
//     holder is the LEADER: the one tab that posts an OS banner when nobody
//     is looking at Talaria.
//   · A BroadcastChannel, `talaria-notify`. Every tab announces
//     `{tabId, visible, focused, lastInputAt, at}` on every focus or
//     visibility change and every 10 s, so each tab knows whether the person
//     is looking at ANOTHER Talaria tab. A tab that toasts an arrival says
//     `claimed` for it, which the leader waits a moment for before it posts.
//
// Without either primitive every tab decides alone, as it always did — and
// the shared tag plus the bannered store (notify-bannered.ts) still collapse
// the duplicates. Pure wiring over injectable edges, so the protocol is a
// unit test.
//
// Wave 2 makes the leader the hub that owns the only `/api/me/events` and
// rebroadcasts it here; the channel name and message shapes are kept for it.

import type { TabState } from '@/lib/notify-decide'

export const LEADER_LOCK = 'talaria-notify-leader'
export const NOTIFY_CHANNEL = 'talaria-notify'
export const HEARTBEAT_MS = 10_000

export interface LockManagerLike {
  request(name: string, callback: (lock: unknown) => Promise<unknown>): Promise<unknown>
}

export interface ChannelLike {
  postMessage(message: unknown): void
  addEventListener(type: 'message', listener: (e: { data: unknown }) => void): void
  removeEventListener(type: 'message', listener: (e: { data: unknown }) => void): void
  close(): void
}

type Msg =
  | { type: 'state'; tab: TabState }
  | { type: 'claimed'; tabId: string; key: string }
  | { type: 'bye'; tabId: string }

export interface LeaderEnv {
  locks?: LockManagerLike | null
  channel?: ChannelLike | null
  /** This tab's view of itself, read at announce time. */
  self: () => { visible: boolean; focused: boolean }
  now?: () => number
  tabId?: string
  heartbeatMs?: number
}

export interface NotifyLeader {
  readonly tabId: string
  /** No lock manager or no channel: this tab decides on its own. */
  readonly alone: boolean
  isLeader(): boolean
  /** The other tabs' last announcements. */
  others(): TabState[]
  /** Announce this tab's state now (focus, visibility, a heartbeat). */
  announce(): void
  /** The person touched this tab. */
  noteInput(): void
  /** This tab has the arrival (it toasted it): tell the others. */
  claim(id: string, version: string): void
  /** Resolves true if another tab claims this arrival within `ms`. */
  waitForClaim(id: string, version: string, ms: number): Promise<boolean>
  /** Called whenever leadership changes (for the debug hook and tests). */
  onLeaderChange(fn: (leader: boolean) => void): () => void
  close(): void
}

const claimKey = (id: string, version: string) => `${id}@${version}`

const randomId = () =>
  typeof crypto !== 'undefined' && 'randomUUID' in crypto ? crypto.randomUUID() : `tab-${Math.random().toString(36).slice(2)}`

export function startNotifyLeader(env: LeaderEnv): NotifyLeader {
  const now = env.now ?? Date.now
  const tabId = env.tabId ?? randomId()
  const locks = env.locks ?? null
  const channel = env.channel ?? null
  const alone = !locks || !channel
  let leader = alone
  let closed = false
  let lastInputAt = 0
  let release: (() => void) | null = null
  const registry = new Map<string, TabState>()
  const claims = new Map<string, number>()
  const claimWaiters = new Map<string, Set<() => void>>()
  const leaderListeners = new Set<(leader: boolean) => void>()

  const setLeader = (v: boolean) => {
    if (leader === v) return
    leader = v
    for (const fn of leaderListeners) fn(v)
  }

  const state = (): TabState => ({ tabId, ...env.self(), lastInputAt, at: now() })
  const post = (m: Msg) => {
    if (closed || !channel) return
    try {
      channel.postMessage(m)
    } catch {
      /* a closed channel — the tab is going away */
    }
  }

  const onMessage = (e: { data: unknown }) => {
    const m = e.data as Msg | null
    if (!m || typeof m !== 'object' || typeof (m as { type?: unknown }).type !== 'string') return
    if (m.type === 'state') {
      if (!m.tab || m.tab.tabId === tabId) return
      const known = registry.has(m.tab.tabId)
      registry.set(m.tab.tabId, m.tab)
      // A tab we had not heard from: answer at once, so it learns of us now
      // rather than at our next heartbeat.
      if (!known) post({ type: 'state', tab: state() })
    } else if (m.type === 'bye') {
      registry.delete(m.tabId)
    } else if (m.type === 'claimed') {
      if (m.tabId === tabId) return
      claims.set(m.key, now())
      for (const fn of claimWaiters.get(m.key) ?? []) fn()
    }
  }

  let beat: ReturnType<typeof setInterval> | null = null
  if (channel) {
    channel.addEventListener('message', onMessage)
    beat = setInterval(() => post({ type: 'state', tab: state() }), env.heartbeatMs ?? HEARTBEAT_MS)
    post({ type: 'state', tab: state() })
  }

  if (locks && channel) {
    // Held for the tab's lifetime: the callback's promise only settles when
    // this tab closes the runtime, and the browser releases it on its own
    // when the page goes away.
    void locks
      .request(LEADER_LOCK, () => {
        if (closed) return Promise.resolve()
        setLeader(true)
        return new Promise<void>((resolve) => {
          release = resolve
        })
      })
      .catch(() => {
        // A lock manager that refuses (an opaque origin, a policy): decide
        // alone rather than never.
        setLeader(true)
      })
  }

  // Old claims are noise: anything older than a minute is dropped on read.
  const fresh = (key: string) => {
    const t = claims.get(key)
    if (t === undefined) return false
    if (now() - t > 60_000) {
      claims.delete(key)
      return false
    }
    return true
  }

  return {
    tabId,
    alone,
    isLeader: () => leader,
    others: () => [...registry.values()],
    announce: () => post({ type: 'state', tab: state() }),
    noteInput: () => {
      lastInputAt = now()
    },
    claim: (id, version) => {
      const key = claimKey(id, version)
      claims.set(key, now())
      post({ type: 'claimed', tabId, key })
    },
    waitForClaim: (id, version, ms) => {
      const key = claimKey(id, version)
      if (fresh(key)) return Promise.resolve(true)
      if (alone) return Promise.resolve(false)
      return new Promise<boolean>((resolve) => {
        const set = claimWaiters.get(key) ?? new Set<() => void>()
        claimWaiters.set(key, set)
        const done = (v: boolean) => {
          clearTimeout(timer)
          set.delete(hit)
          if (set.size === 0) claimWaiters.delete(key)
          resolve(v)
        }
        const hit = () => done(true)
        const timer = setTimeout(() => done(false), ms)
        set.add(hit)
      })
    },
    onLeaderChange: (fn) => {
      leaderListeners.add(fn)
      return () => leaderListeners.delete(fn)
    },
    close: () => {
      if (closed) return
      post({ type: 'bye', tabId })
      closed = true
      if (beat) clearInterval(beat)
      channel?.removeEventListener('message', onMessage)
      try {
        channel?.close()
      } catch {
        /* already closed */
      }
      release?.()
      release = null
      setLeader(false)
    },
  }
}

/** The browser's own edges, or null where an engine lacks one. */
export function browserLeaderEdges(): { locks: LockManagerLike | null; channel: ChannelLike | null } {
  const locks =
    typeof navigator !== 'undefined' && 'locks' in navigator && navigator.locks
      ? (navigator.locks as unknown as LockManagerLike)
      : null
  let channel: ChannelLike | null = null
  try {
    channel = typeof BroadcastChannel !== 'undefined' ? (new BroadcastChannel(NOTIFY_CHANNEL) as unknown as ChannelLike) : null
  } catch {
    channel = null
  }
  return { locks, channel }
}
