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
import { createQuery, useQueryClient, type QueryClient } from '@tanstack/svelte-query'
import { SvelteMap } from 'svelte/reactivity'
import { errorMessage, getJson, postJson } from '@/lib/fetch-json'
import { onUserEvent } from '@/lib/user-events.svelte'
import { briefEventIsNews, withLineFrom, withMark } from '@/lib/daily-brief-types'
import type { BriefEventSignal, BriefMarkAction, BriefMarkAnswer, BriefResponse, BriefView } from '@/lib/daily-brief-types'

export type { BriefResponse, BriefView }
export type * from '@/lib/daily-brief-types'

export { isBriefAbsent } from '@/lib/brief-absent'

/** The browser's IANA zone, sent on the brief read and on item actions. The
 *  person's STORED zone (Settings, or the first-run adoption) outranks it on
 *  the server; this param is the fallback that predates the stored zone —
 *  still what makes "due" and "today" mean the READER'S day rather than the
 *  server's for anyone who has never set one. */
const browserZone = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone
  } catch {
    return ''
  }
}

export const BRIEF_KEY = ['daily-brief'] as const

// ── Marks in flight ─────────────────────────────────────────────────────────
//
// A check-off is shown the moment it is clicked and confirmed by the POST's
// answer, which carries the whole page (POST /api/brief/item). Until then the
// mark lives here, so EVERY page that lands in the cache meanwhile — another
// mark's answer, the slow poll, a refetch — gets it laid back on top instead
// of flickering the line back open. Module state, like the query cache it
// guards: the card that was clicked moves lanes the moment it is marked, and
// whatever it remounts as must still see the mark and, if the POST fails,
// the failure.

interface PendingMark {
  token: number
  action: BriefMarkAction
  at: string
}

/** The latest mark on each line whose POST has not answered, by source key. */
const pending = new Map<string, PendingMark>()
/** Each line's POSTs, one after another — the server applies a check and a
 *  quick put-back in the order they were clicked. */
const chains = new Map<string, Promise<unknown>>()
/** The newest 'brief' event heard while a mark was in flight: an append's is
 *  very likely the mark's own echo, which the POST's answer will already
 *  cover. A stale nudge among them is remembered as such, because the
 *  answer never covers one (`briefEventIsNews`). */
let held: Required<BriefEventSignal> | null = null
let tokens = 0

/** `held`, with one more event heard. */
function holding(prev: Required<BriefEventSignal> | null, event: BriefEventSignal): Required<BriefEventSignal> {
  const stale = event.stale === true
  if (!prev || prev.briefId !== event.briefId) return { briefId: event.briefId, seq: event.seq, stale }
  return { briefId: prev.briefId, seq: Math.max(prev.seq, event.seq), stale: prev.stale || stale }
}

/** Lines whose last mark did not go through, by source key, with the verdict
 *  that failed. Reactive, so a row that remounted on its way back to its lane
 *  can still say so. */
const failedMarks = new SvelteMap<string, BriefMarkAction>()

/** Whether the last mark on this line did not go through — the "That did not
 *  go through" a row shows, including a row that remounted since it was
 *  clicked (a marked card changes lanes at once, and a failed mark brings it
 *  back as a new row). Cleared by the next mark on the line, and by a page
 *  from the server that shows the line where the failed verdict would have
 *  put it (`settleFailures`). Reactive. */
export function briefMarkFailed(sourceKey: string): boolean {
  return failedMarks.has(sourceKey)
}

/** Drop the failures a page from the server makes moot: the line is where
 *  the failed verdict would have put it — closed for a check-off or a
 *  dismissal, open for a put-back — or it is gone from the page.
 *
 *  A POST can fail at the proxy after its commit landed (a slow disk); the
 *  refetch that follows then shows the mark done, and a card sitting in Done
 *  today saying the click failed would be wrong for the life of the tab. A
 *  mark that really failed leaves the line as it was, and the failure stays. */
function settleFailures(page: BriefResponse): void {
  if (failedMarks.size === 0 || 'absent' in page) return
  const lines = new Map(page.sections.flatMap((s) => s.lines).map((l) => [l.key, l] as const))
  const moot = [...failedMarks].filter(([key, action]) => {
    const line = lines.get(key)
    return !line || line.resolved === (action !== 'restore')
  })
  for (const [key] of moot) failedMarks.delete(key)
}

function withPendingMarks(page: BriefResponse): BriefResponse {
  let out = page
  for (const [key, mark] of pending) out = withMark(out, key, mark.action, mark.at)
  return out
}

/** The newer of two documents of one brief, by how far its log reaches. A
 *  page read before a mark committed must not land over the page the mark's
 *  answer carried: the log only grows, so a lower lastSeq is an older read.
 *  A different brief (the day turned, the zone changed) always wins. */
function newerPage(fresh: BriefResponse, cached: BriefResponse | undefined): BriefResponse {
  if (!cached || 'absent' in cached || 'absent' in fresh) return fresh
  return fresh.id === cached.id && fresh.lastSeq < cached.lastSeq ? cached : fresh
}

/** A 'brief' event the cache has not seen refetches it — once.
 *
 *  An append's echo joins a read already in flight rather than restarting
 *  it: that read either has the append or the next event will say so. A
 *  STALE NUDGE needs a read that starts after it — one that left before the
 *  throttle cleared did not sweep, and the sweep is the point of the nudge
 *  (a new message's line, a replied thread's or a decided run's line
 *  closing) — so it restarts the read in flight. Only this listener
 *  refetches (the app-wide map marks stale with no refetch), so a nudge is
 *  one GET, not two that cancel each other. */
function refetchIfNews(qc: QueryClient, event: BriefEventSignal): void {
  if (!briefEventIsNews(qc.getQueryData<BriefResponse>(BRIEF_KEY), event)) return
  void qc.invalidateQueries({ queryKey: BRIEF_KEY }, { cancelRefetch: event.stale === true })
}

export function useBrief() {
  const qc = useQueryClient()
  return createQuery(() => ({
    queryKey: BRIEF_KEY,
    queryFn: async (): Promise<BriefResponse> => {
      const fresh = await getJson<BriefResponse>(`/api/brief?tz=${encodeURIComponent(browserZone())}`)
      const next = withPendingMarks(newerPage(fresh, qc.getQueryData<BriefResponse>(BRIEF_KEY)))
      settleFailures(next)
      return next
    },
    // A slow poll UNDER the realtime subscription, not instead of it. The
    // subscription is the fast path and covers everything that publishes; this
    // is the floor for a dropped SSE connection, a sleeping laptop, and the
    // scheduler's own sweep — none of which reach a socket that is not there.
    //
    // FAST WHILE 'writing': that state means the server is opening the brief
    // for this reader right now (getBrief kicks the open on demand), and the
    // document's first append may beat the SSE connection up — a fresh page's
    // EventSource can still be handshaking when the brief event fires. Four
    // seconds closes that gap without a spinner anyone has to watch; the
    // ordinary 2-minute cadence resumes the moment a document exists.
    refetchInterval: (query): number => {
      const data = query.state.data
      return data && 'absent' in data && data.absent === 'writing' ? 4_000 : 120_000
    },
    refetchOnWindowFocus: true,
  }))
}

/** Refetch the brief when the person's firehose says it moved.
 *
 *  THE EVENT CARRIES NO CONTENT — see the note on `UserEvent` in the Rust
 *  realtime engine (api/src/realtime.rs) — so this deliberately does not try
 *  to patch the page
 *  from the payload. It invalidates, the ordinary route re-reads with the
 *  ordinary ACL, and there is exactly one path by which brief content reaches a
 *  browser.
 *
 *  Filtered on `type`, because `user:<id>` is a firehose: every run transition
 *  this person owns arrives here too, and refetching the brief on each of them
 *  would turn a busy afternoon of agent work into a poll. The subscription
 *  itself is the tab's ONE shared firehose (user-events.svelte), not a
 *  private stream — a dedicated Redis subscriber per surface was the cost of
 *  the old shape.
 *
 *  THE ONE LISTENER THAT REFETCHES. The app-wide map only marks the page
 *  stale for its next mount. An append's event the cached page already
 *  covers (its lastSeq is at or past the event's) is dropped, and one that
 *  arrives while a check-off is in flight waits for the POST's answer, which
 *  carries the page — that event is almost always the click's own echo. A
 *  stale nudge is never dropped: held during a mark, it still refetches once
 *  the mark answers, whatever seq the answer reached. */
export function useBriefLive(): void {
  const qc = useQueryClient()
  $effect(() =>
    onUserEvent((event) => {
      if (event.type !== 'brief') return
      if (pending.size > 0) {
        held = holding(held, event)
        return
      }
      refetchIfNews(qc, event)
    }),
  )
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
      try {
        await postJson('/api/brief/reply', { draftId, decision })
        return { ok: true }
      } catch (e) {
        return { ok: false, error: errorMessage(e) }
      } finally {
        await refresh()
      }
    },
    /** Approve or reject a pending outbound action — an email or an event the
     *  assistant drafted — decided straight off the line.
     *
     *  AWAITED AND ITS ERROR SURFACED, like decideReply and unlike the marks
     *  below: this one makes something leave the building, and a refusal (the
     *  action already decided, the Google connection lapsed) has to reach the
     *  person who clicked. The pending row's decided status is what the sweep
     *  reads to close the line as APPROVED or REJECTED. */
    async decideApproval(id: string, decision: 'approve' | 'reject'): Promise<{ ok: true } | { ok: false; error: string }> {
      try {
        await postJson(`/api/integrations/google/pending/${encodeURIComponent(id)}`, { decision })
        return { ok: true }
      } catch (e) {
        return { ok: false, error: errorMessage(e) }
      } finally {
        await refresh()
      }
    },
    /** The owner's own verdict on a line: done, not needed, or put it back.
     *
     *  OPTIMISTIC. The line is struck through (or put back) in the cache at
     *  once, with the row the server is about to write — the card moves lanes
     *  on the click, not after a round trip. The POST's answer carries the
     *  page as GET would serve it, and replaces the cache: one request per
     *  click, no refetch. A failed POST puts the line back as it was,
     *  refetches, records the failure (`briefMarkFailed`), and resolves false. */
    async markItem(sourceKey: string, action: BriefMarkAction): Promise<boolean> {
      const mark: PendingMark = { token: ++tokens, action, at: new Date().toISOString() }
      failedMarks.delete(sourceKey)
      // A read already in flight is NOT cancelled. Its answer cannot land over
      // the mark — the query lays every pending mark on whatever page lands,
      // and keeps the newer of two pages — and it may be the one a stale
      // nudge started, whose sweep the mark's own answer (read, no sweep)
      // would never make up for.
      const before = qc.getQueryData<BriefResponse>(BRIEF_KEY)
      pending.set(sourceKey, mark)
      if (before) qc.setQueryData<BriefResponse>(BRIEF_KEY, withMark(before, sourceKey, action, mark.at))

      const prior = chains.get(sourceKey) ?? Promise.resolve()
      const post = prior.then(() => postJson<BriefMarkAnswer>('/api/brief/item', { sourceKey, action, tz: browserZone() }))
      const done = post.catch(() => null)
      chains.set(sourceKey, done)
      let answer: BriefMarkAnswer | null = null
      let ok = true
      try {
        answer = await post
      } catch {
        ok = false
      }
      if (chains.get(sourceKey) === done) chains.delete(sourceKey)
      // The line's newest mark — not one a later click on it has overtaken.
      const latest = pending.get(sourceKey)?.token === mark.token
      if (latest) pending.delete(sourceKey)

      if (ok && answer?.brief) {
        // Through the same recency test as a GET: two marks on different
        // lines answer in either order, and the answer read before the other
        // mark committed must not land over the page that has both.
        const next = withPendingMarks(newerPage(answer.brief, qc.getQueryData<BriefResponse>(BRIEF_KEY)))
        settleFailures(next)
        qc.setQueryData<BriefResponse>(BRIEF_KEY, next)
      } else if (ok) {
        void refresh()
      } else {
        // A failure the line's later click has overtaken is not the line's
        // state; that click's own answer is.
        if (latest) failedMarks.set(sourceKey, action)
        const now = qc.getQueryData<BriefResponse>(BRIEF_KEY)
        const restored = now ? withLineFrom(now, before, sourceKey) : before
        if (restored) qc.setQueryData<BriefResponse>(BRIEF_KEY, withPendingMarks(restored))
        void refresh()
      }
      if (pending.size === 0 && held) {
        const event = held
        held = null
        refetchIfNews(qc, event)
      }
      return ok
    },
    /** Hand a conversation to the assistant, or take it back. */
    async setDelegated(channelId: string | null, granted: boolean): Promise<boolean> {
      let ok = true
      try {
        await postJson('/api/brief/delegate', { channelId, granted })
      } catch {
        ok = false
      }
      await refresh()
      return ok
    },
    invalidate: (): void => void qc.invalidateQueries({ queryKey: BRIEF_KEY }),
    /** Advance the read cursor. Fire-and-forget: the flag it clears is a
     *  nicety, and a failed POST must not interrupt someone reading. */
    markRead: (brief: BriefView): void => {
      if (brief.lastSeq <= brief.readSeq) return
      void postJson('/api/brief/read', { briefId: brief.id, seq: brief.lastSeq })
        .then(() => qc.invalidateQueries({ queryKey: BRIEF_KEY }))
        .catch(() => {})
    },
  }
}

export function clockLabel(iso: string, zone: string): string {
  try {
    return new Intl.DateTimeFormat('en-US', { timeZone: zone, hour: 'numeric', minute: '2-digit' }).format(new Date(iso))
  } catch {
    return new Date(iso).toISOString().slice(11, 16)
  }
}

export function dateLabel(date: string, zone: string): string {
  try {
    const at = new Date(`${date}T12:00:00Z`)
    const parts = new Intl.DateTimeFormat('en-GB', { timeZone: zone, weekday: 'long', day: 'numeric', month: 'long' }).formatToParts(at)
    const get = (type: string): string => parts.find((p) => p.type === type)?.value ?? ''
    return `${get('weekday')}, ${get('day')} ${get('month')}`
  } catch {
    return date
  }
}
