import { describe, expect, it } from 'vitest'
import type { ScheduleRow, ScheduleTone } from '@/lib/schedules'
import { TONE_DOT, TONE_TITLE, nextRunLabel } from './schedules-panel'

// The row's two derived readings. The case that matters is the null live half
// (the agent could not be asked): it must not read as paused, and it must not
// name a next run it does not know.

type Row = Pick<ScheduleRow, 'enabled' | 'state' | 'lastStatus' | 'nextRunAt'>

const row = (over: Partial<Row> = {}): Row => ({
  enabled: true,
  state: 'scheduled',
  lastStatus: null,
  nextRunAt: '2026-10-17T09:00:00.000Z',
  ...over,
})

const dead = (over: Partial<Row> = {}): Row =>
  row({ enabled: null, state: null, lastStatus: null, nextRunAt: null, ...over })

describe('TONE_DOT', () => {
  it('gives every tone its own dot, and never reads "cannot ask" as a failure', () => {
    expect(TONE_DOT.ok).toBe('ok')
    expect(TONE_DOT.paused).toBe('idle')
    expect(TONE_DOT.error).toBe('danger')
    expect(TONE_DOT.unknown).toBe('warn')
    expect(TONE_DOT.unknown).not.toBe(TONE_DOT.error)
  })

  it('every tone is spelled out in words for the tooltip', () => {
    for (const tone of ['ok', 'paused', 'error', 'unknown'] as ScheduleTone[]) {
      expect(TONE_TITLE[tone].trim()).not.toBe('')
      expect(TONE_TITLE[tone]).not.toMatch(/…|\.\.\./)
    }
  })
})

describe('nextRunLabel', () => {
  it('names the next run when the agent answered', () => {
    // The formatting is fmtNext's (and the viewer's locale); what this pins
    // is that the line is the next run and not something else.
    expect(nextRunLabel(row())).toMatch(/^next /)
  })

  it('a paused job says paused, not a time it will not keep', () => {
    expect(nextRunLabel(row({ enabled: false }))).toBe('paused')
    expect(nextRunLabel(row({ state: 'paused' }))).toBe('paused')
  })

  it('an agent that could not be asked waits on it — never "paused"', () => {
    expect(nextRunLabel(dead())).toBe('waiting on the agent')
    // Even with a stale next-run time on the stored half, there is nothing
    // live to confirm it.
    expect(nextRunLabel(dead({ nextRunAt: '2026-10-17T09:00:00.000Z' }))).toBe('waiting on the agent')
  })

  it("a failed last run keeps the next run's time: it is still going to fire", () => {
    expect(nextRunLabel(row({ lastStatus: 'error' }))).toMatch(/^next /)
  })

  it('a live job with no next run says nothing rather than inventing a word', () => {
    expect(nextRunLabel(row({ nextRunAt: null }))).toBe('')
  })
})
