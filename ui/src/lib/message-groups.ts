// Message grouping for chat transcripts: a run of consecutive messages from
// one sender reads as one block. The block's first row carries the avatar, the
// name and the time; every later row is a continuation line that keeps its own
// time in the gutter (shown on hover or focus, and always exposed to assistive
// tech). Pure on purpose, like the day dividers next door: every transcript
// (channels and DMs, the thread panel, agent chat, work and research runs)
// calls this one function, so the rule cannot drift between surfaces. What
// each surface feeds it is pure too, and tested where it lives:
// channelGroupFacts / threadGroupFacts (components/chat/channel-groups.ts)
// and turnGroupFacts (components/chat/chat-view.ts).
//
// THE RULES. A row starts a new block when:
//   - the sender changes (a user and an agent, or two users, never share one);
//   - more than GROUP_GAP_MS passed since the row before it, or it is stamped
//     BEFORE the row before it. A negative gap is not a short one: a row the
//     client drew on a slow clock reads as earlier than the server-stamped row
//     above it, and grouping it would move its header when the sync swaps in
//     the server's time;
//   - it opens a calendar day (the list's own day divider sits above it);
//   - either row has no time that parses (a gap that cannot be measured is
//     not claimed to be short). DisplayMessage.createdAt is optional, so the
//     type allows this even though every row drawn today carries one;
//   - the caller marks it `fresh`: a row whose context the header carries — a
//     scheduled run's turn, or the first reply under a thread's root.
//
// Nothing else decides it. A message's status, content, tools, reactions or
// replies do not: a streaming row is grouped exactly as it will be once it
// settles, so tokens arriving never move a header, and an agent's tool-only
// turn stays in its run (its tool disclosure says what it is). A deleted
// message simply leaves the list, and the rows around it regroup.

import type { Instant } from '@/lib/day-dividers'

/** A pause long enough that the reader wants the clock again. Five minutes:
 *  inside it, consecutive messages are one exchange (someone typing a few
 *  lines, an agent posting its follow-up); past it, the next message is a new
 *  thought, and its time is worth showing without a hover. Measured between
 *  consecutive messages, not from the block's start, and a gap of exactly
 *  this long still groups ("exceeds" is strict). */
export const GROUP_GAP_MS = 5 * 60 * 1000

export interface GroupFacts {
  /** Who sent it, as a stable key (e.g. `user:<id>`, `agent:<model>`). */
  sender: string
  /** When it was sent. Missing or unparseable → it starts a block. */
  at: Instant | null | undefined
  /** Start a new block regardless of the rows before it. */
  fresh?: boolean
}

export interface MessageBlock {
  /** Index of the block's first row in the input list. */
  start: number
  /** Index of its last row (inclusive). */
  end: number
  sender: string
}

export interface GroupOpts {
  /** The indexes that open a calendar day: the list's own day-divider map, so
   *  the blocks and the dividers can never disagree about where a day starts. */
  dayStarts: { has(index: number): boolean }
  /** Override the threshold (tests). */
  gapMs?: number
}

const toMs = (v: Instant | null | undefined): number =>
  v == null ? Number.NaN : v instanceof Date ? v.getTime() : typeof v === 'number' ? v : Date.parse(v)

/** Partition a chronologically ordered list into blocks. */
export function messageBlocks<T>(
  items: readonly T[],
  facts: (item: T, index: number) => GroupFacts,
  opts: GroupOpts,
): MessageBlock[] {
  const gap = opts.gapMs ?? GROUP_GAP_MS
  const out: MessageBlock[] = []
  let prev: { sender: string; ms: number } | null = null
  items.forEach((item, index) => {
    const f = facts(item, index)
    const ms = toMs(f.at)
    // NaN on either side fails both comparisons, so an unmeasurable gap breaks.
    const d = prev === null ? Number.NaN : ms - prev.ms
    const joins =
      prev !== null && !f.fresh && prev.sender === f.sender && !opts.dayStarts.has(index) && d >= 0 && d <= gap
    const last = out[out.length - 1]
    if (joins && last) last.end = index
    else out.push({ start: index, end: index, sender: f.sender })
    prev = { sender: f.sender, ms }
  })
  return out
}

/** The indexes that render as continuation lines (every row of a block but
 *  its first) — what a `{#each}` looks up. */
export function continuationSet(blocks: readonly MessageBlock[]): Set<number> {
  const out = new Set<number>()
  for (const b of blocks) for (let i = b.start + 1; i <= b.end; i++) out.add(i)
  return out
}
