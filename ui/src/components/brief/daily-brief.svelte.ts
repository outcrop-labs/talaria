// The brief's client state: one query, one realtime subscription, one cursor.
//
// THE READ CURSOR IS THE INTERESTING PART. Everything a person sees marked
// "new" is derived from `read_seq` on the server row, and the page advances it
// deliberately rather than on render. The reason is the shape of the surface: a
// brief is meant to be left OPEN all day, so an "unseen" flag that cleared the
// moment the data arrived would clear while the person was in another tab, and
// the one question the surface exists to answer — what moved while I was away —
// would be answered "nothing" every time.
//
// So the cursor advances on an explicit gesture (the "N new" control, or
// scrolling the timeline to the top), never on load.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'
import { isBriefAbsent, type BriefResponse, type BriefView } from '@/server/daily-brief-types'

export { isBriefAbsent }
export type { BriefResponse, BriefView }
export type * from '@/server/daily-brief-types'

export const BRIEF_KEY = ['daily-brief'] as const

export function useBrief() {
  return createQuery(() => ({
    queryKey: BRIEF_KEY,
    queryFn: (): Promise<BriefResponse> => getJson<BriefResponse>('/api/brief'),
    // A slow poll UNDER the realtime subscription, not instead of it. The
    // subscription is the fast path and covers everything that publishes; this
    // is the floor for a dropped SSE connection, a sleeping laptop, and the
    // scheduler's own sweep — none of which reach a socket that is not there.
    refetchInterval: 2 * 60_000,
    refetchOnWindowFocus: true,
  }))
}

/** Subscribe to the person's own firehose and refetch when the brief moves.
 *
 *  THE EVENT CARRIES NO CONTENT — see the note on `UserEvent` in
 *  server/realtime.ts — so this deliberately does not try to patch the page
 *  from the payload. It invalidates, the ordinary route re-reads with the
 *  ordinary ACL, and there is exactly one path by which brief content reaches a
 *  browser.
 *
 *  Filtered on `type`, because `user:<id>` is a firehose: every run transition
 *  this person owns arrives here too, and refetching the brief on each of them
 *  would turn a busy afternoon of agent work into a poll. */
export function useBriefLive(): void {
  const qc = useQueryClient()
  $effect(() => {
    const es = new EventSource('/api/me/events')
    es.onmessage = (event: MessageEvent<string>) => {
      try {
        if ((JSON.parse(event.data) as { type?: string }).type !== 'brief') return
      } catch {
        // A frame we cannot parse is a frame we ignore. The slow poll above is
        // what makes that safe rather than silent.
        return
      }
      void qc.invalidateQueries({ queryKey: BRIEF_KEY })
    }
    return () => es.close()
  })
}

export function useBriefActions() {
  const qc = useQueryClient()
  const refresh = (): Promise<unknown> => qc.invalidateQueries({ queryKey: BRIEF_KEY })
  return {
    /** Send or discard a drafted reply.
     *
     *  AWAITED AND ITS ERROR SURFACED, unlike `markRead` below — this is the one
     *  action on the surface that makes something leave the building, and a 409
     *  means the thread moved on and the draft would answer the wrong message.
     *  A person who clicks send has to learn that it did not. */
    async decideReply(draftId: string, decision: 'approve' | 'reject'): Promise<{ ok: true } | { ok: false; error: string }> {
      const res = await fetch('/api/brief/reply', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ draftId, decision }),
      })
      const payload = (await res.json().catch(() => null)) as { error?: string } | null
      await refresh()
      return res.ok ? { ok: true } : { ok: false, error: payload?.error ?? `Failed (${res.status})` }
    },
    /** Hand a conversation to the assistant, or take it back. */
    async setDelegated(channelId: string | null, granted: boolean): Promise<boolean> {
      const res = await fetch('/api/brief/delegate', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ channelId, granted }),
      })
      await refresh()
      return res.ok
    },
    invalidate: (): void => void qc.invalidateQueries({ queryKey: BRIEF_KEY }),
    /** Advance the read cursor. Fire-and-forget: the flag it clears is a
     *  nicety, and a failed POST must not interrupt someone reading. */
    markRead: (brief: BriefView): void => {
      if (brief.lastSeq <= brief.readSeq) return
      void fetch('/api/brief/read', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ briefId: brief.id, seq: brief.lastSeq }),
      })
        .then(() => qc.invalidateQueries({ queryKey: BRIEF_KEY }))
        .catch(() => {})
    },
  }
}

/** Section headings. Held here rather than on the server because they are
 *  presentation — the server's `section` is an enum, and a rename is a UI
 *  change, not a migration on every row ever written. */
export const SECTION_TITLE: Record<string, string> = {
  action: 'Needs you',
  schedule: 'Today',
  comms: 'Waiting on you',
  highlights: 'Worth knowing',
}

export const SECTION_HINT: Record<string, string> = {
  action: 'Work that has stopped and is waiting on a decision from you.',
  schedule: 'Your calendar for today.',
  comms: 'People who asked you something and have not heard back.',
  highlights: 'Nothing is blocked on these — they are just worth knowing.',
}

/** A local time-of-day label. Mono chrome, so it stays short. */
export function clockLabel(iso: string, zone: string): string {
  try {
    return new Intl.DateTimeFormat('en-US', { timeZone: zone, hour: 'numeric', minute: '2-digit' }).format(new Date(iso))
  } catch {
    return new Date(iso).toISOString().slice(11, 16)
  }
}

/** "Monday · 17 Aug" — the brief's own date, read in its own zone. Built from
 *  the stored `YYYY-MM-DD` rather than from `now`, so a brief opened before
 *  midnight for the following day is titled with the day it is FOR. */
export function dateLabel(date: string, zone: string): string {
  try {
    const at = new Date(`${date}T12:00:00Z`)
    const parts = new Intl.DateTimeFormat('en-GB', { timeZone: zone, weekday: 'long', day: 'numeric', month: 'short' }).formatToParts(at)
    const get = (type: string): string => parts.find((p) => p.type === type)?.value ?? ''
    return `${get('weekday')} · ${get('day')} ${get('month')}`
  } catch {
    return date
  }
}
