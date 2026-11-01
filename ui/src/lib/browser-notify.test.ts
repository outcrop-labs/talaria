import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  NOTIFY_BADGE,
  NOTIFY_ICON,
  bannerOptions,
  shellHref,
  showOsNotification,
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

const invoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))

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

// The shell dispatches `talaria:notification-open` only for a banner whose
// href passes its openable_href (I-1), and drops an empty one. The catch-up
// summary had href '' — so its click in the shell never reached the page and
// the bell never opened (found by the #634 + #635 integration run).
describe('shell banners — the summary carries a path the shell will hand back', () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue(undefined)
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
    vi.stubGlobal('location', { pathname: '/boards/b1', search: '?view=list' })
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('a summary with no href carries the page it was posted from', () => {
    expect(shellHref({ href: '', summary: true })).toBe('/boards/b1?view=list')
    expect(shellHref({ summary: true })).toBe('/boards/b1?view=list')
  })

  it('a row keeps its own href, and a link-less row stays link-less', () => {
    expect(shellHref({ href: '/comms/channel/c1', summary: false })).toBe('/comms/channel/c1')
    expect(shellHref({ href: '' })).toBe('')
    expect(shellHref({})).toBeUndefined()
  })

  it('posts the summary to the shell with that path, under its own tag', async () => {
    const ok = await showOsNotification(
      { id: '', tag: 'talaria-summary', title: '4 new — x…', href: '', kind: 'summary', renotify: true, summary: true },
      () => {},
    )
    expect(ok).toBe(true)
    // kind and renotify are forwarded by a shell bridge that knows them
    // (desktop-shell.ts, #634); tag and href are this file's to choose.
    expect(invoke).toHaveBeenCalledWith(
      'desktop_notify',
      expect.objectContaining({ title: '4 new — x…', tag: 'talaria-summary', href: '/boards/b1?view=list' }),
    )
  })

  it('posts a row with its own href, untouched', async () => {
    await showOsNotification({ id: 'r1', title: 'DM', href: '/comms/channel/c1', kind: 'dm' }, () => {})
    expect(invoke).toHaveBeenCalledWith('desktop_notify', expect.objectContaining({ tag: 'r1', href: '/comms/channel/c1' }))
  })
})
