import { describe, expect, it } from 'vitest'
import { MIN_LOADED_MS, shouldAsk, turnOnResult, type AskInput } from './notify-prompt'

const NOW = Date.parse('2026-10-31T12:00:00.000Z')
const ok: AskInput = {
  permission: 'default',
  inShell: false,
  memory: { dismissedAt: null, count: 0, nextAskAt: null, granted: false },
  now: NOW,
  loadedAt: NOW - MIN_LOADED_MS,
  hasBeenActive: true,
  trigger: true,
}

describe('shouldAsk (the soft ask)', () => {
  it('asks when every condition holds', () => {
    expect(shouldAsk(ok)).toBe(true)
  })

  it('never asks on load: no trigger, no touch, or too soon', () => {
    expect(shouldAsk({ ...ok, trigger: false })).toBe(false)
    expect(shouldAsk({ ...ok, hasBeenActive: false })).toBe(false)
    expect(shouldAsk({ ...ok, loadedAt: NOW - MIN_LOADED_MS + 1 })).toBe(false)
  })

  it('asks only from the default state, and never in the shell', () => {
    for (const permission of ['granted', 'denied', 'insecure', 'unsupported'] as const) {
      expect(shouldAsk({ ...ok, permission })).toBe(false)
    }
    expect(shouldAsk({ ...ok, inShell: true })).toBe(false)
  })

  it('waits for the server record, honors nextAskAt, retires at three, stops on grant', () => {
    expect(shouldAsk({ ...ok, memory: undefined })).toBe(false)
    const later = new Date(NOW + 1000).toISOString()
    const earlier = new Date(NOW - 1000).toISOString()
    expect(shouldAsk({ ...ok, memory: { dismissedAt: earlier, count: 1, nextAskAt: later, granted: false } })).toBe(false)
    expect(shouldAsk({ ...ok, memory: { dismissedAt: earlier, count: 1, nextAskAt: earlier, granted: false } })).toBe(true)
    expect(shouldAsk({ ...ok, memory: { dismissedAt: earlier, count: 2, nextAskAt: earlier, granted: false } })).toBe(true)
    expect(shouldAsk({ ...ok, memory: { dismissedAt: earlier, count: 3, nextAskAt: null, granted: false } })).toBe(false)
    expect(shouldAsk({ ...ok, memory: { dismissedAt: null, count: 0, nextAskAt: null, granted: true } })).toBe(false)
  })
})

describe('turnOnResult', () => {
  it('reads a closed prompt as "try again", not as a dismissal', () => {
    expect(turnOnResult('granted')).toEqual({ kind: 'on' })
    expect(turnOnResult('default')).toEqual({
      kind: 'closed',
      message: "The browser's prompt closed without an answer — Turn on to try again.",
    })
    expect(turnOnResult('denied')).toEqual({ kind: 'blocked' })
    expect(turnOnResult('insecure').kind).toBe('unavailable')
    expect(turnOnResult('unsupported').kind).toBe('unavailable')
  })
})
