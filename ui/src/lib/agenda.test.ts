import { describe, expect, it } from 'vitest'
import { formatAgendaTime, groupAgendaByDay, type AgendaEvent } from '@/lib/agenda'

// Day-grouping for the Home agenda. The two traps this file pins:
//   - grouping is on LOCAL days (the browser's timezone is the right one for
//     a person's own agenda), so 23:59 and 00:01 split across a midnight even
//     when both are the same UTC day;
//   - an all-day event's date-only start ('2026-08-26') IS its day — parsing
//     it as an instant gives UTC midnight, which files it a day early in
//     every timezone behind UTC.
// Test times are built as LOCAL Dates and serialized with toISOString(), so
// the round trip lands on the same local day no matter the runner's timezone.

const at = (y: number, m: number, d: number, h: number, min = 0): string =>
  new Date(y, m - 1, d, h, min).toISOString()

const ev = (id: string, start: string | null, allDay = false): AgendaEvent => ({
  id,
  summary: id,
  start,
  end: null,
  allDay,
  location: null,
  htmlLink: null,
})

// Mon Aug 24 2026, 09:00 local.
const now = new Date(2026, 7, 24, 9, 0)

describe('groupAgendaByDay', () => {
  it('groups by local day and labels Today and Tomorrow', () => {
    // Within a day the SERVER's order is preserved (Google answers
    // soonest-first, all-day first) — the helper groups, it does not re-sort.
    const days = groupAgendaByDay(
      [
        ev('later-today', at(2026, 8, 24, 23, 30)),
        ev('first-today', at(2026, 8, 24, 10, 0)),
        ev('tomorrow-morning', at(2026, 8, 25, 8, 0)),
      ],
      now,
    )
    expect(days.map((d) => [d.key, d.label, d.events.map((e) => e.id)])).toEqual([
      ['2026-08-24', 'Today', ['later-today', 'first-today']],
      ['2026-08-25', 'Tomorrow', ['tomorrow-morning']],
    ])
  })

  it('splits across a local midnight even when both events share a UTC day', () => {
    // 23:59 and 00:01 local are different days; in UTC+X both can be Aug 24.
    const days = groupAgendaByDay(
      [ev('late', at(2026, 8, 24, 23, 59)), ev('early', at(2026, 8, 25, 0, 1))],
      now,
    )
    expect(days.map((d) => d.key)).toEqual(['2026-08-24', '2026-08-25'])
  })

  it('labels days past tomorrow by weekday, sorted soonest first', () => {
    const days = groupAgendaByDay(
      // Inserted out of order — the groups must still come out sorted.
      [ev('thursday', at(2026, 8, 27, 12, 0)), ev('wednesday', at(2026, 8, 26, 12, 0))],
      now,
    )
    expect(days.map((d) => d.key)).toEqual(['2026-08-26', '2026-08-27'])
    // The label format itself is the locale's business; pin only that it is
    // the weekday of the LOCAL date (Thu Aug 27 2026), not a shifted day.
    expect(days[1]!.label).toBe(
      new Date(2026, 7, 27).toLocaleDateString(undefined, { weekday: 'long', month: 'short', day: 'numeric' }),
    )
  })

  it('keys an all-day event by its literal date — never the UTC-midnight shift', () => {
    const days = groupAgendaByDay([ev('ooo', '2026-08-26', true)], now)
    expect(days.map((d) => d.key)).toEqual(['2026-08-26'])
  })
})

describe('formatAgendaTime', () => {
  it('says All day for all-day events and a bare time otherwise', () => {
    expect(formatAgendaTime(ev('a', '2026-08-26', true))).toBe('All day')
    expect(formatAgendaTime(ev('t', at(2026, 8, 24, 10, 30)))).toBe(
      new Date(2026, 7, 24, 10, 30).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }),
    )
    expect(formatAgendaTime(ev('n', null))).toBe('')
  })
})
