import { describe, expect, it } from 'vitest'
import { describeSchedule, editPatch, parseSchedule, schedToString } from './agent-crons'

// A job as the api serves it: the schedule is Hermes's own display string,
// which is what the edit form is seeded from.
const job = { name: 'Morning digest', schedule: '30 7 * * *', prompt: 'Summarise the overnight tickets.' }

// What the form hands back when nobody touched it: the same fields, run
// through the same trim and schedule round-trip the row's save does.
const untouched = (j: typeof job) => ({
  name: j.name.trim(),
  schedule: schedToString(parseSchedule(j.schedule)),
  prompt: j.prompt.trim(),
})

describe('editPatch: an edit sends only what changed', () => {
  it('sends nothing for an untouched form', () => {
    expect(editPatch(job, untouched(job))).toEqual({})
  })

  it('sends a rename alone — the schedule is not re-sent, so it is not re-timed', () => {
    expect(editPatch(job, { ...untouched(job), name: 'Daily digest' })).toEqual({ name: 'Daily digest' })
  })

  it('sends a reschedule alone', () => {
    expect(editPatch(job, { ...untouched(job), schedule: '0 8 * * *' })).toEqual({ schedule: '0 8 * * *' })
  })

  it('sends a prompt change alone', () => {
    expect(editPatch(job, { ...untouched(job), prompt: 'Summarise yesterday.' })).toEqual({ prompt: 'Summarise yesterday.' })
  })

  it('leaves out a prompt longer than the cap when only the name changes', () => {
    // A job written through the CLI before the 5,000-character cap: the api
    // would refuse its prompt, so a rename must not carry it.
    const legacy = { ...job, prompt: 'x'.repeat(6_000) }
    expect(editPatch(legacy, { ...untouched(legacy), name: 'Renamed' })).toEqual({ name: 'Renamed' })
  })

  it('treats whitespace Hermes kept around a field as unchanged', () => {
    const padded = { ...job, name: ' Morning digest ', prompt: 'Summarise the overnight tickets.\n' }
    expect(editPatch(padded, untouched(padded))).toEqual({})
  })

  it('round-trips every schedule shape the builder rebuilds, so none is re-sent untouched', () => {
    for (const schedule of ['every 30m', 'every 360m', 'every 2h', '30 7 * * *', '0 9 * * 1-5', '0 9 * * 3', '15 6 1 * *', 'once at 2026-11-01T09:00:00', '*/15 9-17 * * 1-5']) {
      const j = { ...job, schedule }
      expect(editPatch(j, untouched(j))).toEqual({})
    }
  })
})

describe('describeSchedule: an interval reads in the largest whole unit', () => {
  // Hermes stores every interval in minutes (`every 6h` is saved as
  // `{"minutes": 360, "display": "every 360m"}`), and the api serves that
  // display string back. The row folds it into hours or days.
  it('folds whole hours and whole days out of Hermes minutes', () => {
    expect(describeSchedule('every 360m')).toBe('Every 6 hours')
    expect(describeSchedule('every 60m')).toBe('Every hour')
    expect(describeSchedule('every 120m')).toBe('Every 2 hours')
    expect(describeSchedule('every 1440m')).toBe('Every day')
    expect(describeSchedule('every 2880m')).toBe('Every 2 days')
    expect(describeSchedule('every 2160m')).toBe('Every 36 hours')
  })

  it('keeps minutes that are not a whole hour', () => {
    expect(describeSchedule('every 90m')).toBe('Every 90 minutes')
    expect(describeSchedule('every 30m')).toBe('Every 30 minutes')
    expect(describeSchedule('every 1m')).toBe('Every minute')
  })

  it('reads what the builder writes the same way', () => {
    expect(describeSchedule('every 6h')).toBe('Every 6 hours')
    expect(describeSchedule('every 1h')).toBe('Every hour')
    expect(describeSchedule('every 24h')).toBe('Every day')
    expect(describeSchedule('every 48h')).toBe('Every 2 days')
  })

  it('leaves the stored string alone: the edit form still opens on the minutes', () => {
    expect(parseSchedule('every 360m')).toMatchObject({ mode: 'interval', every: 360, unit: 'm' })
    expect(schedToString(parseSchedule('every 360m'))).toBe('every 360m')
  })
})
