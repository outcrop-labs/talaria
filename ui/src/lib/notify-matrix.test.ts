// The matrix is the registry: these fixtures are the shape GET
// /api/notifications serves (`classes[]`, trimmed to what the matrix reads).
import { describe, expect, it } from 'vitest'
import {
  atDefault,
  effectiveChannels,
  emailColumnNote,
  legacyRoute,
  matrixGroups,
  mergePatch,
  resetToDefault,
  setChannel,
} from './notify-matrix'
import type { Category, ClassInfo } from './notify-wire'

const cls = (id: string, category: Category, d: [boolean, boolean, boolean], adminOnly = false): ClassInfo => ({
  id,
  label: id,
  blurb: '',
  category,
  defaults: { desktop: d[0], push: d[1], email: d[2] },
  breaksThrough: category === 'needs_you',
  adminOnly,
  emailDelayMin: 0,
  kinds: [{ id, reason: id }],
})

// The registry's eleven, in its order (classes.rs CLASSES).
const ADMIN: ClassInfo[] = [
  cls('approval_pending', 'needs_you', [true, true, true]),
  cls('judge_escalation', 'needs_you', [true, true, true]),
  cls('agent_blocked', 'needs_you', [true, true, true], true),
  cls('workchain_turn', 'needs_you', [true, true, true]),
  cls('mention', 'direct', [true, true, true]),
  cls('dm', 'direct', [true, true, true]),
  cls('participating', 'direct', [true, true, false]),
  cls('work_complete', 'following', [true, true, false]),
  cls('workchain_paused', 'following', [true, true, false]),
  cls('channel_activity', 'following', [true, false, false]),
  cls('gap_reported', 'workspace', [true, true, false], true),
]
const MEMBER = ADMIN.filter((c) => !c.adminOnly)

describe('matrixGroups', () => {
  it('has exactly the served classes, grouped by category in order', () => {
    const g = matrixGroups(ADMIN, undefined)
    expect(g.map((x) => x.category)).toEqual(['needs_you', 'direct', 'following', 'workspace'])
    expect(g.flatMap((x) => x.rows.map((r) => r.cls.id))).toEqual(ADMIN.map((c) => c.id))
    expect(g.flatMap((x) => x.rows).length).toBe(11)
  })

  it('a member is served nine and sees nine, with no Workspace group', () => {
    const g = matrixGroups(MEMBER, undefined)
    expect(g.flatMap((x) => x.rows).length).toBe(9)
    expect(g.map((x) => x.category)).toEqual(['needs_you', 'direct', 'following'])
  })

  it('reads the server’s channels, falling back to the defaults, and overlays a pending click', () => {
    const served = { dm: { inApp: true as const, desktop: true, push: false, email: true } }
    const g = matrixGroups(ADMIN, served, { mention: { email: false } })
    const rows = Object.fromEntries(g.flatMap((x) => x.rows).map((r) => [r.cls.id, r]))
    expect(rows.dm?.channels).toEqual({ inApp: true, desktop: true, push: false, email: true })
    expect(rows.dm?.atDefault).toBe(false)
    expect(rows.mention?.channels.email).toBe(false)
    expect(rows.participating?.channels).toEqual({ inApp: true, desktop: true, push: true, email: false })
    expect(rows.participating?.atDefault).toBe(true)
  })

  it('puts a category it does not know last, rather than dropping its rows', () => {
    const g = matrixGroups([...MEMBER, cls('future', 'brand_new' as Category, [true, true, true])], undefined)
    expect(g.at(-1)?.category).toBe('brand_new')
    expect(g.at(-1)?.label).toBe('brand_new')
  })
})

describe('saves', () => {
  it('one cell, a merge, a reset', () => {
    expect(setChannel('dm', 'push', false)).toEqual({ dm: { push: false } })
    expect(mergePatch({ dm: { push: false } }, { dm: { email: false }, mention: { desktop: false } })).toEqual({
      dm: { push: false, email: false },
      mention: { desktop: false },
    })
    expect(resetToDefault(ADMIN[6]!)).toEqual({ participating: { desktop: true, push: true, email: false } })
  })

  it('in-app is always on in the effective answer', () => {
    expect(effectiveChannels(ADMIN[0]!, undefined).inApp).toBe(true)
    expect(atDefault(ADMIN[0]!, { inApp: true, desktop: true, push: true, email: true })).toBe(true)
  })
})

describe('legacyRoute (I-13)', () => {
  it('derives the route an older app reads', () => {
    expect(legacyRoute({ desktop: true, push: false, email: true })).toBe('both')
    expect(legacyRoute({ desktop: false, push: true, email: true })).toBe('both')
    expect(legacyRoute({ desktop: false, push: false, email: true })).toBe('email')
    expect(legacyRoute({ desktop: true, push: true, email: false })).toBe('in_app')
  })
})

describe('the email column', () => {
  it('explains itself when the instance switch is off', () => {
    expect(emailColumnNote(true)).toBeNull()
    expect(emailColumnNote(false)).toMatch(/off for this instance/)
  })
})
