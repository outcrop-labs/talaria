// The turn-landing edge (TALA-33), tested at its pure helper — no
// component-test precedent exists in ui/src, so the edge was extracted from
// ChatView.svelte into turn-landing.ts and these tests drive it directly.
//
// Each case is a SEQUENCE of evaluations: ChatView's effect runs per reactive
// tick, and the interesting behaviors are transitions, not snapshots.
import { describe, expect, it } from 'vitest'
import { landingTransition, type TurnArm } from './turn-landing'

const CONV = 'plan-1'
const OTHER = 'plan-2'

// One reactive tick: evaluate the edge, carry the arm forward.
const tick = (arm: TurnArm, inFlight: boolean, landed: boolean, convId: string | null = CONV) =>
  landingTransition(arm, convId, inFlight, landed)

describe('landingTransition', () => {
  it('a landed turn this thread armed fires onTurnComplete', () => {
    // Streaming starts (last row the user's): arm.
    let s = tick(null, true, false)
    expect(s.arm).toBe(CONV)
    expect(s.fire).toBe(false)
    // Stream ends, last row a complete assistant reply: fire.
    s = tick(s.arm, false, true)
    expect(s.fire).toBe(true)
    expect(s.arm).toBeNull()
    // A second landing with no arm does not re-fire.
    s = tick(s.arm, false, true)
    expect(s.fire).toBe(false)
  })

  it('TALA-33: a turn landing behind a queued user row still fires', () => {
    // The user queues a message while the reply streams: last row is the
    // user's, the turn still in flight — armed.
    let s = tick(null, true, false)
    expect(s.arm).toBe(CONV)
    // The reply lands COMPLETE BEHIND the queued row: the last row is still
    // the user's (not landed), but the poller's reload flips the thread
    // not-in-flight... the queued message's own chained turn is now owed, so
    // `resuming` re-arms rather than lands. The landing registers when the
    // chained turn completes: last row is again a complete assistant reply.
    s = tick(s.arm, true, false)
    expect(s.fire).toBe(false)
    expect(s.arm).toBe(CONV)
    // The chained follow-up completes; the last row is finally a complete
    // assistant reply — the landing fires for the thread that armed it.
    s = tick(s.arm, false, true)
    expect(s.fire).toBe(true)
    expect(s.arm).toBeNull()
  })

  it('a landing observed long after the stream started still fires', () => {
    // Long planning reply: many ticks of in-flight streaming, then the
    // landing arrives far later. The arm holds across every one of them.
    let s = tick(null, true, false)
    for (let i = 0; i < 300; i++) {
      s = tick(s.arm, true, false)
      expect(s.fire).toBe(false)
    }
    expect(s.arm).toBe(CONV)
    s = tick(s.arm, false, true)
    expect(s.fire).toBe(true)
  })

  it('a thread switch clears the arm without firing', () => {
    let s = tick(null, true, false)
    expect(s.arm).toBe(CONV)
    // Switch to another plan while the turn is in flight: no fire, arm gone.
    s = tick(s.arm, true, false, OTHER)
    expect(s.fire).toBe(false)
    expect(s.arm).toBeNull()
    // The new thread's rows landing do not fire the old thread's callback.
    s = tick(s.arm, false, true, OTHER)
    expect(s.fire).toBe(false)
  })

  it('loading an old conversation never arms and never fires', () => {
    // Nothing in flight, the last row already complete: the initial load
    // evaluation is a no-op, not a landing.
    const s = tick(null, false, true)
    expect(s.fire).toBe(false)
    expect(s.arm).toBeNull()
  })

  it('a new-chat reset (convId null) clears the arm without firing', () => {
    let s = tick(null, true, false)
    expect(s.arm).toBe(CONV)
    s = tick(s.arm, true, false, null)
    expect(s.fire).toBe(false)
    expect(s.arm).toBeNull()
  })
})
