// The fallback is the whole point of this helper: six views share it, and six
// hand-rolled versions would give six answers to "what does /models/nonsense
// do". These pin the one answer — the home tab, always something real.
import { describe, expect, it } from 'vitest'
import { tabFromPath } from './route-tabs'

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
