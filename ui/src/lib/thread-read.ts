// What an open thread panel posts as read, and when (ThreadPanel.svelte).
//
// Two things the panel cannot get right from its rows alone:
//
// - A FULL PAGE IS THE THREAD'S OLDEST MESSAGES. The api serves a thread's
//   first THREAD_PAGE_CAP rows, root included, oldest first
//   (talaria-channels MSG_PAGE_THREAD). Past that, the last row is not the
//   newest reply, and a cursor at its seq never covers the replies after it:
//   the badge and the thread's bell row would stick for good. So a full page
//   posts NO seq, and the server reads to the thread's newest complete reply.
//   Its last seq stops moving, so it posts again on each refetch of the page
//   (each one may carry a reply past it).
// - A BURST OF REPLIES IS ONE READ. Every post costs a POST and two refetches
//   (the sidebar's ['channels'], the rail's ['unreads']): the rail's heaviest
//   queries. A reply that lands while the panel is open waits a beat, and the
//   newest read in a burst is the one sent (trailing, with a ceiling so a
//   steady stream still reads).

import { readCursorToPost } from './looking'

/** The thread page's row cap: talaria-channels MSG_PAGE_THREAD's `limit 300`. */
export const THREAD_PAGE_CAP = 300

/** One read to post: a seq, or null for "the thread's newest complete reply". */
export interface ThreadRead {
  rootId: string
  seq: number | null
}

/** What was last handed off for posting: the root, its seq, and the page's
 *  refetch it came from (a full page has no new seq to compare). */
export interface ThreadReadPosted {
  id: string
  seq: number
  at: number
}

export const NOTHING_POSTED: ThreadReadPosted = { id: '', seq: 0, at: 0 }

/** The read an open thread posts now, or null for none. */
export function threadReadFor(input: {
  rootId: string
  /** Rows on the page, root included. */
  count: number
  /** The page's last row's seq. */
  latest: number
  /** When the page was last fetched (svelte-query's dataUpdatedAt). */
  fetchedAt: number
  posted: ThreadReadPosted
  looking: boolean
}): ThreadRead | null {
  if (!input.looking || !input.latest) return null
  if (input.count >= THREAD_PAGE_CAP) {
    if (input.posted.id === input.rootId && input.fetchedAt <= input.posted.at) return null
    return { rootId: input.rootId, seq: null }
  }
  const seq = readCursorToPost({ channelId: input.rootId, latest: input.latest, posted: input.posted, looking: input.looking })
  return seq === null ? null : { rootId: input.rootId, seq }
}

/** The record to keep after handing `read` off. */
export function postedAfter(read: ThreadRead, fetchedAt: number): ThreadReadPosted {
  return { id: read.rootId, seq: read.seq ?? 0, at: fetchedAt }
}

export const READ_DEBOUNCE_MS = 750
export const READ_MAX_WAIT_MS = 3000

interface Timers {
  set: (fn: () => void, ms: number) => unknown
  clear: (handle: unknown) => void
  now: () => number
}

const realTimers: Timers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
  now: () => Date.now(),
}

/** A trailing debounce over thread reads: `push` holds the newest read for
 *  `waitMs` after the last push, but never past `maxWaitMs` after the first
 *  of a burst. A read for ANOTHER root sends the held one first (switching
 *  threads must not drop the one the person just read), and `flush` sends
 *  whatever is held (the panel closing). */
export function trailingReads(
  send: (read: ThreadRead) => void,
  opts: { waitMs?: number; maxWaitMs?: number; timers?: Timers } = {},
): { push: (read: ThreadRead) => void; flush: () => void } {
  const waitMs = opts.waitMs ?? READ_DEBOUNCE_MS
  const maxWaitMs = opts.maxWaitMs ?? READ_MAX_WAIT_MS
  const t = opts.timers ?? realTimers
  let held: ThreadRead | null = null
  let since = 0
  let timer: unknown = null
  const flush = () => {
    if (timer !== null) t.clear(timer)
    timer = null
    const read = held
    held = null
    if (read) send(read)
  }
  const push = (read: ThreadRead) => {
    if (held && held.rootId !== read.rootId) flush()
    if (!held) since = t.now()
    held = read
    if (timer !== null) t.clear(timer)
    const left = maxWaitMs - (t.now() - since)
    timer = t.set(flush, Math.max(0, Math.min(waitMs, left)))
  }
  return { push, flush }
}
