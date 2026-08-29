import { describe, expect, it, vi } from 'vitest'

// The frequency floor (#243): a cron is an agent turn is LLM spend, and
// `* * * * *` — 1,440 turns a day per agent — used to be accepted verbatim.
// minIntervalMinutes is the pure half (what a schedule means); the floor's
// enforcement is one comparison against it in createCronJob/editCronJob.
vi.mock('./db/pg', () => ({ db: () => Promise.reject(new Error('no database in this test')) }))
vi.mock('./audit', () => ({ getSetting: async (_k: string, fallback: unknown) => fallback }))
vi.mock('./docker-exec', () => ({
  agentContainer: async () => 'stub',
  dockerExec: async () => ({ stdout: '', stderr: '' }),
}))

const { minIntervalMinutes } = await import('./agent-crons')

describe('minIntervalMinutes — the shortest gap a schedule can fire', () => {
  it('interval forms carry their unit', () => {
    expect(minIntervalMinutes('30m')).toBe(30)
    expect(minIntervalMinutes('every 2h')).toBe(120)
    expect(minIntervalMinutes('90s')).toBe(1.5)
    expect(minIntervalMinutes('1d')).toBe(1440)
    expect(minIntervalMinutes('Every 45 Min')).toBe(45)
  })

  it('every-minute and stepped crons expose their true pace', () => {
    expect(minIntervalMinutes('* * * * *')).toBe(1)
    expect(minIntervalMinutes('*/5 * * * *')).toBe(5)
    expect(minIntervalMinutes('15,45 * * * *')).toBe(30)
    expect(minIntervalMinutes('0,5,10 * * * *')).toBe(5)
  })

  it('a single minute value fires at most hourly, whatever the other fields say', () => {
    expect(minIntervalMinutes('0 * * * *')).toBe(60)
    expect(minIntervalMinutes('30 9 * * 1-5')).toBe(60)
  })

  it('gaps wrap past the hour — 59 then 0 is one minute apart', () => {
    expect(minIntervalMinutes('0,59 * * * *')).toBe(1)
  })

  it('null for anything it cannot prove — Hermes stays the validator for exotic schedules', () => {
    expect(minIntervalMinutes('at 9am on weekdays')).toBeNull()
    expect(minIntervalMinutes('* * *')).toBeNull() // not 5 fields
    expect(minIntervalMinutes('*/x * * * *')).toBeNull() // unparseable step
    expect(minIntervalMinutes('61 * * * *')).toBeNull() // out of range
  })
})
