import { describe, expect, it } from 'vitest'
import { briefWindow, fireHour, localMoment, nextBriefAt, type BriefConfig } from './daily-brief-config'

// The clock. Every assertion here is a way somebody loses a brief for a day:
// a fire hour computed wrong, a window that only opens on the exact minute the
// scheduler happens to tick, or an evening brief filed under the wrong date and
// therefore re-opened as a second one the next morning.

const config = (over: Partial<BriefConfig> = {}): BriefConfig => ({
  workdayStartHour: 9,
  leadHours: 2,
  timeZone: 'UTC',
  sweepMinutes: 5,
  ...over,
})

describe('fireHour', () => {
  it('is the workday start less the lead', () => {
    expect(fireHour(config())).toBe(7)
  })

  it('wraps into the previous evening rather than clamping at midnight', () => {
    // A 01:00 start with a 2h lead means 23:00 the night before. Clamping to 0
    // would silently move a night-shift brief to midnight, which is neither
    // what was configured nor two hours before anything.
    expect(fireHour(config({ workdayStartHour: 1 }))).toBe(23)
  })
})

describe('briefWindow', () => {
  it('is not due before the fire hour', () => {
    expect(briefWindow(config(), 'UTC', new Date('2026-08-17T06:59:00Z')).due).toBe(false)
  })

  it('stays due for the rest of the day, so a missed tick is recoverable', () => {
    // THE POINT OF A WINDOW RATHER THAN AN EQUALITY. The scheduler can miss a
    // tick — a deploy, a lease held by an instance that died — and an
    // `hour === fireHour` test would cost that person their whole brief.
    expect(briefWindow(config(), 'UTC', new Date('2026-08-17T07:00:00Z')).due).toBe(true)
    expect(briefWindow(config(), 'UTC', new Date('2026-08-17T15:30:00Z')).due).toBe(true)
    expect(briefWindow(config(), 'UTC', new Date('2026-08-17T23:59:00Z')).due).toBe(true)
  })

  it('dates an ordinary brief with the local day it opens on', () => {
    expect(briefWindow(config(), 'UTC', new Date('2026-08-17T07:30:00Z')).date).toBe('2026-08-17')
  })

  it('dates a wrapped evening brief with the workday it is FOR', () => {
    const wrapped = config({ workdayStartHour: 1 })
    // Written Monday 23:00, for Tuesday. `brief_date` is half the unique key,
    // so filing this under Monday would have it re-opened as a SECOND brief
    // when Tuesday actually arrived.
    expect(briefWindow(wrapped, 'UTC', new Date('2026-08-17T23:10:00Z')).date).toBe('2026-08-18')
    // And the small hours of Tuesday are still Tuesday's brief.
    const early = briefWindow(wrapped, 'UTC', new Date('2026-08-18T00:30:00Z'))
    expect(early).toEqual({ due: true, date: '2026-08-18' })
  })

  it('closes the wrapped window once the workday starts', () => {
    const wrapped = config({ workdayStartHour: 1 })
    expect(briefWindow(wrapped, 'UTC', new Date('2026-08-18T02:00:00Z')).due).toBe(false)
  })

  it('reads the hour in the person’s zone, not the server’s', () => {
    // 07:00 in New York is 11:00 UTC. A window computed on the server clock
    // would open this brief four hours early and title it correctly, which is
    // the failure that looks like it works.
    expect(briefWindow(config(), 'America/New_York', new Date('2026-08-17T10:59:00Z')).due).toBe(false)
    expect(briefWindow(config(), 'America/New_York', new Date('2026-08-17T11:01:00Z')).due).toBe(true)
  })

  it('falls back to UTC on an unreadable zone instead of throwing', () => {
    // A typo in one org-wide settings row must not stop every brief in the
    // workspace — that is the exact failure the scheduler exists to prevent.
    expect(() => briefWindow(config(), 'Not/AZone', new Date('2026-08-17T07:30:00Z'))).not.toThrow()
    expect(briefWindow(config(), 'Not/AZone', new Date('2026-08-17T07:30:00Z'))).toEqual({ due: true, date: '2026-08-17' })
  })
})

describe('nextBriefAt', () => {
  it('is the next occurrence of the fire hour in the person’s zone', () => {
    const next = nextBriefAt(config(), 'UTC', new Date('2026-08-17T06:10:00Z'))
    expect(next.toISOString()).toBe('2026-08-17T07:00:00.000Z')
  })

  it('rolls to tomorrow once today’s has passed', () => {
    const next = nextBriefAt(config(), 'UTC', new Date('2026-08-17T09:00:00Z'))
    expect(next.toISOString()).toBe('2026-08-18T07:00:00.000Z')
  })

  it('lands on the right wall-clock hour across a DST boundary', () => {
    // US DST ends 2026-11-01. Walking the clock an hour at a time and re-reading
    // the LOCAL hour is what makes this land on 07:00 local either side of it;
    // adding 24h of milliseconds would drift by the hour the zone gave back.
    const next = nextBriefAt(config(), 'America/New_York', new Date('2026-10-31T20:00:00Z'))
    expect(localMoment('America/New_York', next).hour).toBe(7)
  })
})
