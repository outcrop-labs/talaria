// WHO IS TYPING, live — "Maya is typing…" and the dots beside a name
// in the Comms rail.
//
// Typing is ephemeral presence: nothing is stored server-side. A composer
// tells the server (POST /api/channels/:id/typing) at most every PING_MS while
// its text changes, and `typing: false` when the text is sent or cleared. The
// server fans that out on the channel's stream (the open conversation or
// thread panel) and, for channel-level typing, on every other member's own
// stream (their rail). Listeners feed both into this one table, keyed by
// `typingKey`: the channel id, or `<channelId>#<threadRootId>` for a thread.
//
// A `typing: true` that is never followed by `false` (a closed tab, a dropped
// connection) expires on its own after EXPIRE_MS — so the worst a lost signal
// costs is a few seconds of dots, never a stuck indicator.

import { postJson } from './fetch-json'

const PING_MS = 3000
const EXPIRE_MS = 6000

/** channelId → userId → expiry (epoch ms). Replaced, never mutated. */
let typers = $state<Record<string, Record<string, number>>>({})
let sweep: ReturnType<typeof setInterval> | null = null

function prune(): void {
  const now = Date.now()
  let changed = false
  const next: Record<string, Record<string, number>> = {}
  for (const [cid, users] of Object.entries(typers)) {
    const live = Object.entries(users).filter(([, at]) => at > now)
    if (live.length !== Object.keys(users).length) changed = true
    if (live.length) next[cid] = Object.fromEntries(live)
  }
  if (changed) typers = next
  if (Object.keys(next).length === 0 && sweep) {
    clearInterval(sweep)
    sweep = null
  }
}

/** The table key: a channel, or one thread in it. */
export const typingKey = (channelId: string, threadRootId?: string | null): string =>
  threadRootId ? `${channelId}#${threadRootId}` : channelId

/** Record a typing signal from either stream. */
export function setTyping(channelId: string, userId: string, typing: boolean): void {
  const users = { ...(typers[channelId] ?? {}) }
  if (typing) users[userId] = Date.now() + EXPIRE_MS
  else delete users[userId]
  const next = { ...typers }
  if (Object.keys(users).length) next[channelId] = users
  else delete next[channelId]
  typers = next
  if (!sweep && Object.keys(next).length) sweep = setInterval(prune, 1000)
}

/** Everyone typing in a channel right now, minus `selfId`. Reactive. */
export function typersIn(channelId: string | null | undefined, selfId: string | null | undefined): string[] {
  if (!channelId) return []
  const now = Date.now()
  return Object.entries(typers[channelId] ?? {})
    .filter(([uid, at]) => at > now && uid !== selfId)
    .map(([uid]) => uid)
}

/** "Maya is typing", "Maya and Jordan are typing", "Several people are typing". */
export function typingSentence(names: string[]): string {
  if (names.length === 0) return ''
  if (names.length === 1) return `${names[0]} is typing`
  if (names.length === 2) return `${names[0]} and ${names[1]} are typing`
  return 'Several people are typing'
}

const post = (channelId: string, threadRootId: string | null, typing: boolean) =>
  postJson<{ ok: true }>(
    `/api/channels/${encodeURIComponent(channelId)}/typing`,
    threadRootId ? { typing, threadRootId } : { typing },
  ).catch(() => {
    // Presence is best-effort; a dropped ping just expires on the other side.
  })

/** The composer's side: `input(empty)` on every keystroke, `stop()` on send,
 *  clear, or leaving the channel (or thread). Pings at most every PING_MS. */
export function createTypingSender(
  channelId: () => string | null | undefined,
  threadRootId: () => string | null | undefined = () => null,
) {
  let lastSent = 0
  let active: { id: string; root: string | null } | null = null
  const stop = () => {
    if (!active) return
    const { id, root } = active
    active = null
    lastSent = 0
    void post(id, root, false)
  }
  return {
    input(empty: boolean) {
      const id = channelId()
      const root = threadRootId() ?? null
      if (!id || empty) return stop()
      const same = active && active.id === id && active.root === root
      if (active && !same) stop()
      const now = Date.now()
      if (same && now - lastSent < PING_MS) return
      active = { id, root }
      lastSent = now
      void post(id, root, true)
    },
    stop,
  }
}
