// Do Not Disturb, as the client presents it: the pause presets, the custom
// "until", the working-hours schedule, and the one sentence that says what is
// in force now. PURE — the zone and the clock come in as arguments — so every
// preset, including the ones that cross a daylight-saving change, is pinned by
// a test rather than found on the day the clocks move.
//
// The server is the one evaluator (`dnd_state` in talaria-notify, I-14): GET
// /api/notifications serves `dnd.active`, `until` and `reason` computed in the
// person's zone, and delivery (W2-E) calls the same function. The client only
// COMPUTES the instant a pause preset means ("until tomorrow 9:00" in Pacific/
// Auckland is a UTC instant the server stores as `pausedUntil`), and re-reads
// expiry locally so a pause that ended while the bell was open stops showing.

import { FOREVER, type DndSchedule, type DndWire } from './notify-wire'

export const FOREVER_MS = Date.parse(FOREVER)

// ── Zone arithmetic ──────────────────────────────────────────────────────────

export interface WallTime {
  y: number
  /** 1–12 */
  m: number
  d: number
  hh: number
  mm: number
  /** 0 = Sunday */
  weekday: number
}

const WEEKDAY_INDEX: Record<string, number> = { Sun: 0, Mon: 1, Tue: 2, Wed: 3, Thu: 4, Fri: 5, Sat: 6 }

const partsFormats = new Map<string, Intl.DateTimeFormat>()
function partsFormat(zone: string): Intl.DateTimeFormat {
  let f = partsFormats.get(zone)
  if (!f) {
    f = new Intl.DateTimeFormat('en-US', {
      timeZone: zone,
      hourCycle: 'h23',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      weekday: 'short',
    })
    partsFormats.set(zone, f)
  }
  return f
}

/** A zone the runtime can resolve, else UTC — the fallback the server's
 *  `talaria_tz` takes for a bad name, so both sides agree on a typo. */
export function safeZone(zone: string | null | undefined): string {
  if (!zone) return 'UTC'
  try {
    partsFormat(zone)
    return zone
  } catch {
    return 'UTC'
  }
}

/** The wall clock in `zone` at `ms`. */
export function wallTime(ms: number, zone: string): WallTime {
  const out: Record<string, string> = {}
  for (const p of partsFormat(safeZone(zone)).formatToParts(new Date(ms))) out[p.type] = p.value
  return {
    y: Number(out.year),
    m: Number(out.month),
    d: Number(out.day),
    hh: Number(out.hour) % 24,
    mm: Number(out.minute),
    weekday: WEEKDAY_INDEX[out.weekday ?? ''] ?? 0,
  }
}

/** The zone's offset from UTC at `ms`, in ms (Auckland in summer: +13 h). */
function offsetAt(ms: number, zone: string): number {
  const whole = Math.floor(ms / 60_000) * 60_000
  const w = wallTime(whole, zone)
  return Date.UTC(w.y, w.m - 1, w.d, w.hh, w.mm) - whole
}

/** The instant a wall time in `zone` names. The offsets a day either side
 *  bracket any daylight-saving change: the candidate whose wall clock reads
 *  back as asked wins (the earlier one for a fall-back hour that happens
 *  twice), and a wall time that does not exist (inside a spring-forward gap)
 *  lands just after the gap, as the clocks do. */
export function instantOf(w: Pick<WallTime, 'y' | 'm' | 'd' | 'hh' | 'mm'>, zone: string): number {
  const z = safeZone(zone)
  const guess = Date.UTC(w.y, w.m - 1, w.d, w.hh, w.mm)
  const a = guess - offsetAt(guess - 86_400_000, z)
  const b = guess - offsetAt(guess + 86_400_000, z)
  const reads = (t: number) => {
    const r = wallTime(t, z)
    return r.y === w.y && r.m === w.m && r.d === w.d && r.hh === w.hh && r.mm === w.mm
  }
  const [early, late] = a <= b ? [a, b] : [b, a]
  if (reads(early)) return early
  if (reads(late)) return late
  return late
}

/** The calendar day after `ms`'s day in `zone`, at hh:mm there. */
export function nextDayAt(ms: number, zone: string, hh: number, mm = 0): number {
  const today = wallTime(ms, zone)
  // Noon UTC of the civil date + 1 day, read back as a civil date: no zone
  // ever moves a noon across a date line twice.
  const tomorrow = new Date(Date.UTC(today.y, today.m - 1, today.d + 1, 12))
  return instantOf(
    { y: tomorrow.getUTCFullYear(), m: tomorrow.getUTCMonth() + 1, d: tomorrow.getUTCDate(), hh, mm },
    zone,
  )
}

/** The zone a pause preset is computed in (O-19): the profile's zone, else
 *  the browser's. `profile` is undefined while the profile has not loaded —
 *  then the zone the server served is used unless it is the UTC it falls back
 *  to for a person with no zone set. */
export function presetZone(input: {
  profile: string | null | undefined
  served: string | null | undefined
  browser: string | null | undefined
}): string {
  if (input.profile) return safeZone(input.profile)
  if (input.profile === null) return safeZone(input.browser ?? input.served)
  if (input.served && input.served !== 'UTC') return safeZone(input.served)
  return safeZone(input.browser ?? input.served)
}

// ── Pause presets ────────────────────────────────────────────────────────────

export type PausePreset = '30m' | '1h' | '2h' | 'tomorrow' | 'forever'

export const PAUSE_PRESETS: ReadonlyArray<{ id: PausePreset; label: string }> = [
  { id: '30m', label: 'For 30 minutes' },
  { id: '1h', label: 'For 1 hour' },
  { id: '2h', label: 'For 2 hours' },
  { id: 'tomorrow', label: 'Until tomorrow, 9:00' },
  { id: 'forever', label: 'Until I resume' },
]

/** The hour "until tomorrow" resumes at, in the person's zone. */
export const TOMORROW_HOUR = 9

/** The `pausedUntil` a preset means at `now`, as the ISO instant the server
 *  stores. */
export function presetUntil(preset: PausePreset, now: number, zone: string): string {
  switch (preset) {
    case '30m':
      return new Date(now + 30 * 60_000).toISOString()
    case '1h':
      return new Date(now + 60 * 60_000).toISOString()
    case '2h':
      return new Date(now + 120 * 60_000).toISOString()
    case 'tomorrow':
      return new Date(nextDayAt(now, zone, TOMORROW_HOUR)).toISOString()
    case 'forever':
      return FOREVER
  }
}

/** A custom "until" from a `datetime-local` value (`YYYY-MM-DDTHH:MM`), read
 *  as a wall time in `zone`. Null when it does not parse or is not in the
 *  future. */
export function customUntil(value: string, now: number, zone: string): string | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value.trim())
  if (!m) return null
  const [y, mo, d, hh, mm] = m.slice(1).map(Number) as [number, number, number, number, number]
  if (mo < 1 || mo > 12 || d < 1 || d > 31 || hh > 23 || mm > 59) return null
  const at = instantOf({ y, m: mo, d, hh, mm }, zone)
  if (!Number.isFinite(at) || at <= now || at >= FOREVER_MS) return null
  return new Date(at).toISOString()
}

/** `datetime-local`'s value for an instant, in `zone` — the custom field's
 *  starting point. */
export function localInputValue(ms: number, zone: string): string {
  const w = wallTime(ms, zone)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${w.y}-${p(w.m)}-${p(w.d)}T${p(w.hh)}:${p(w.mm)}`
}

// ── Saying it ────────────────────────────────────────────────────────────────

/** "3:00 PM" today, "tomorrow 9:00 AM", "Mon 9:00 AM" within the week, else
 *  "Nov 12, 9:00 AM" — all in `zone`. Null for "until I resume". */
export function formatUntil(until: number, now: number, zone: string, locale?: string): string | null {
  if (until >= FOREVER_MS - 1) return null
  const z = safeZone(zone)
  // ICU puts a narrow no-break space before AM/PM; a plain one reads the same
  // and keeps the sentence comparable.
  const time = new Intl.DateTimeFormat(locale, { timeZone: z, hour: 'numeric', minute: '2-digit' })
    .format(until)
    .replace(/[\u202f\u00a0]/g, ' ')
  const a = wallTime(now, z)
  const b = wallTime(until, z)
  const days = Math.round((Date.UTC(b.y, b.m - 1, b.d) - Date.UTC(a.y, a.m - 1, a.d)) / 86_400_000)
  if (days === 0) return time
  if (days === 1) return `tomorrow ${time}`
  if (days > 1 && days < 7) {
    return `${new Intl.DateTimeFormat(locale, { timeZone: z, weekday: 'short' }).format(until)} ${time}`
  }
  return `${new Intl.DateTimeFormat(locale, { timeZone: z, month: 'short', day: 'numeric' }).format(until)}, ${time}`
}

export interface DndView {
  active: boolean
  reason: 'paused' | 'schedule' | null
  /** ms, or null for open-ended. */
  until: number | null
  /** "Until I resume". */
  forever: boolean
  /** The sentence the bell and the panel show; null when nothing is in force. */
  label: string | null
}

const OFF: DndView = { active: false, reason: null, until: null, forever: false, label: null }

/** What is in force at `now`. A pause is read from `pausedUntil` itself, so it
 *  shows the moment it is saved and stops the moment it ends; a schedule
 *  trusts the server's evaluation until the `until` it served passes. */
export function dndView(dnd: DndWire | null | undefined, now: number, zone: string, locale?: string): DndView {
  if (!dnd) return OFF
  const paused = dnd.pausedUntil ? Date.parse(dnd.pausedUntil) : NaN
  if (Number.isFinite(paused) && paused > now) {
    const forever = paused >= FOREVER_MS - 1
    const when = formatUntil(paused, now, zone, locale)
    return { active: true, reason: 'paused', until: forever ? null : paused, forever, label: forever ? 'Paused until you resume' : `Paused until ${when}` }
  }
  if (dnd.active && dnd.reason === 'schedule') {
    const until = dnd.until ? Date.parse(dnd.until) : NaN
    if (Number.isFinite(until) && until <= now) return OFF
    const when = Number.isFinite(until) ? formatUntil(until, now, zone, locale) : null
    return {
      active: true,
      reason: 'schedule',
      until: Number.isFinite(until) ? until : null,
      forever: false,
      label: when ? `Outside working hours until ${when}` : 'Outside working hours',
    }
  }
  return OFF
}

// ── Working hours ────────────────────────────────────────────────────────────

export const WEEKDAYS: ReadonlyArray<{ n: number; short: string }> = [
  { n: 1, short: 'Mon' },
  { n: 2, short: 'Tue' },
  { n: 3, short: 'Wed' },
  { n: 4, short: 'Thu' },
  { n: 5, short: 'Fri' },
  { n: 6, short: 'Sat' },
  { n: 0, short: 'Sun' },
]

export const DEFAULT_SCHEDULE: DndSchedule = { enabled: true, days: [1, 2, 3, 4, 5], start: '09:00', end: '17:00' }

export const isHhmm = (s: string): boolean => /^([01]\d|2[0-3]):[0-5]\d$/.test(s)

/** A schedule the server will accept: unique sorted weekdays and two HH:MM
 *  times, or the reason it will not. */
export function validSchedule(s: DndSchedule): string | null {
  if (!isHhmm(s.start)) return 'Start must be a time, like 09:00'
  if (!isHhmm(s.end)) return 'End must be a time, like 17:00'
  if (s.start === s.end) return 'Start and end must differ'
  if (s.enabled && s.days.length === 0) return 'Pick at least one day'
  return null
}

export function toggleDay(days: readonly number[], n: number): number[] {
  return (days.includes(n) ? days.filter((d) => d !== n) : [...days, n]).sort((a, b) => a - b)
}

/** "Mon–Fri, 09:00–17:00", "Every day, 22:00–06:00 (overnight)". */
export function scheduleSummary(s: DndSchedule): string {
  const days = [...s.days].sort((a, b) => a - b)
  const key = days.join(',')
  const span =
    key === '0,1,2,3,4,5,6'
      ? 'Every day'
      : key === '1,2,3,4,5'
        ? 'Mon–Fri'
        : key === '0,6'
          ? 'Weekends'
          : WEEKDAYS.filter((w) => days.includes(w.n))
              .map((w) => w.short)
              .join(', ') || 'No days'
  return `${span}, ${s.start}–${s.end}${s.start > s.end ? ' (overnight)' : ''}`
}
