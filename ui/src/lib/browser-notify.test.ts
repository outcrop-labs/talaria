import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { browserNotifyEnabled, setBrowserNotifyPref, shouldBrowserNotify } from './browser-notify'

// localStorage isn't guaranteed under the node test environment; the module
// reads it lazily inside functions, so a stub before each call is enough.
const backing = new Map<string, string>()
const localStorageStub = {
  getItem: (k: string) => backing.get(k) ?? null,
  setItem: (k: string, v: string) => void backing.set(k, v),
  removeItem: (k: string) => void backing.delete(k),
}

describe('shouldBrowserNotify (the one gate)', () => {
  const base = { permission: 'granted' as const, enabled: true }

  it('stays quiet while the person is looking at Talaria', () => {
    expect(shouldBrowserNotify({ ...base, focused: true, visible: true })).toBe(false)
  })

  it('fires when Talaria is in a background tab or another window', () => {
    expect(shouldBrowserNotify({ ...base, focused: false, visible: true })).toBe(true)
    expect(shouldBrowserNotify({ ...base, focused: true, visible: false })).toBe(true)
    expect(shouldBrowserNotify({ ...base, focused: false, visible: false })).toBe(true)
  })

  it('never fires without the grant or with the pref off', () => {
    expect(shouldBrowserNotify({ focused: false, visible: false, permission: 'default', enabled: true })).toBe(false)
    expect(shouldBrowserNotify({ focused: false, visible: false, permission: 'granted', enabled: false })).toBe(false)
  })
})

describe('browserNotifyEnabled (grant AND pref)', () => {
  beforeEach(() => {
    backing.clear()
    vi.stubGlobal('localStorage', localStorageStub)
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('is on when granted and no pref was ever written', () => {
    vi.stubGlobal('Notification', { permission: 'granted' })
    expect(browserNotifyEnabled()).toBe(true)
  })

  it('honors the local off switch over a standing grant', () => {
    vi.stubGlobal('Notification', { permission: 'granted' })
    setBrowserNotifyPref(false)
    expect(browserNotifyEnabled()).toBe(false)
    setBrowserNotifyPref(true)
    expect(browserNotifyEnabled()).toBe(true)
  })

  it('is off without a grant regardless of pref', () => {
    vi.stubGlobal('Notification', { permission: 'denied' })
    setBrowserNotifyPref(true)
    expect(browserNotifyEnabled()).toBe(false)
  })
})
