// ONE `/api/me/events` PER BROWSER. The person's firehose used to be one
// EventSource per tab — five tabs, five sockets, five Redis subscribers on
// the server, and on a plain-http dev origin five of the six connections the
// browser allows a host (BT-3). Now the tab that holds the leader lock
// (notify-leader.ts) is the HUB: it alone opens the stream and rebroadcasts
// every frame on the `talaria-notify` BroadcastChannel; every other tab is a
// FOLLOWER and hears the frames there instead of opening its own.
//
//   · The leader opens the stream (with `?since=` the arrival record's
//     cursor, so a reconnect replays what it missed, I-19). Each frame is
//     delivered to its own listeners and posted as `hub:frame`. Each open is
//     posted as `hub:open`, and its connection state as `hub:state`.
//   · A follower says `hub:hello`; the leader answers with its state. The
//     follower's first word from a hub is its "first open"; a later
//     `hub:open` is a reopen (refetch what the gap may have dropped), and so
//     is a frozen tab's `resume`.
//   · The leader goes away (its tab closes, crashes, is discarded): the lock
//     passes to the next tab at once, which opens the stream itself and says
//     `hub:open` — every tab refetches, and nothing is lost (the arrival
//     record's `?since` read is exact).
//   · No hub answers within 2.5 s (a tab of an older build holds the lock
//     during a deploy, say): this tab opens its own stream — wave 1's shape —
//     until a hub speaks up or it leads itself.
//   · No Web Locks or no BroadcastChannel: every tab opens its own (wave 1).
//
// Framework-free over injected edges; the protocol is a unit test.

import type { ExtMessage, NotifyLeader } from './notify-leader'

export type HubRole = 'leader' | 'follower' | 'solo'
export type HubStreamState = 'connecting' | 'open' | 'reconnecting' | 'paused' | 'closed'

/** How long a follower waits to hear from a hub before it opens its own. */
export const FOLLOWER_WAIT_MS = 2_500

export interface HubStreamCallbacks {
  onOpen: (info: { first: boolean }) => void
  onState: (state: HubStreamState) => void
}

export interface HubEnv {
  leader: Pick<NotifyLeader, 'tabId' | 'alone' | 'isLeader' | 'onLeaderChange' | 'send' | 'onMessage'>
  /** Open this tab's own stream; returns its close. */
  open: (onFrame: (data: string) => void, cb: HubStreamCallbacks) => () => void
  followerWaitMs?: number
}

export interface Hub {
  role(): HubRole
  /** The stream's state as this tab knows it (its own, or its hub's). */
  state(): HubStreamState
  onFrame(fn: (data: string) => void): () => void
  onOpen(fn: (info: { first: boolean }) => void): () => void
  onRole(fn: (role: HubRole) => void): () => void
  onState(fn: (state: HubStreamState) => void): () => void
  /** A frozen page came back: refetch as after a reopen. */
  resumed(): void
  close(): void
}

type HubMsg =
  | { type: 'hub:frame'; from: string; data: string }
  | { type: 'hub:open'; from: string }
  | { type: 'hub:state'; from: string; state: HubStreamState; to?: string }
  | { type: 'hub:hello'; from: string }

export function startHub(env: HubEnv): Hub {
  const { leader } = env
  const wait = env.followerWaitMs ?? FOLLOWER_WAIT_MS
  const frameFns = new Set<(data: string) => void>()
  const openFns = new Set<(info: { first: boolean }) => void>()
  const roleFns = new Set<(role: HubRole) => void>()
  const stateFns = new Set<(s: HubStreamState) => void>()
  let role: HubRole = leader.alone ? 'solo' : leader.isLeader() ? 'leader' : 'follower'
  let state: HubStreamState = 'connecting'
  let closeStream: (() => void) | null = null
  let openedOnce = false
  let closed = false
  let watchdog: ReturnType<typeof setTimeout> | null = null

  const setRole = (r: HubRole) => {
    if (r === role) return
    role = r
    for (const fn of roleFns) fn(r)
  }
  const setState = (s: HubStreamState) => {
    if (s === state) return
    state = s
    for (const fn of stateFns) fn(s)
  }
  const deliver = (data: string) => {
    for (const fn of frameFns) fn(data)
  }
  const opened = () => {
    const first = !openedOnce
    openedOnce = true
    for (const fn of openFns) fn({ first })
  }
  const post = (m: HubMsg) => leader.send(m as unknown as ExtMessage)

  /** Open this tab's own stream (as the leader, or solo). */
  const own = () => {
    if (closeStream || closed) return
    closeStream = env.open(
      (data) => {
        deliver(data)
        if (role === 'leader') post({ type: 'hub:frame', from: leader.tabId, data })
      },
      {
        onOpen: () => {
          if (role === 'leader') post({ type: 'hub:open', from: leader.tabId })
          opened()
        },
        onState: (s) => {
          setState(s)
          if (role === 'leader') post({ type: 'hub:state', from: leader.tabId, state: s })
        },
      },
    )
  }
  const dropOwn = () => {
    closeStream?.()
    closeStream = null
  }
  const clearWatchdog = () => {
    if (watchdog) clearTimeout(watchdog)
    watchdog = null
  }

  const follow = () => {
    setRole('follower')
    post({ type: 'hub:hello', from: leader.tabId })
    clearWatchdog()
    watchdog = setTimeout(() => {
      watchdog = null
      // Nobody rebroadcasts: be your own stream, as wave 1 was.
      if (role === 'follower' && !closed) {
        setRole('solo')
        own()
      }
    }, wait)
  }

  const heardHub = () => {
    clearWatchdog()
    if (role === 'solo') {
      // A hub spoke up after this tab went its own way: rejoin it, and
      // refetch, since the switch can drop a frame.
      dropOwn()
      setRole('follower')
      if (openedOnce) for (const fn of openFns) fn({ first: false })
    }
  }

  const offMsg = leader.onMessage((raw) => {
    if (closed) return
    const m = raw as unknown as HubMsg
    if (m.from === leader.tabId) return
    if (m.type === 'hub:hello') {
      if (role === 'leader') post({ type: 'hub:state', from: leader.tabId, state, to: m.from })
      return
    }
    // Only a follower (or a solo tab looking for a hub) listens to a hub.
    if (role === 'leader') return
    if (m.type === 'hub:state') {
      if (m.to && m.to !== leader.tabId) return
      heardHub()
      setState(m.state)
      // Only the answer to this tab's hello counts as its open: a broadcast
      // `open` state is followed by the `hub:open` that says so.
      if (m.to === leader.tabId && m.state === 'open' && !openedOnce) opened()
    } else if (m.type === 'hub:open') {
      heardHub()
      setState('open')
      opened()
    } else if (m.type === 'hub:frame') {
      if (role === 'solo') return // its own stream carries the same frame
      clearWatchdog()
      deliver(m.data)
    }
  })

  const offLeader = leader.onLeaderChange((isLeader) => {
    if (closed) return
    if (isLeader) {
      clearWatchdog()
      setRole('leader')
      // A solo stream already open simply becomes the hub's; else open now.
      if (closeStream) post({ type: 'hub:state', from: leader.tabId, state })
      else own()
    } else if (role === 'leader') {
      dropOwn()
      follow()
    }
  })

  if (role === 'leader' || role === 'solo') own()
  else follow()

  return {
    role: () => role,
    state: () => state,
    onFrame: (fn) => {
      frameFns.add(fn)
      return () => frameFns.delete(fn)
    },
    onOpen: (fn) => {
      openFns.add(fn)
      return () => openFns.delete(fn)
    },
    onRole: (fn) => {
      roleFns.add(fn)
      return () => roleFns.delete(fn)
    },
    onState: (fn) => {
      stateFns.add(fn)
      return () => stateFns.delete(fn)
    },
    resumed: () => {
      if (closed || !openedOnce) return
      for (const fn of openFns) fn({ first: false })
    },
    close: () => {
      if (closed) return
      closed = true
      clearWatchdog()
      offMsg()
      offLeader()
      dropOwn()
      setState('closed')
    },
  }
}
