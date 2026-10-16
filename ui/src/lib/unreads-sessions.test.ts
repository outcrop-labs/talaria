// THE MERGED WORK BADGE'S ARITHMETIC. One rail entry holds work sessions, plans
// and research threads, so the number on it is their total — and the total was
// an expression inside a query function, where the one thing that could go
// wrong with it (an arm that is not there) could not be reached from a test.
// It is `sumSessions` now, and this is the arithmetic pinned.
//
// Why it is worth pinning at all, for a `a + b + c`: this is the surface a
// person checks INSTEAD of opening anything. A badge that is wrong by an arm
// says "nothing is waiting" about work that is, and a badge that is `NaN` says
// nothing at all while looking like it tried.
import { describe, expect, it } from 'vitest'
import { sumSessions } from './unreads-sum'
import type { Unreads } from './unreads.svelte'

/** The answer as the route actually sends it — every arm, no `sessions` (the
 *  client derives that). Typed against the shipped `Unreads` so this fixture
 *  cannot drift from the wire without a type error, and so the annotation
 *  doubles as proof that the real answer satisfies `sumSessions`'s input. */
const wire = (arms: Partial<Omit<Unreads, 'sessions'>>): Omit<Unreads, 'sessions'> => ({
  comms: 0,
  work: 0,
  plan: 0,
  research: 0,
  notifications: 0,
  ...arms,
})

describe('sumSessions', () => {
  it('is the three session arms added up', () => {
    expect(sumSessions(wire({ work: 2, plan: 3, research: 4 }))).toBe(9)
  })

  it('counts each arm on its own', () => {
    // One assertion per arm, because a sum that dropped exactly one term is
    // the failure this badge is most likely to have and the hardest to see:
    // the number still looks like a number.
    expect(sumSessions(wire({ work: 5 }))).toBe(5)
    expect(sumSessions(wire({ plan: 5 }))).toBe(5)
    expect(sumSessions(wire({ research: 5 }))).toBe(5)
  })

  it('is 0 when nothing is waiting — an honest zero, which the rail hides', () => {
    expect(sumSessions(wire({}))).toBe(0)
  })

  it('ignores comms and notifications, which have surfaces of their own', () => {
    // Adding them would count one unread twice in one rail — the Comms entry
    // and the bell already carry those — and make the Work number one nobody
    // could act on.
    expect(sumSessions(wire({ comms: 7, notifications: 11 }))).toBe(0)
    expect(sumSessions(wire({ comms: 7, notifications: 11, work: 1 }))).toBe(1)
  })

  it('counts a MISSING arm as 0 rather than poisoning the whole badge', () => {
    // The route answers all three arms or 500s, so this is not about the
    // current server: it is about an api one deploy behind, or one that drops
    // an arm this UI still knows. `work + plan + research` with one term
    // `undefined` is NaN, and a NaN badge renders as a pill with nonsense in
    // it — strictly worse than the two counts that did arrive.
    expect(sumSessions({ work: 4, research: 1 })).toBe(5)
    expect(sumSessions({ plan: 2 })).toBe(2)
    expect(sumSessions({})).toBe(0)
  })

  it('counts a null or non-finite arm as 0, for the same reason', () => {
    // A JSON `null`, or a number that came back as NaN, is a gap in the
    // answer and not a count. The two arms beside it stay true.
    expect(sumSessions({ work: 3, plan: null, research: 2 })).toBe(5)
    expect(sumSessions({ work: 3, plan: Number.NaN, research: 2 })).toBe(5)
    expect(sumSessions({ work: Number.POSITIVE_INFINITY, plan: 1 })).toBe(1)
  })

  it('does not round or clamp a real count — the rail decides how to show it', () => {
    // `99+` is a rendering decision (and lives in the badge), not arithmetic.
    // Summing to a capped number here would make the title lie too.
    expect(sumSessions(wire({ work: 120, plan: 7, research: 1 }))).toBe(128)
  })

  it('names a field the unreads answer carries', () => {
    // The badge is read as `unreads.data?.[item.badge] ?? 0`, with
    // `item.badge === 'sessions'` on the rail's Work entry (pinned in
    // work-selection.test.ts). The annotation is the assertion: a type error
    // the day the field this sum fills stops existing under that name, which
    // would otherwise present as a badge that is 0 forever.
    const field: keyof Unreads = 'sessions'
    expect(field).toBe('sessions')
  })
})
