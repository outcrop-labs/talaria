// @vitest-environment runes
//
// The status stub's contract: W2-R's setters move exactly the field they
// name, and W2-C's getter sees it.
import { beforeEach, describe, expect, it } from 'vitest'
import {
  noteStreamEvent,
  notifyStatus,
  resetNotifyStatus,
  setHubRole,
  setPresence,
  setStreamState,
} from './notify-status.svelte'

describe('notify-status', () => {
  beforeEach(() => resetNotifyStatus())

  it('starts closed, with nobody looking and no role', () => {
    expect(notifyStatus()).toEqual({
      stream: { state: 'closed', since: null, lastEventAt: null },
      presence: { looking: null, active: null, clients: 0, self: null },
      hubRole: 'unknown',
    })
  })

  it('records when the stream changed state, and keeps it across a repeat', () => {
    setStreamState('connecting', 100)
    setStreamState('open', 200)
    setStreamState('open', 900)
    noteStreamEvent(950)
    expect(notifyStatus().stream).toEqual({ state: 'open', since: 200, lastEventAt: 950 })
    setStreamState('reconnecting', 1_000)
    expect(notifyStatus().stream.since).toBe(1_000)
  })

  it('merges presence and sets the role', () => {
    setPresence({ self: 'tab-aaaaaaaa' })
    setPresence({ looking: 'tab-aaaaaaaa', clients: 2 })
    setHubRole('leader')
    expect(notifyStatus().presence).toEqual({ looking: 'tab-aaaaaaaa', active: null, clients: 2, self: 'tab-aaaaaaaa' })
    expect(notifyStatus().hubRole).toBe('leader')
  })
})
