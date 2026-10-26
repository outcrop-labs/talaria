import { describe, expect, it } from 'vitest'
import { appInScope, withApp } from './app-scope'

// WHICH APP IS RUNNING is the host's answer, set where it hands control to app
// code and carried through everything that code starts — never a name the app
// passed in.
describe('withApp', () => {
  it('names the app for everything the code starts, and nothing outside it', async () => {
    expect(appInScope()).toBeNull()
    const seen = await withApp('crm', async () => {
      const later = new Promise<string | null>((resolve) => setTimeout(() => resolve(appInScope()), 1))
      await Promise.resolve()
      return [appInScope(), await later]
    })
    expect(seen).toEqual(['crm', 'crm'])
    expect(appInScope()).toBeNull()
  })

  it('lets an inner app scope win, and restores the outer one after it', async () => {
    await withApp('crm', async () => {
      expect(await withApp('billing', async () => appInScope())).toBe('billing')
      expect(appInScope()).toBe('crm')
    })
  })

  it('names no app for a label that is not a slug', () => {
    expect(withApp('_host', () => appInScope())).toBeNull()
    expect(withApp('Not A Slug', () => appInScope())).toBeNull()
  })
})
