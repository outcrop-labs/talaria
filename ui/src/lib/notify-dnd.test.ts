// The pause presets and the DND sentence, pinned with fixed instants — the
// daylight-saving days included, in both hemispheres.
import { describe, expect, it } from 'vitest'
import {
  customUntil,
  dndView,
  FOREVER_MS,
  formatUntil,
  instantOf,
  localInputValue,
  nextDayAt,
  presetUntil,
  presetZone,
  safeZone,
  scheduleSummary,
  toggleDay,
  validSchedule,
  wallTime,
} from './notify-dnd'
import { FOREVER, type DndWire } from './notify-wire'

const at = (iso: string) => Date.parse(iso)
const BASE: DndWire = {
  pausedUntil: null,
  schedule: null,
  allowNeedsYou: true,
  active: false,
  until: null,
  reason: null,
  timeZone: 'UTC',
}

describe('zone arithmetic', () => {
  it('reads a wall time in a zone', () => {
    // 2026-11-01T03:00Z is 16:00 on Sunday 1 Nov in Auckland (NZDT, +13).
    expect(wallTime(at('2026-11-01T03:00:00Z'), 'Pacific/Auckland')).toEqual({ y: 2026, m: 11, d: 1, hh: 16, mm: 0, weekday: 0 })
  })

  it('finds the instant of a wall time on either side of a DST change', () => {
    // New York springs forward at 02:00 on 8 Mar 2026: 09:00 that day is EDT.
    expect(new Date(instantOf({ y: 2026, m: 3, d: 8, hh: 9, mm: 0 }, 'America/New_York')).toISOString()).toBe('2026-03-08T13:00:00.000Z')
    // The day before is still EST.
    expect(new Date(instantOf({ y: 2026, m: 3, d: 7, hh: 9, mm: 0 }, 'America/New_York')).toISOString()).toBe('2026-03-07T14:00:00.000Z')
    // London falls back on 25 Oct 2026.
    expect(new Date(instantOf({ y: 2026, m: 10, d: 25, hh: 9, mm: 0 }, 'Europe/London')).toISOString()).toBe('2026-10-25T09:00:00.000Z')
    expect(new Date(instantOf({ y: 2026, m: 10, d: 24, hh: 9, mm: 0 }, 'Europe/London')).toISOString()).toBe('2026-10-24T08:00:00.000Z')
  })

  it('lands a wall time inside a spring-forward gap just after it', () => {
    // 02:30 does not exist in New York on 8 Mar 2026; the clock reads 03:30 EDT.
    const t = instantOf({ y: 2026, m: 3, d: 8, hh: 2, mm: 30 }, 'America/New_York')
    expect(wallTime(t, 'America/New_York')).toMatchObject({ d: 8, hh: 3, mm: 30 })
  })

  it('falls back to UTC for a zone the runtime cannot resolve', () => {
    expect(safeZone('Mars/Olympus_Mons')).toBe('UTC')
    expect(safeZone(null)).toBe('UTC')
    expect(safeZone('Pacific/Auckland')).toBe('Pacific/Auckland')
  })
})

describe('pause presets', () => {
  const now = at('2026-11-01T03:00:00.000Z')

  it('adds the short ones to now', () => {
    expect(presetUntil('30m', now, 'UTC')).toBe('2026-11-01T03:30:00.000Z')
    expect(presetUntil('1h', now, 'UTC')).toBe('2026-11-01T04:00:00.000Z')
    expect(presetUntil('2h', now, 'UTC')).toBe('2026-11-01T05:00:00.000Z')
    expect(presetUntil('forever', now, 'UTC')).toBe(FOREVER)
  })

  it('"until tomorrow 9:00" is 9:00 on the next calendar day in the person’s zone', () => {
    // 16:00 Sunday in Auckland → 09:00 Monday NZDT = 20:00Z Sunday.
    expect(presetUntil('tomorrow', now, 'Pacific/Auckland')).toBe('2026-11-01T20:00:00.000Z')
    // The same instant is 23:00 Saturday in New York → 09:00 Sunday EST.
    expect(presetUntil('tomorrow', now, 'America/New_York')).toBe('2026-11-01T14:00:00.000Z')
    expect(presetUntil('tomorrow', now, 'UTC')).toBe('2026-11-02T09:00:00.000Z')
  })

  it('crosses a DST change in each hemisphere', () => {
    // Auckland leaves NZDT at 03:00 on 5 Apr 2026: 22:00 on the 4th is +13,
    // 09:00 on the 5th is +12.
    expect(presetUntil('tomorrow', at('2026-04-04T09:00:00Z'), 'Pacific/Auckland')).toBe('2026-04-04T21:00:00.000Z')
    // Auckland enters NZDT on 27 Sep 2026.
    expect(presetUntil('tomorrow', at('2026-09-26T09:00:00Z'), 'Pacific/Auckland')).toBe('2026-09-26T20:00:00.000Z')
    // New York springs forward on 8 Mar 2026.
    expect(presetUntil('tomorrow', at('2026-03-08T01:00:00Z'), 'America/New_York')).toBe('2026-03-08T13:00:00.000Z')
    // London falls back on 25 Oct 2026.
    expect(presetUntil('tomorrow', at('2026-10-24T20:00:00Z'), 'Europe/London')).toBe('2026-10-25T09:00:00.000Z')
  })

  it('crosses a month and a year', () => {
    expect(new Date(nextDayAt(at('2026-12-31T12:00:00Z'), 'UTC', 9)).toISOString()).toBe('2027-01-01T09:00:00.000Z')
    expect(new Date(nextDayAt(at('2026-02-28T12:00:00Z'), 'UTC', 9)).toISOString()).toBe('2026-03-01T09:00:00.000Z')
  })

  it('reads a custom until as a wall time in the zone, future only', () => {
    expect(customUntil('2026-11-02T08:30', now, 'Pacific/Auckland')).toBe('2026-11-01T19:30:00.000Z')
    expect(customUntil('2026-11-01T02:00', now, 'UTC')).toBeNull()
    expect(customUntil('not a time', now, 'UTC')).toBeNull()
    expect(customUntil('2026-13-01T09:00', now, 'UTC')).toBeNull()
    expect(localInputValue(at('2026-11-01T20:00:00Z'), 'Pacific/Auckland')).toBe('2026-11-02T09:00')
  })

  it('picks the zone by O-19: the profile’s, else the browser’s', () => {
    expect(presetZone({ profile: 'Pacific/Auckland', served: 'Pacific/Auckland', browser: 'Europe/London' })).toBe('Pacific/Auckland')
    expect(presetZone({ profile: null, served: 'UTC', browser: 'Europe/London' })).toBe('Europe/London')
    // Profile not loaded yet: a served zone other than the fallback is the profile's.
    expect(presetZone({ profile: undefined, served: 'America/New_York', browser: 'Europe/London' })).toBe('America/New_York')
    expect(presetZone({ profile: undefined, served: 'UTC', browser: 'Europe/London' })).toBe('Europe/London')
  })
})

describe('the DND sentence', () => {
  const now = at('2026-11-01T03:00:00.000Z')

  it('says nothing when nothing is in force', () => {
    expect(dndView(BASE, now, 'UTC').active).toBe(false)
    expect(dndView(undefined, now, 'UTC').label).toBeNull()
  })

  it('reads a pause from pausedUntil itself, and drops it once it has passed', () => {
    const v = dndView({ ...BASE, pausedUntil: '2026-11-01T04:00:00.000Z' }, now, 'UTC', 'en-US')
    expect(v).toMatchObject({ active: true, reason: 'paused', until: at('2026-11-01T04:00:00Z'), forever: false })
    expect(v.label).toBe('Paused until 4:00 AM')
    expect(dndView({ ...BASE, pausedUntil: '2026-11-01T02:59:00.000Z', active: true, reason: 'paused' }, now, 'UTC').active).toBe(false)
  })

  it('says "until you resume" for FOREVER', () => {
    expect(dndView({ ...BASE, pausedUntil: FOREVER }, now, 'UTC')).toMatchObject({ active: true, forever: true, until: null, label: 'Paused until you resume' })
  })

  it('trusts the server for a schedule until the until it served', () => {
    const quiet = { ...BASE, active: true, reason: 'schedule' as const, until: '2026-11-01T09:00:00.000Z', schedule: { enabled: true, days: [1], start: '09:00', end: '17:00' } }
    expect(dndView(quiet, now, 'UTC', 'en-US').label).toBe('Outside working hours until 9:00 AM')
    expect(dndView(quiet, at('2026-11-01T09:00:01Z'), 'UTC').active).toBe(false)
  })

  it('formats the until relative to today in the zone', () => {
    expect(formatUntil(at('2026-11-01T20:00:00Z'), now, 'Pacific/Auckland', 'en-US')).toBe('tomorrow 9:00 AM')
    expect(formatUntil(at('2026-11-01T05:00:00Z'), now, 'Pacific/Auckland', 'en-US')).toBe('6:00 PM')
    expect(formatUntil(at('2026-11-04T20:00:00Z'), now, 'Pacific/Auckland', 'en-US')).toBe('Thu 9:00 AM')
    expect(formatUntil(at('2026-11-20T20:00:00Z'), now, 'Pacific/Auckland', 'en-US')).toBe('Nov 21, 9:00 AM')
    expect(formatUntil(FOREVER_MS, now, 'UTC')).toBeNull()
  })
})

describe('working hours', () => {
  it('validates what the server would refuse', () => {
    expect(validSchedule({ enabled: true, days: [1, 2], start: '09:00', end: '17:00' })).toBeNull()
    expect(validSchedule({ enabled: true, days: [1], start: '9:00', end: '17:00' })).toMatch(/Start/)
    expect(validSchedule({ enabled: true, days: [1], start: '09:00', end: '24:00' })).toMatch(/End/)
    expect(validSchedule({ enabled: true, days: [1], start: '09:00', end: '09:00' })).toMatch(/differ/)
    expect(validSchedule({ enabled: true, days: [], start: '09:00', end: '17:00' })).toMatch(/day/)
    expect(validSchedule({ enabled: false, days: [], start: '09:00', end: '17:00' })).toBeNull()
  })

  it('toggles days sorted, and summarises', () => {
    expect(toggleDay([1, 3], 2)).toEqual([1, 2, 3])
    expect(toggleDay([1, 2, 3], 2)).toEqual([1, 3])
    expect(scheduleSummary({ enabled: true, days: [5, 4, 3, 2, 1], start: '09:00', end: '17:00' })).toBe('Mon–Fri, 09:00–17:00')
    expect(scheduleSummary({ enabled: true, days: [0, 1, 2, 3, 4, 5, 6], start: '22:00', end: '06:00' })).toBe('Every day, 22:00–06:00 (overnight)')
    expect(scheduleSummary({ enabled: true, days: [1, 3], start: '08:00', end: '12:00' })).toBe('Mon, Wed, 08:00–12:00')
  })
})
