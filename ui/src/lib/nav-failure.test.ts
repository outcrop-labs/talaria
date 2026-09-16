// The words are the contract: whatever the engine threw, the banner says
// something a person can act on — and the chunk case names reloading, because
// a failed dynamic import stays failed in this document (the browser caches
// the rejected import in its module map), so "try again" alone can be a lie.
import { describe, expect, it } from 'vitest'
import { describeNavigationFailure } from './nav-failure'

describe('describeNavigationFailure', () => {
  it('names the deploy/connection story for a failed chunk load, per engine', () => {
    const chrome = new Error(
      'Failed to fetch dynamically imported module: http://talaria/_app/immutable/nodes/12.js',
    )
    const firefox = new Error('error loading dynamically imported module http://talaria/_app/x.js')
    const webkit = new Error('Importing a module script failed.')
    for (const e of [chrome, firefox, webkit]) {
      const msg = describeNavigationFailure(e)
      expect(msg).toContain('failed to load')
      expect(msg).toContain('Reloading')
    }
  })

  it('names the network for plain request failures', () => {
    expect(describeNavigationFailure(new TypeError('Failed to fetch'))).toContain('network')
    expect(
      describeNavigationFailure(new Error('NetworkError when attempting to fetch resource.')),
    ).toContain('network')
  })

  it('stays honest on anything it does not recognize', () => {
    for (const e of [new Error('boom'), 'boom', null, undefined]) {
      expect(describeNavigationFailure(e)).toContain('failed to open')
    }
  })
})
