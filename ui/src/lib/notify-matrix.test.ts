// The matrix is the registry: these fixtures are the shape GET
// /api/notifications serves (`classes[]`, trimmed to what the matrix reads).
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { decide, type DecideInput } from './notify-decide'
import {
  atDefault,
  effectiveChannels,
  emailColumnNote,
  legacyRoute,
  mailNote,
  MATRIX_COLUMNS,
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

// The registry as classes.rs ships it (defaults and Needs you's 15-minute
// mail), the labels included: the fine print below reads both.
const real = (id: string, label: string, category: Category, d: [boolean, boolean, boolean], emailDelayMin = 0): ClassInfo => ({
  ...cls(id, category, d),
  label,
  emailDelayMin,
})
const REGISTRY: ClassInfo[] = [
  real('approval_pending', 'Approvals waiting on you', 'needs_you', [true, true, true], 15),
  real('judge_escalation', 'Judge escalations', 'needs_you', [true, true, true], 15),
  real('agent_blocked', 'Blocked agents', 'needs_you', [true, true, true], 15),
  real('workchain_turn', 'Your turn in a workchain', 'needs_you', [true, true, true], 15),
  real('mention', 'Mentions & shares', 'direct', [true, true, true]),
  real('dm', 'Direct messages', 'direct', [true, true, true]),
  real('participating', "Threads you're in", 'direct', [true, true, false]),
  real('account', 'Your account', 'direct', [true, true, true]),
  real('work_complete', 'Work finished', 'following', [false, false, false]),
  real('workchain_paused', 'Workchains paused', 'following', [false, false, false]),
  real('channel_activity', 'Channel activity', 'following', [true, true, true]),
]

describe('the missed-activity fine print (notify-web-4)', () => {
  it('says what the registry mails: Channel activity is Following, and mailed by default', () => {
    expect(mailNote(REGISTRY, undefined)).toBe('Needs you waits 15 minutes whatever you pick. Under Following, only Channel activity is mailed.')
  })

  it('follows the person’s own Email cells, not the defaults', () => {
    const on = { work_complete: { inApp: true as const, desktop: false, push: false, email: true } }
    expect(mailNote(REGISTRY, on)).toMatch(/Under Following, only Work finished and Channel activity are mailed\.$/)
    const off = { channel_activity: { inApp: true as const, desktop: true, push: true, email: false } }
    expect(mailNote(REGISTRY, off)).toMatch(/Nothing under Following is mailed\.$/)
    const all = Object.fromEntries(
      ['work_complete', 'workchain_paused', 'channel_activity'].map((id) => [id, { inApp: true as const, desktop: false, push: false, email: true }]),
    )
    expect(mailNote(REGISTRY, all)).toMatch(/Everything under Following is mailed too\.$/)
  })

  it('names a delay only where the registry has one, and no Following where none is served', () => {
    expect(mailNote(REGISTRY.filter((c) => c.category === 'direct'), undefined)).toBe('')
    expect(mailNote([real('approval_pending', 'A', 'needs_you', [true, true, true], 1)], undefined)).toBe('Needs you waits 1 minute whatever you pick.')
  })

  it('on Never nothing is mailed — email_due returns no time for off before any class delay', () => {
    const never = mailNote(REGISTRY, undefined, 'off')
    expect(never).toBe('Nothing is mailed while this is Never, whatever the Email column says.')
    expect(never).not.toMatch(/waits|whatever you pick|Following/)
    // Any other pick still says the delay and the Following clause.
    for (const f of ['15m', '1h', '1d'] as const) expect(mailNote(REGISTRY, undefined, f)).toBe(mailNote(REGISTRY, undefined))
  })

  it('a class waits its delay only where the person has its Email cell on', () => {
    const delayed = real('approval_pending', 'A', 'needs_you', [true, true, true], 15)
    expect(mailNote([delayed], undefined, '1h')).toBe('Needs you waits 15 minutes whatever you pick.')
    const off = { approval_pending: { inApp: true as const, desktop: true, push: true, email: false } }
    expect(mailNote([delayed], off, '1h')).toBe('')
    // A class whose Email is off by default says nothing until the person turns it on.
    const quiet = real('approval_pending', 'A', 'needs_you', [true, true, false], 15)
    expect(mailNote([quiet], undefined)).toBe('')
    const on = { approval_pending: { inApp: true as const, desktop: true, push: true, email: true } }
    expect(mailNote([quiet], on)).toBe('Needs you waits 15 minutes whatever you pick.')
  })

  it('Settings reads its fine print from mailNote, not a sentence of its own', () => {
    const here = dirname(fileURLToPath(import.meta.url))
    const section = readFileSync(join(here, '../routes/app/settings/NotificationsSection.svelte'), 'utf8')
    expect(section).toMatch(/mailNote\(classes, data\?\.channels, frequency\)/)
    expect(section).not.toMatch(/never mailed/i)
  })
})

describe('the Desktop column says what Desktop does (notify-web-3)', () => {
  // Desktop is the row's `alert` (I-15): on → `banner`, off → `feed`. The
  // decider is the proof of what that means in a tab the person is looking at.
  const looking: DecideInput = {
    read: false,
    self: { visible: true, focused: true, lastInputAt: 1_000_000 - 5_000 },
    others: [],
    isLeader: true,
    now: 1_000_000,
  }
  const desktop = MATRIX_COLUMNS.find((c) => c.id === 'desktop')!

  it('Desktop on is the pop-up where you look and the banner where you do not; off is neither', () => {
    expect(decide({ ...looking, alert: 'banner' })).toBe('toast')
    expect(decide({ ...looking, alert: 'banner', self: { visible: false, focused: false } })).toBe('banner')
    expect(decide({ ...looking, alert: 'feed' })).toBe('none')
    expect(decide({ ...looking, alert: 'feed', self: { visible: false, focused: false } })).toBe('none')
    // …so the column names both, and never says it only acts while you are away.
    expect(desktop.title).toMatch(/pop-up in Talaria while you are looking/)
    expect(desktop.title).toMatch(/banner on this computer/)
    expect(desktop.title).toMatch(/Off, neither shows/)
    expect(desktop.title).not.toMatch(/while you are not looking at Talaria\.$/)
  })

  it('In-app does not claim the bell counts every notification: its badge is For you', () => {
    const inApp = MATRIX_COLUMNS.find((c) => c.id === 'inApp')!
    expect(inApp.title).not.toMatch(/counted on the bell/)
    expect(inApp.title).toMatch(/badge counts Needs you and Direct/)
  })
})
