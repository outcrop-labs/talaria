// The fallback is the whole point of this helper: six views share it, and six
// hand-rolled versions would give six answers to "what does /models/nonsense
// do". These pin the one answer — the home tab, always something real.
import { describe, expect, it } from 'vitest'
import { pathId, tabFromPath, isUnder } from './route-tabs'

const TABS = ['models', 'pricing', 'fitness'] as const

describe('tabFromPath', () => {
  it('reads the segment when it names a tab', () => {
    expect(tabFromPath('/models/pricing', '/models', TABS, 'models')).toBe('pricing')
    expect(tabFromPath('/models/fitness', '/models', TABS, 'models')).toBe('fitness')
  })

  it('falls back on the bare view path', () => {
    expect(tabFromPath('/models', '/models', TABS, 'models')).toBe('models')
  })

  it('falls back on a trailing slash', () => {
    expect(tabFromPath('/models/', '/models', TABS, 'models')).toBe('models')
  })

  it('falls back on an unknown segment — a stale or typo’d link lands somewhere real', () => {
    expect(tabFromPath('/models/nonsense', '/models', TABS, 'models')).toBe('models')
  })

  it('falls back on a deeper path rather than matching its first segment', () => {
    // `/models/pricing/extra` means something else, or nothing. Rendering the
    // pricing tab for it would be a quiet wrong answer.
    expect(tabFromPath('/models/pricing/extra', '/models', TABS, 'models')).toBe('models')
  })

  it('does not match a different view that shares a prefix', () => {
    // `/modelsomething` must not read as `/models` + a tab.
    expect(tabFromPath('/modelsomething/pricing', '/models', TABS, 'models')).toBe('models')
  })

  it('does not treat the view itself as a tab of another', () => {
    expect(tabFromPath('/admin/security', '/models', TABS, 'models')).toBe('models')
  })

  it('decodes a percent-encoded segment', () => {
    // App-provided settings tabs are keyed by slug and route through the same
    // helper, so a slug that needed encoding must still match.
    expect(tabFromPath('/settings/my%2Dapp', '/settings', ['profile', 'my-app'] as const, 'profile')).toBe('my-app')
  })

  it('is exact about case, because the tab ids are', () => {
    expect(tabFromPath('/models/Pricing', '/models', TABS, 'models')).toBe('models')
  })

  it('handles a root-level base', () => {
    expect(tabFromPath('/activity', '', ['inbox', 'activity'] as const, 'inbox')).toBe('activity')
  })
})

describe('pathId', () => {
  it('reads the id a path selects', () => {
    expect(pathId('/research/run-42', '/research')).toBe('run-42')
  })

  it('is null on the bare view — nothing selected is a real state', () => {
    expect(pathId('/research', '/research')).toBeNull()
    expect(pathId('/research/', '/research')).toBeNull()
  })

  it('does not read a deeper path as a selection', () => {
    expect(pathId('/research/run-42/extra', '/research')).toBeNull()
  })

  it('does not match a view that shares a prefix', () => {
    expect(pathId('/researchers/run-42', '/research')).toBeNull()
  })

  it('decodes an encoded id', () => {
    expect(pathId('/plan/a%2Fb', '/plan')).toBe('a/b')
  })

  it('takes ids as opaque — existence is the loading query’s question', () => {
    // Deliberately NOT validated as a uuid: an id may name a row that was
    // deleted, and the query that loads it already has a not-found story.
    expect(pathId('/plan/not-a-uuid', '/plan')).toBe('not-a-uuid')
  })
})

// ── isUnder: the guard that keeps a leaving view from navigating you back ────

describe('isUnder', () => {
  it('matches the view itself and everything beneath it', () => {
    expect(isUnder('/comms', '/comms')).toBe(true)
    expect(isUnder('/comms/channel/c-1', '/comms')).toBe(true)
    expect(isUnder('/knowledge/space-1/doc-2', '/knowledge')).toBe(true)
  })

  it('rejects other views, which is the whole point', () => {
    // The bug this guard exists for: a view that is leaving still runs its
    // effects against the NEXT view's pathname, and if those effects navigate,
    // a wrong answer here drags the user back. It reads as a broken nav rail —
    // the same item wants two or three clicks before it moves.
    expect(isUnder('/agents', '/comms')).toBe(false)
    expect(isUnder('/', '/comms')).toBe(false)
  })

  it('rejects prefix look-alikes, which a bare startsWith would accept', () => {
    expect(isUnder('/commsomething', '/comms')).toBe(false)
    expect(isUnder('/knowledgebase', '/knowledge')).toBe(false)
  })
})
