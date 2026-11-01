/** Which notification rows this browser has already surfaced, so a live
 *  arrival becomes a toast or a banner exactly once — and so nothing that
 *  landed while the page was closed, reloading, or mid-deploy is silently
 *  absorbed.
 *
 *  THE RECORD OUTLIVES THE PAGE. It used to be an in-memory set whose first
 *  read was a silent baseline: anything that landed during a reload, a
 *  closed laptop, or a deploy's 502 was in that first page and was never
 *  surfaced. Now the record lives in localStorage, per user, and only a
 *  browser with NO record for this person baselines silently (a fresh
 *  profile must not replay the inbox as a burst).
 *
 *  THE CONTRACT (I-3 in docs/NOTIFICATIONS.md — the desktop shell's suspend
 *  handoff reads and merges the same key):
 *
 *    localStorage['talaria.notify.arrivals.v1'] =
 *      { "<userId>": { "hw":   "<ISO createdAt of the newest row surfaced or baselined>",
 *                      "seen": { "<rowId>": "<ISO createdAt as last surfaced>", … ≤ 300 },
 *                      "at":   <ms epoch of the last successful read> } }
 *
 *  A row is an ARRIVAL when it is unread, its createdAt is newer than
 *  `hw` minus 120 s (commits land out of order; the margin and the seen set
 *  together catch a row that committed after a newer one), and its id is
 *  unseen OR its createdAt moved later than the one recorded — the second
 *  case is a RE-ALERT (a DM row refreshed in place by a newer message).
 *
 *  V2 (I-20, docs/notifications/contract.md): the same key, and the record
 *  gains `v: 2` and `seq` — the person's notify seq this browser has read
 *  THROUGH. Since the contract every alerting write takes the next seq in its
 *  own transaction (I-10), and `GET /api/notifications?since=S` answers every
 *  row with a seq past S, in seq order. So when the api serves `since`, the
 *  tracker reads exactly the rows past its cursor — on every `notification`
 *  frame, every `resync`, every stream (re)open and every poll — and a row
 *  with a seq is an arrival iff it is unread, its seq is past `rec.seq`, and
 *  it is unseen or re-alerted. No margin, nothing absorbed, nothing twice: a
 *  60-row outage is 60 arrivals, the cursor at the server's seq after. A row
 *  with no seq (filed before the contract) and a wave-1 record keep wave 1's
 *  rule. A wave-1 reader (the desktop shell's suspend handoff) reads only
 *  `hw`/`seen`/`at` and keeps the rest.
 *
 *  THE SERVER'S SEQ CAN GO BACK (docs/notifications/live-and-inbox.md): a
 *  database restored from a backup, a dev stack reset. A `?since` read then
 *  answers no rows and a seq BELOW the cursor, and every row filed since has
 *  a seq at or below the old cursor — a tracker that kept its cursor would
 *  never count one of them as new. So the tracker REBASES: it takes the
 *  server's seq, judges the newest page by wave 1's rule (createdAt against
 *  the high-water and the seen set: what was filed since the restore
 *  surfaces, nothing older replays), and stamps the record `rebased` (ms
 *  epoch). A merge takes the cursor of the record rebased last, so another
 *  tab's pre-restore cursor cannot put the old seq back.
 *
 *  Framework-free so it is testable without a DOM. */

export const ARRIVALS_KEY = 'talaria.notify.arrivals.v1'
/** The out-of-order commit margin. */
export const MARGIN_MS = 120_000
/** Ids kept per user — newest by createdAt. The feed is 50 rows, so 300 is
 *  six feeds of memory. */
export const SEEN_MAX = 300
/** At most this many arrivals are surfaced one by one; past it, one summary. */
export const EACH_MAX = 3

export interface ArrivalRecord {
  hw: string
  seen: Record<string, string>
  at: number
  /** 2 once a v2 tracker wrote it (I-20). */
  v?: 2
  /** The notify seq this browser has read through: a complete `?since` read. */
  seq?: number
  /** When this browser last took a seq LOWER than its cursor from the server
   *  (a restore; see above), ms epoch. In a merge, the record rebased last
   *  holds the cursor. */
  rebased?: number
}

export interface ArrivalRow {
  id: string
  createdAt: string
  readAt: string | null
  /** The row's notify seq (I-10); null on a row filed before the contract. */
  seq?: number | null
}

export interface Arrival<T> {
  row: T
  /** A known row whose createdAt moved later: alert again, same tag. */
  realert: boolean
}

const ms = (iso: string): number => {
  const t = Date.parse(iso)
  return Number.isFinite(t) ? t : 0
}

/** Keep the newest `SEEN_MAX` ids by createdAt. */
function trimSeen(seen: Record<string, string>): Record<string, string> {
  const ids = Object.keys(seen)
  if (ids.length <= SEEN_MAX) return seen
  ids.sort((a, b) => ms(seen[b] ?? '') - ms(seen[a] ?? ''))
  const out: Record<string, string> = {}
  for (const id of ids.slice(0, SEEN_MAX)) out[id] = seen[id] ?? ''
  return out
}

/** Fold a page into a record: every row seen at its createdAt, the
 *  high-water at the newest createdAt either holds; the seq cursor kept (and
 *  moved to `through` when the page was a complete `?since` read). */
function absorb(rec: ArrivalRecord | null, rows: readonly ArrivalRow[], now: number, through?: number | null): ArrivalRecord {
  const seen: Record<string, string> = { ...(rec?.seen ?? {}) }
  let hw = rec?.hw ?? ''
  for (const r of rows) {
    const known = seen[r.id]
    if (!known || ms(r.createdAt) > ms(known)) seen[r.id] = r.createdAt
    if (!hw || ms(r.createdAt) > ms(hw)) hw = r.createdAt
  }
  const out: ArrivalRecord = { hw, seen: trimSeen(seen), at: now }
  const seq = maxSeq(rec?.seq, through)
  if (seq !== undefined) {
    out.v = 2
    out.seq = seq
  } else if (rec?.v === 2) out.v = 2
  if (isStamp(rec?.rebased)) out.rebased = rec.rebased
  return out
}

const isSeq = (n: unknown): n is number => typeof n === 'number' && Number.isFinite(n) && n >= 0
const isStamp = (n: unknown): n is number => typeof n === 'number' && Number.isFinite(n) && n > 0
function maxSeq(a: number | null | undefined, b: number | null | undefined): number | undefined {
  if (isSeq(a) && isSeq(b)) return Math.max(a, b)
  if (isSeq(a)) return a
  if (isSeq(b)) return b
  return undefined
}

/** The pure rule: given what this browser had surfaced (or `null` — never)
 *  and a page of rows (the newest 50 as /api/notifications serves them, or a
 *  `?since` page in seq order), the arrivals, oldest first, and the record
 *  after them. `through`: the page was a complete `?since` read through this
 *  seq — the record's cursor moves there (and, with no record, it is where a
 *  fresh profile's silent baseline starts). */
export function computeArrivals<T extends ArrivalRow>(
  rec: ArrivalRecord | null,
  rows: readonly T[],
  now: number,
  through?: number | null,
): { arrivals: Arrival<T>[]; next: ArrivalRecord } {
  if (!rec) return { arrivals: [], next: absorb(null, rows, now, through) }
  const floor = rec.hw ? ms(rec.hw) - MARGIN_MS : -Infinity
  const cursor = isSeq(rec.seq) ? rec.seq : null
  const arrivals: Arrival<T>[] = []
  for (const r of rows) {
    if (r.readAt) continue // read elsewhere: never surfaced
    const created = ms(r.createdAt)
    if (cursor !== null && isSeq(r.seq)) {
      // v2: the cursor is exact. At or below it, this browser already
      // decided about the row (and a Quiet coalesce that kept its createdAt
      // is not news either).
      if (r.seq <= cursor) continue
    } else if (created <= floor) continue // v1: the margin
    const known = rec.seen[r.id]
    if (known === undefined) arrivals.push({ row: r, realert: false })
    else if (created > ms(known)) arrivals.push({ row: r, realert: true })
  }
  arrivals.sort((a, b) => ms(a.row.createdAt) - ms(b.row.createdAt))
  return { arrivals, next: absorb(rec, rows, now, through) }
}

/** The server's seq went BELOW the cursor (a restore; see the top of this
 *  file). `rows` is the newest page, read now, and `seq` the server's seq:
 *  the page is judged by wave 1's rule — the cursor set aside, createdAt
 *  against the high-water and the seen set — and the record takes `seq`,
 *  stamped `rebased: now`. With no record at all it is the silent baseline. */
export function rebaseArrivals<T extends ArrivalRow>(
  rec: ArrivalRecord | null,
  rows: readonly T[],
  now: number,
  seq: number,
): { arrivals: Arrival<T>[]; next: ArrivalRecord } {
  const base: ArrivalRecord | null = rec ? { hw: rec.hw, seen: rec.seen, at: rec.at } : null
  const { arrivals, next } = computeArrivals(base, rows, now, seq)
  return { arrivals, next: { ...next, v: 2, seq, rebased: now } }
}

// ── The `?since` read (I-19, I-20) ───────────────────────────────────────────

/** One `GET /api/notifications?since=S` page: rows with a seq past S in seq
 *  order (≤ 200), whether more remain, and the person's seq in the same
 *  snapshot. */
export interface SincePage<T> {
  notifications: T[]
  more?: boolean
  seq?: number
}

/** Pages a single catch-up may read before it yields. */
export const SINCE_PAGES = 10

/** Read every row past `from`, page by page. `through` is how far the read
 *  is complete: the snapshot's seq when the last page said `more: false`;
 *  after `SINCE_PAGES` pages, the last row's seq (a page is every row up to
 *  its last one, I-10), and `complete` is false — the caller reads on.
 *  `behind`: the server's seq is BELOW where the read stood (a restore) —
 *  `through` is then the server's seq, and the caller rebases on it. */
export async function readSince<T extends ArrivalRow>(
  fetchPage: (since: number) => Promise<SincePage<T>>,
  from: number,
  maxPages = SINCE_PAGES,
): Promise<{ rows: T[]; through: number | null; complete: boolean; behind?: boolean }> {
  const rows: T[] = []
  let cursor = from
  for (let i = 0; i < maxPages; i++) {
    const page = await fetchPage(cursor)
    const got = Array.isArray(page.notifications) ? page.notifications : []
    rows.push(...got)
    let last = cursor
    for (const r of got) if (isSeq(r.seq) && r.seq > last) last = r.seq
    if (!page.more) {
      // The server's seq is below this read's cursor: it went back. Its seq
      // is the truth (never the larger one this browser had).
      if (isSeq(page.seq) && page.seq < cursor) return { rows, through: page.seq, complete: true, behind: true }
      const snap = isSeq(page.seq) ? page.seq : last
      return { rows, through: Math.max(snap, last), complete: true }
    }
    // `more` with no progress would loop forever: stop where we are.
    if (last <= cursor) return { rows, through: cursor > from ? cursor : null, complete: false }
    cursor = last
  }
  return { rows, through: cursor > from ? cursor : null, complete: false }
}

/** Does this stream frame call for a `?since` read? A `notification` frame
 *  whose seq the cursor already covers, and a complete `resync` AT it, do
 *  not: a replay (I-19) sends one frame per row past the STREAM's cursor,
 *  one at a time, and the first read has usually covered them all. Anything
 *  without a seq, or a tab without a cursor yet, reads — and so does a
 *  `resync` BELOW the cursor: the server's seq went back (a restore), and
 *  the read rebases. */
export function frameNeedsRead(
  ev: { type: string; seq?: number; complete?: boolean },
  cursor: number | null,
): boolean {
  if (ev.type !== 'notification' && ev.type !== 'resync') return false
  if (cursor === null || !isSeq(ev.seq)) return true
  if (ev.seq > cursor) return true
  if (ev.type === 'resync' && ev.seq < cursor) return true
  return ev.type === 'resync' && !ev.complete
}

/** One run at a time; a call while one runs asks for exactly one more after
 *  it. Every `notification` frame, `resync`, open and poll asks for a
 *  `?since` read — a burst of 60 frames is two reads, not sixty. */
export function coalesce(run: () => Promise<void>): () => Promise<void> {
  let running: Promise<void> | null = null
  let again = false
  const go = (): Promise<void> => {
    if (running) {
      again = true
      return running
    }
    running = (async () => {
      try {
        do {
          again = false
          try {
            await run()
          } catch {
            /* the next ask reads again */
          }
        } while (again)
      } finally {
        running = null
      }
    })()
    return running
  }
  return go
}

/** How a batch of arrivals is shown: up to three one by one, oldest first;
 *  more than that as one summary naming the count and the newest title. */
export type Presentation<T> = { kind: 'each'; items: Arrival<T>[] } | { kind: 'summary'; count: number; newest: T }

export function present<T>(arrivals: Arrival<T>[]): Presentation<T> | null {
  if (arrivals.length === 0) return null
  const newest = arrivals[arrivals.length - 1]
  if (arrivals.length <= EACH_MAX || !newest) return { kind: 'each', items: arrivals }
  return { kind: 'summary', count: arrivals.length, newest: newest.row }
}

/** The summary's sentence: "5 new — <newest title>…". */
export function summaryTitle(count: number, newestTitle: string): string {
  return `${count} new — ${newestTitle}…`
}

// ── Storage ──────────────────────────────────────────────────────────────────

/** The store, injectable for tests. Every access is guarded: private mode and
 *  blocked site data throw on access, and then this browser simply behaves as
 *  if it had no record (today's first-read baseline). */
export interface ArrivalStore {
  read(): string | null
  write(value: string): void
}

const localStore: ArrivalStore = {
  read: () => {
    try {
      return window.localStorage.getItem(ARRIVALS_KEY)
    } catch {
      return null
    }
  },
  write: (value) => {
    try {
      window.localStorage.setItem(ARRIVALS_KEY, value)
    } catch {
      /* the record does not persist; the tab still dedupes in memory */
    }
  },
}

function isRecord(v: unknown): v is ArrivalRecord {
  if (!v || typeof v !== 'object') return false
  const r = v as Record<string, unknown>
  if (!(typeof r.hw === 'string' && !!r.seen && typeof r.seen === 'object' && typeof r.at === 'number')) return false
  // A junk cursor is no cursor: the record falls back to wave 1's rule.
  if (r.seq !== undefined && !isSeq(r.seq)) delete r.seq
  if (r.rebased !== undefined && !isStamp(r.rebased)) delete r.rebased
  return true
}

function readAll(store: ArrivalStore): Record<string, ArrivalRecord> {
  const raw = store.read()
  if (!raw) return {}
  try {
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== 'object') return {}
    const out: Record<string, ArrivalRecord> = {}
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) if (isRecord(v)) out[k] = v
    return out
  } catch {
    return {}
  }
}

/** Merge two records of the same person: the union of what each surfaced. */
function merge(a: ArrivalRecord | undefined, b: ArrivalRecord): ArrivalRecord {
  if (!a) return b
  const seen = { ...a.seen }
  for (const [id, at] of Object.entries(b.seen)) {
    const known = seen[id]
    if (!known || ms(at) > ms(known)) seen[id] = at
  }
  const out: ArrivalRecord = {
    hw: !a.hw || ms(b.hw) > ms(a.hw) ? b.hw : a.hw,
    seen: trimSeen(seen),
    at: Math.max(a.at, b.at),
  }
  // The union's cursor is the furthest either read through: each tab's view
  // only ever advances on a complete read. Unless one of them REBASED later
  // than the other (the server's seq went back): its cursor is the server's
  // truth, and the other's larger one is from before the restore.
  const ra = isStamp(a.rebased) ? a.rebased : 0
  const rb = isStamp(b.rebased) ? b.rebased : 0
  const seq = ra === rb ? maxSeq(a.seq, b.seq) : ra > rb ? maxSeq(a.seq, undefined) : maxSeq(b.seq, undefined)
  if (seq !== undefined) out.seq = seq
  if (a.v === 2 || b.v === 2 || seq !== undefined) out.v = 2
  if (ra || rb) out.rebased = Math.max(ra, rb)
  return out
}

/** One tab's tracker for one person. The persisted record is read at the
 *  tab's FIRST read (so a reopen catches up instead of baselining), and every
 *  read merges this tab's view back into it. After that first read the tab
 *  judges against its OWN view: another tab having surfaced a row must not
 *  stop this one deciding about it (only the leader banners, and it may be
 *  this one). */
export class PersistedArrivals<T extends ArrivalRow> {
  private view: ArrivalRecord | null = null
  private primed = false

  constructor(
    private readonly userId: string,
    private readonly store: ArrivalStore = localStore,
  ) {}

  /** Judge a page of rows. `through`: the page was a complete `?since`
   *  read up to this seq (the cursor moves there). */
  arrive(rows: readonly T[], now = Date.now(), through?: number | null): Arrival<T>[] {
    this.prime()
    const { arrivals, next } = computeArrivals(this.view, rows, now, through)
    this.save(next)
    return arrivals
  }

  /** The server's seq went below the cursor (a restore): judge the newest
   *  page by wave 1's rule and take the server's `seq` (rebaseArrivals). */
  rebase(rows: readonly T[], seq: number, now = Date.now()): Arrival<T>[] {
    this.prime()
    const { arrivals, next } = rebaseArrivals(this.view, rows, now, seq)
    this.save(next)
    return arrivals
  }

  private save(next: ArrivalRecord): void {
    this.view = next
    const all = readAll(this.store)
    all[this.userId] = merge(all[this.userId], next)
    this.store.write(JSON.stringify(all))
  }

  /** Where this tab's `?since` read starts: its cursor, or null when it has
   *  none yet (a wave-1 record, or no record at all). */
  cursor(): number | null {
    this.prime()
    return isSeq(this.view?.seq) ? (this.view?.seq as number) : null
  }

  /** Has this browser any record for the person (else the first read is a
   *  silent baseline)? */
  hasRecord(): boolean {
    this.prime()
    return this.view !== null
  }

  /** The furthest cursor any tab of this browser has persisted — what a
   *  stream opened now replays from (`/api/me/events?since=`). */
  persistedCursor(): number | null {
    const rec = readAll(this.store)[this.userId]
    return rec && isSeq(rec.seq) ? rec.seq : this.cursor()
  }

  private prime(): void {
    if (this.primed) return
    this.primed = true
    this.view = readAll(this.store)[this.userId] ?? null
  }
}
