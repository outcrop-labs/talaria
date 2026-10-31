import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  NOTIFY_BADGE,
  NOTIFY_ICON,
  bannerOptions,
  browserNotifyEnabled,
  effectiveNotifyPermission,
  ensurePushSubscription,
  permissionState,
  requestBrowserNotify,
  sameServerKey,
  setBrowserNotifyPref,
  shouldBrowserNotify,
  urlB64ToUint8Array,
} from './browser-notify'

// localStorage isn't guaranteed under the node test environment; the modules
// read it lazily inside functions (through `window.localStorage`, see
// lib/persist.ts), so a stub before each call is enough.
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

describe('effectiveNotifyPermission (shell vs webview)', () => {
  it('does not inherit a denied webview read inside the shell', () => {
    expect(effectiveNotifyPermission({ inShell: true, browser: 'denied' })).toBe('granted')
    expect(effectiveNotifyPermission({ inShell: true, browser: 'unsupported' })).toBe('granted')
  })

  it('keeps the browser read outside the shell', () => {
    expect(effectiveNotifyPermission({ inShell: false, browser: 'denied' })).toBe('denied')
    expect(effectiveNotifyPermission({ inShell: false, browser: 'default' })).toBe('default')
  })
})

describe('browserNotifyEnabled (grant AND pref)', () => {
  beforeEach(() => {
    backing.clear()
    vi.stubGlobal('window', { localStorage: localStorageStub })
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

describe('urlB64ToUint8Array (the VAPID key decode)', () => {
  it('decodes unpadded url-safe base64 to the raw bytes', () => {
    // /api/push/key hands the key unpadded url-safe — the two jobs are
    // re-padding and translating the alphabet; both are pinned here.
    expect(Array.from(urlB64ToUint8Array('AAEC'))).toEqual([0, 1, 2])
    // '-' and '_' are legal in the wire form but not in atob's alphabet.
    expect(Array.from(urlB64ToUint8Array('-_8'))).toEqual([251, 255])
  })
})

describe('ensurePushSubscription (outside a browser)', () => {
  it('answers unsupported instead of throwing where the APIs are absent', async () => {
    expect(await ensurePushSubscription()).toBe('unsupported')
  })
})

describe('permissionState — the insecure origin says so', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('reads insecure BEFORE the browser\'s own "denied" on a plain-http origin', () => {
    vi.stubGlobal('window', { isSecureContext: false, localStorage: localStorageStub })
    vi.stubGlobal('Notification', { permission: 'denied' })
    expect(permissionState()).toBe('insecure')
    expect(browserNotifyEnabled()).toBe(false)
  })

  it('reads the browser on a secure origin', () => {
    vi.stubGlobal('window', { isSecureContext: true, localStorage: localStorageStub })
    vi.stubGlobal('Notification', { permission: 'default' })
    expect(permissionState()).toBe('default')
  })

  it('never prompts from an insecure origin', async () => {
    const requestPermission = vi.fn()
    vi.stubGlobal('window', { isSecureContext: false })
    vi.stubGlobal('Notification', { permission: 'denied', requestPermission })
    expect(await requestBrowserNotify()).toBe('insecure')
    expect(requestPermission).not.toHaveBeenCalled()
  })

  it('prompts from the default state, synchronously inside the call', async () => {
    let state: NotificationPermission = 'default'
    const requestPermission = vi.fn(() => {
      state = 'granted'
      return Promise.resolve(state)
    })
    vi.stubGlobal('window', { isSecureContext: true })
    vi.stubGlobal('Notification', {
      get permission() {
        return state
      },
      requestPermission,
    })
    const p = requestBrowserNotify()
    // Called before the first await yields: the click's activation holds.
    expect(requestPermission).toHaveBeenCalledTimes(1)
    expect(await p).toBe('granted')
  })
})

describe('sameServerKey (the key-mismatch heal)', () => {
  const want = new Uint8Array([4, 1, 2, 3])
  it('compares the bound key byte for byte', () => {
    expect(sameServerKey(new Uint8Array([4, 1, 2, 3]).buffer, want)).toBe(true)
    expect(sameServerKey(new Uint8Array([4, 1, 2, 9]).buffer, want)).toBe(false)
    expect(sameServerKey(new Uint8Array([4, 1, 2]).buffer, want)).toBe(false)
  })
  it('treats an engine that hides the key as unknown — kept, never a resubscribe loop', () => {
    expect(sameServerKey(null, want)).toBeNull()
    expect(sameServerKey(undefined, want)).toBeNull()
  })
})

describe('bannerOptions (what every OS banner carries)', () => {
  it('brands it, tags it by row, dates it, and carries the click data', () => {
    const o = bannerOptions({ id: 'n1', title: 'T', body: 'B', href: '/x', createdAt: '2026-10-31T12:00:00.000Z' }) as NotificationOptions & {
      renotify: boolean
      timestamp: number
    }
    expect(o.tag).toBe('n1')
    expect(o.icon).toBe(NOTIFY_ICON)
    expect(o.badge).toBe(NOTIFY_BADGE)
    expect(o.data).toEqual({ id: 'n1', href: '/x', v: '2026-10-31T12:00:00.000Z' })
    expect(o.timestamp).toBe(Date.parse('2026-10-31T12:00:00.000Z'))
    expect(o.renotify).toBe(false)
  })
  it('re-alerts only when asked, under the same tag', () => {
    const o = bannerOptions({ id: 'dm1', title: 'T', renotify: true }) as NotificationOptions & { renotify: boolean }
    expect(o.renotify).toBe(true)
    expect(o.tag).toBe('dm1')
  })
})
