import { afterAll, describe, expect, it } from 'vitest'
import { dateInputValue, dueIsoFromDateInput, startIsoFromDateInput } from '@/lib/dates'

// Both writers return `string | null` — null only for input that is not a
// `YYYY-MM-DD` day, which is the contract the last test here pins. Every day
// string below is well-formed, so `!` is a statement of fact, not a shortcut.
const due = (ymd: string): string => dueIsoFromDateInput(ymd)!
const start = (ymd: string): string => startIsoFromDateInput(ymd)!

// These helpers are entirely about LOCAL time, so the test has to control the
// zone. Node re-reads process.env.TZ per Date operation, and vitest isolates
// each test file in its own process, so setting it here cannot leak sideways.
const ORIGINAL_TZ = process.env.TZ
const inZone = <T>(tz: string, fn: () => T): T => {
  process.env.TZ = tz
  try {
    return fn()
  } finally {
    process.env.TZ = ORIGINAL_TZ
  }
}
afterAll(() => {
  process.env.TZ = ORIGINAL_TZ
})

// A deliberately awkward spread: west of Greenwich (where the bug lived), far
// east (where the ISO date is the PREVIOUS day), sub-hour offsets, and UTC.
const ZONES = [
  'America/Los_Angeles', // −08:00 / −07:00
  'America/New_York', // −05:00 / −04:00
  'America/St_Johns', // −03:30 / −02:30
  'UTC',
  'Europe/Berlin', // +01:00 / +02:00
  'Asia/Kathmandu', // +05:45, never DST
  'Asia/Tokyo', // +09:00, never DST
  'Pacific/Kiritimati', // +14:00, the far edge
]

const DAYS = ['2026-01-15', '2026-03-08', '2026-06-30', '2026-07-30', '2026-11-01', '2026-12-31', '2027-02-28']

describe('dueDateIso / startDateIso — business-day convention', () => {
  it('puts a due date at 17:00 local and a start date at 09:00 local', () => {
    inZone('America/New_York', () => {
      // 2026-07-30 is EDT (UTC−4): 17:00 → 21:00Z, 09:00 → 13:00Z.
      expect(due('2026-07-30')).toBe('2026-07-30T21:00:00.000Z')
      expect(start('2026-07-30')).toBe('2026-07-30T13:00:00.000Z')
    })
  })

  it('tracks daylight saving rather than a fixed offset', () => {
    inZone('America/New_York', () => {
      expect(due('2026-01-15')).toBe('2026-01-15T22:00:00.000Z') // EST, UTC−5
      expect(due('2026-07-15')).toBe('2026-07-15T21:00:00.000Z') // EDT, UTC−4
    })
  })

  it('keeps a due date after its start date on the same day', () => {
    for (const tz of ZONES) {
      inZone(tz, () => {
        for (const day of DAYS) {
          expect(new Date(due(day)).getTime()).toBeGreaterThan(new Date(start(day)).getTime())
        }
      })
    }
  })

  it('parses the day string field-by-field, not as a UTC instant', () => {
    // `new Date('2026-07-30')` is UTC midnight — still the 29th anywhere west of
    // Greenwich. If the helper ever regressed to that, this is the assertion
    // that catches it.
    inZone('America/Los_Angeles', () => {
      expect(dateInputValue(due('2026-07-30'))).toBe('2026-07-30')
      expect(new Date('2026-07-30').getDate()).toBe(29) // the trap it avoids
    })
  })


})

describe('dateInputValue', () => {
  it('renders the LOCAL day, not the UTC one', () => {
    inZone('America/New_York', () => {
      const iso = due('2026-07-30')
      // The bug this replaced: the raw ISO string says the 31st, because 17:00
      // in New York is already tomorrow in UTC.
      expect(iso.slice(0, 10)).toBe('2026-07-30')
      expect(dateInputValue('2026-07-30T23:30:00.000Z')).toBe('2026-07-30')
      expect(dateInputValue('2026-07-31T01:30:00.000Z')).toBe('2026-07-30')
      expect('2026-07-31T01:30:00.000Z'.slice(0, 10)).toBe('2026-07-31') // what slicing would have shown
    })
  })

  it('renders the local day east of Greenwich, where the ISO day runs BEHIND', () => {
    inZone('Pacific/Kiritimati', () => {
      const iso = start('2026-07-30') // 09:00 at UTC+14
      expect(iso).toBe('2026-07-29T19:00:00.000Z')
      expect(iso.slice(0, 10)).toBe('2026-07-29') // slicing would lose a day here too
      expect(dateInputValue(iso)).toBe('2026-07-30')
    })
  })

  it('zero-pads month and day', () => {
    inZone('UTC', () => {
      expect(dateInputValue(due('2026-01-05'))).toBe('2026-01-05')
      expect(dateInputValue('2026-01-05T12:00:00.000Z')).toBe('2026-01-05')
    })
  })

  it('returns an empty string for nothing and for garbage', () => {
    expect(dateInputValue(null)).toBe('')
    expect(dateInputValue(undefined)).toBe('')
    expect(dateInputValue('')).toBe('')
    expect(dateInputValue('not a date')).toBe('')
    expect(dateInputValue('2026-13-45T99:99:99Z')).toBe('')
  })
})

describe('the round-trip that was broken', () => {
  it('a day picked in any zone renders back as the same day, for due AND start', () => {
    for (const tz of ZONES) {
      inZone(tz, () => {
        for (const day of DAYS) {
          expect({ tz, day, back: dateInputValue(due(day)) }).toEqual({ tz, day, back: day })
          expect({ tz, day, back: dateInputValue(start(day)) }).toEqual({ tz, day, back: day })
        }
      })
    }
  })

  it('is stable when re-saved: reading a stored instant and writing it back is a no-op', () => {
    for (const tz of ZONES) {
      inZone(tz, () => {
        for (const day of DAYS) {
          const first = due(day)
          expect(due(dateInputValue(first))).toBe(first)
        }
      })
    }
  })

  it('a due date created in a UTC-negative zone still reads as that day in the SAME zone', () => {
    // The concrete symptom from the audit: the board pill wrote 17:00 local, the
    // rail read `iso.slice(0, 10)`, and the ticket showed the day after.
    inZone('America/Los_Angeles', () => {
      const stored = due('2026-12-31')
      expect(stored).toBe('2027-01-01T01:00:00.000Z') // next day, next YEAR, in UTC
      expect(dateInputValue(stored)).toBe('2026-12-31')
    })
  })
})

describe('the null contract', () => {
  it('refuses anything that is not a YYYY-MM-DD day', () => {
    // Deliberately stricter than the Date constructor: an onChange handler that
    // persisted `new Date('nonsense').toISOString()` would throw RangeError, so
    // these return null and the caller declines to save rather than crashing.
    for (const bad of ['', 'nonsense', '2026-7-30', '30-07-2026', '2026-07-30T12:00:00Z'])
      expect(dueIsoFromDateInput(bad)).toBeNull()
    expect(startIsoFromDateInput('2026-13-45')).toBeNull() // shaped right, not a real day
  })
})
