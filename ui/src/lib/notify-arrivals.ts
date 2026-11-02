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
}

export interface ArrivalRow {
  id: string
  createdAt: string
  readAt: string | null
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
 *  high-water at the newest createdAt either holds. */
function absorb(rec: ArrivalRecord | null, rows: readonly ArrivalRow[], now: number): ArrivalRecord {
  const seen: Record<string, string> = { ...(rec?.seen ?? {}) }
  let hw = rec?.hw ?? ''
  for (const r of rows) {
    const known = seen[r.id]
    if (!known || ms(r.createdAt) > ms(known)) seen[r.id] = r.createdAt
    if (!hw || ms(r.createdAt) > ms(hw)) hw = r.createdAt
  }
  return { hw, seen: trimSeen(seen), at: now }
}

/** The pure rule: given what this browser had surfaced (or `null` — never)
 *  and a page of rows (newest first, as /api/notifications serves it),
 *  the arrivals, oldest first, and the record after them. */
export function computeArrivals<T extends ArrivalRow>(
  rec: ArrivalRecord | null,
  rows: readonly T[],
  now: number,
): { arrivals: Arrival<T>[]; next: ArrivalRecord } {
  if (!rec) return { arrivals: [], next: absorb(null, rows, now) }
  const floor = rec.hw ? ms(rec.hw) - MARGIN_MS : -Infinity
  const arrivals: Arrival<T>[] = []
  for (const r of rows) {
    if (r.readAt) continue // read elsewhere: never surfaced
    const created = ms(r.createdAt)
    if (created <= floor) continue
    const known = rec.seen[r.id]
    if (known === undefined) arrivals.push({ row: r, realert: false })
    else if (created > ms(known)) arrivals.push({ row: r, realert: true })
  }
  arrivals.sort((a, b) => ms(a.row.createdAt) - ms(b.row.createdAt))
  return { arrivals, next: absorb(rec, rows, now) }
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
  return typeof r.hw === 'string' && !!r.seen && typeof r.seen === 'object' && typeof r.at === 'number'
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
  return {
    hw: !a.hw || ms(b.hw) > ms(a.hw) ? b.hw : a.hw,
    seen: trimSeen(seen),
    at: Math.max(a.at, b.at),
  }
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

  arrive(rows: readonly T[], now = Date.now()): Arrival<T>[] {
    if (!this.primed) {
      this.primed = true
      this.view = readAll(this.store)[this.userId] ?? null
    }
    const { arrivals, next } = computeArrivals(this.view, rows, now)
    this.view = next
    const all = readAll(this.store)
    all[this.userId] = merge(all[this.userId], next)
    this.store.write(JSON.stringify(all))
    return arrivals
  }
}
