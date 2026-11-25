import { describe, expect, it } from 'vitest'
import { SETTINGS_TABS, opensOn, settingsTabOf, tabsFor } from './board-settings'

describe('who sees which Board settings tab', () => {
  it('owners and editors see every tab, Notifications last', () => {
    for (const role of ['owner', 'editor']) {
      expect(tabsFor(role)).toEqual(SETTINGS_TABS)
    }
    expect(SETTINGS_TABS.at(-1)).toBe('notifications')
    expect(settingsTabOf('notifications')).toBe('notifications')
  })

  it('a viewer sees only their own notification setting', () => {
    expect(tabsFor('viewer')).toEqual(['notifications'])
    expect(opensOn('viewer', 'notifications')).toBe(true)
    // A link to a tab a viewer cannot see opens nothing, as an unknown tab does.
    for (const t of SETTINGS_TABS.filter((t) => t !== 'notifications')) {
      expect(opensOn('viewer', t)).toBe(false)
    }
  })

  it('no role, no tab, nothing opens', () => {
    expect(tabsFor(undefined)).toEqual([])
    expect(opensOn(undefined, 'notifications')).toBe(false)
    expect(opensOn('owner', undefined)).toBe(false)
    expect(opensOn('owner', 'sync')).toBe(true)
  })
})
