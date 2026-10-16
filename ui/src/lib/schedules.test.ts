import { describe, expect, it } from 'vitest'
import { scheduleTone, type ScheduleRow } from '@/lib/schedules'

// The tone function exists for one mistake the wire makes easy: the live half
// of a schedule is null when the container could not be read, and a surface
// that folds that into "error" — or into "paused" — reports a healthy
// schedule as broken every time its agent rolls. These cases pin the four
// answers apart.

type Live = Pick<ScheduleRow, 'enabled' | 'state' | 'lastStatus'>

/** A row whose agent answered: live and ticking unless a case says otherwise. */
const live = (over: Partial<Live> = {}): Live => ({ enabled: true, state: 'scheduled', lastStatus: null, ...over })

describe('scheduleTone', () => {
  it('reads an agent that could not be asked as unknown, not as a failure', () => {
    expect(scheduleTone({ enabled: null, state: null, lastStatus: null })).toBe('unknown')
  })

  it('reads a live, enabled job as ok', () => {
    expect(scheduleTone(live())).toBe('ok')
    expect(scheduleTone(live({ lastStatus: 'ok' }))).toBe('ok')
  })

  it('reads both spellings of stopped as paused', () => {
    expect(scheduleTone(live({ enabled: false }))).toBe('paused')
    expect(scheduleTone(live({ state: 'paused' }))).toBe('paused')
  })

  it("reads the agent's own words for a bad run as error", () => {
    for (const lastStatus of ['error', 'failed', 'Error: timed out', 'FAILURE']) {
      expect(scheduleTone(live({ lastStatus }))).toBe('error')
    }
  })

  it('keeps a paused job paused even when its last run failed', () => {
    // Paused outranks the last outcome: the row's story is that it is not
    // going to run again, which is the more actionable of the two.
    expect(scheduleTone(live({ enabled: false, lastStatus: 'error' }))).toBe('paused')
  })
})
