// Date dividers for chat transcripts (R1 / KTD3): one centered pill before the
// first message of each calendar day, in the VIEWER's zone. Pure on purpose —
// both transcripts (channels + thread pane, and agent DMs) call this one
// helper, and the clock is an argument so "Today" never depends on when a test
// runs or how long a tab has been open.
//
// Labels are English-only ("Friday, September 4th") to match the rest of the
// UI's copy; the zone, not the locale, is what decides the day.

export type Instant = string | number | Date

export interface DayDividerOpts {
  /** IANA zone (the user's profile preference). Undefined, or a name Intl
   *  cannot resolve, falls back to the browser's zone. */
  timeZone?: string | null
  /** The clock "Today"/"Yesterday" are measured against. */
  now: Date | number
}

export interface DayBoundary {
  /** Index of the day's first item in the input list. */
  index: number
  /** `YYYY-MM-DD` in the resolved zone — a stable `{#each}` key. */
  key: string
  label: string
}

const toMs = (v: Instant): number => (v instanceof Date ? v.getTime() : typeof v === 'number' ? v : Date.parse(v))

const zoneOrUndefined = (tz: string | null | undefined): string | undefined => {
  if (!tz) return undefined
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: tz })
    return tz
  } catch {
    return undefined
  }
}

interface Ymd {
  y: number
  m: number
  d: number
  weekday: string
  month: string
}

// One formatter per zone — dayDividers runs on every transcript render.
const formatters = new Map<string, Intl.DateTimeFormat>()
const formatter = (tz: string | undefined): Intl.DateTimeFormat => {
  const k = tz ?? ''
  let f = formatters.get(k)
  if (!f) {
    f = new Intl.DateTimeFormat('en-US', {
      timeZone: tz,
      year: 'numeric',
      month: 'numeric',
      day: 'numeric',
      weekday: 'long',
    })
    formatters.set(k, f)
  }
  return f
}

const MONTHS = [
  'January', 'February', 'March', 'April', 'May', 'June',
  'July', 'August', 'September', 'October', 'November', 'December',
]

function partsOf(ms: number, tz: string | undefined): Ymd {
  const parts = formatter(tz).formatToParts(new Date(ms))
  const get = (t: string) => parts.find((p) => p.type === t)?.value ?? ''
  const m = Number(get('month'))
  return { y: Number(get('year')), m, d: Number(get('day')), weekday: get('weekday'), month: MONTHS[m - 1] ?? '' }
}

const pad = (n: number) => String(n).padStart(2, '0')
const keyOf = (p: { y: number; m: number; d: number }) => `${p.y}-${pad(p.m)}-${pad(p.d)}`

/** 1st 2nd 3rd 4th … 11th 12th 13th … 21st 22nd 23rd. */
export function ordinal(n: number): string {
  const teen = n % 100
  if (teen >= 11 && teen <= 13) return `${n}th`
  return `${n}${({ 1: 'st', 2: 'nd', 3: 'rd' } as Record<number, string>)[n % 10] ?? 'th'}`
}

function labelFor(p: Ymd, todayKey: string, yesterdayKey: string, thisYear: number): string {
  const key = keyOf(p)
  if (key === todayKey) return 'Today'
  if (key === yesterdayKey) return 'Yesterday'
  const base = `${p.weekday}, ${p.month} ${ordinal(p.d)}`
  return p.y === thisYear ? base : `${base}, ${p.y}`
}

function clock(opts: DayDividerOpts) {
  const tz = zoneOrUndefined(opts.timeZone)
  const today = partsOf(typeof opts.now === 'number' ? opts.now : opts.now.getTime(), tz)
  // Calendar arithmetic, not "now minus 24h" — a DST day is 23 or 25 hours.
  const y = new Date(Date.UTC(today.y, today.m - 1, today.d - 1))
  const yesterdayKey = keyOf({ y: y.getUTCFullYear(), m: y.getUTCMonth() + 1, d: y.getUTCDate() })
  return { tz, todayKey: keyOf(today), yesterdayKey, thisYear: today.y }
}

/** The divider label for one instant: "Today", "Yesterday", or a full date. */
export function dayLabel(instant: Instant, opts: DayDividerOpts): string {
  const ms = toMs(instant)
  if (Number.isNaN(ms)) return ''
  const c = clock(opts)
  return labelFor(partsOf(ms, c.tz), c.todayKey, c.yesterdayKey, c.thisYear)
}

/** Day boundaries for a chronologically ordered list: one entry per calendar
 *  day, at the index of that day's first item. Items whose timestamp does not
 *  parse never open a day — they ride with the one before them. */
export function dayDividers<T>(items: readonly T[], at: (item: T) => Instant, opts: DayDividerOpts): DayBoundary[] {
  if (items.length === 0) return []
  const c = clock(opts)
  const out: DayBoundary[] = []
  let prev = ''
  items.forEach((item, index) => {
    const ms = toMs(at(item))
    if (Number.isNaN(ms)) return
    const p = partsOf(ms, c.tz)
    const key = keyOf(p)
    if (key === prev) return
    prev = key
    out.push({ index, key, label: labelFor(p, c.todayKey, c.yesterdayKey, c.thisYear) })
  })
  return out
}

/** `dayDividers` as an index → boundary map — what a `{#each}` looks up. */
export function dayDividerMap<T>(
  items: readonly T[],
  at: (item: T) => Instant,
  opts: DayDividerOpts,
): Map<number, DayBoundary> {
  return new Map(dayDividers(items, at, opts).map((b) => [b.index, b]))
}
