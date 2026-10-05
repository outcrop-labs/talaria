import { describe, expect, it } from 'vitest'
import { dayDividers, dayLabel } from '@/lib/day-dividers'

// The transcript's date dividers (R1, AE1). The pins that matter:
//   - one divider per CALENDAR day in the viewer's zone, before that day's
//     first message — never per message, never per UTC day;
//   - "Today"/"Yesterday" are computed against a SUPPLIED clock, so the test
//     (and a long-open tab) never depends on when it runs;
//   - older days read "Friday, September 4th", with the year only when it is
//     not the current one.

const NOW = Date.parse('2026-10-05T12:00:00Z') // a Monday
const at = (m: { at: string }) => m.at

describe('dayDividers', () => {
  it('AE1: one "Friday, September 4th" divider, then one "Today" divider for two of today', () => {
    const items = [{ at: '2026-09-04T15:00:00Z' }, { at: '2026-10-05T09:00:00Z' }, { at: '2026-10-05T10:30:00Z' }]
    expect(dayDividers(items, at, { timeZone: 'UTC', now: NOW })).toEqual([
      { index: 0, key: '2026-09-04', label: 'Friday, September 4th' },
      { index: 1, key: '2026-10-05', label: 'Today' },
    ])
  })

  it('labels yesterday "Yesterday"', () => {
    const items = [{ at: '2026-10-04T23:00:00Z' }, { at: '2026-10-05T01:00:00Z' }]
    expect(dayDividers(items, at, { timeZone: 'UTC', now: NOW }).map((d) => d.label)).toEqual(['Yesterday', 'Today'])
  })

  it('respects the supplied zone: 23:30 UTC on day N is day N+1 in Asia/Tokyo', () => {
    const items = [{ at: '2026-10-03T12:00:00Z' }, { at: '2026-10-03T23:30:00Z' }]
    const utc = dayDividers(items, at, { timeZone: 'UTC', now: NOW })
    const tokyo = dayDividers(items, at, { timeZone: 'Asia/Tokyo', now: NOW })
    expect(utc).toHaveLength(1)
    expect(tokyo.map((d) => [d.index, d.key])).toEqual([
      [0, '2026-10-03'],
      [1, '2026-10-04'],
    ])
    // NOW is 21:00 on Oct 5 in Tokyo, so Oct 4 is yesterday there.
    expect(tokyo[1]!.label).toBe('Yesterday')
  })

  it('returns no dividers for an empty list and exactly one for a single message', () => {
    expect(dayDividers([], at, { timeZone: 'UTC', now: NOW })).toEqual([])
    expect(dayDividers([{ at: '2026-10-05T08:00:00Z' }], at, { timeZone: 'UTC', now: NOW })).toHaveLength(1)
  })

  it('accepts Date and epoch-ms timestamps and skips unparseable ones', () => {
    const items: Array<{ at: string | number | Date }> = [
      { at: new Date('2026-10-01T08:00:00Z') },
      { at: 'nonsense' },
      { at: Date.parse('2026-10-02T08:00:00Z') },
    ]
    expect(dayDividers(items, (m) => m.at, { timeZone: 'UTC', now: NOW }).map((d) => d.index)).toEqual([0, 2])
  })

  it('falls back to the browser zone for an unknown zone instead of throwing', () => {
    expect(() => dayDividers([{ at: '2026-10-05T08:00:00Z' }], at, { timeZone: 'Not/AZone', now: NOW })).not.toThrow()
  })
})

describe('dayLabel', () => {
  const label = (iso: string) => dayLabel(iso, { timeZone: 'UTC', now: NOW })

  it('suffixes ordinals correctly, teens included', () => {
    const cases: Array<[number, string]> = [
      [1, '1st'], [2, '2nd'], [3, '3rd'], [4, '4th'], [11, '11th'], [12, '12th'],
      [13, '13th'], [21, '21st'], [22, '22nd'], [23, '23rd'],
    ]
    for (const [day, suffix] of cases) {
      expect(label(`2026-08-${String(day).padStart(2, '0')}T12:00:00Z`)).toMatch(new RegExp(` ${suffix}$`))
    }
  })

  it('adds the year only when it is not the current one', () => {
    expect(label('2026-04-07T12:00:00Z')).toBe('Tuesday, April 7th')
    expect(label('2025-12-31T12:00:00Z')).toBe('Wednesday, December 31st, 2025')
  })
})
