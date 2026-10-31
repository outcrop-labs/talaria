import { describe, expect, it } from 'vitest'
import { blockedSteps, detectBrowser, insecureExplanation } from './notify-help'

const UA = {
  chromeLinux:
    'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36',
  headlessChrome:
    'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/146.0.0.0 Safari/537.36',
  edgeWin:
    'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36 Edg/146.0.0.0',
  firefoxMac: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 15.6; rv:144.0) Gecko/20100101 Firefox/144.0',
  safariMac:
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 15_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15',
  webkitGtk:
    'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15',
}

describe('detectBrowser', () => {
  it('names the product a person recognizes, and the OS', () => {
    expect(detectBrowser(UA.chromeLinux)).toEqual({ engine: 'chromium', name: 'Chrome', os: 'linux' })
    expect(detectBrowser(UA.headlessChrome)).toEqual({ engine: 'chromium', name: 'Chrome', os: 'linux' })
    expect(detectBrowser(UA.edgeWin)).toEqual({ engine: 'chromium', name: 'Edge', os: 'windows' })
    expect(detectBrowser(UA.firefoxMac)).toEqual({ engine: 'firefox', name: 'Firefox', os: 'mac' })
    expect(detectBrowser(UA.safariMac)).toEqual({ engine: 'safari', name: 'Safari', os: 'mac' })
    expect(detectBrowser(UA.webkitGtk)).toEqual({ engine: 'safari', name: 'Safari', os: 'linux' })
  })

  it('takes Brave from its own flag — its UA is Chrome on purpose', () => {
    expect(detectBrowser(UA.chromeLinux, { brave: true }).name).toBe('Brave')
  })

  it('falls back to a generic name instead of guessing', () => {
    expect(detectBrowser('curl/8.0')).toEqual({ engine: 'other', name: 'your browser', os: 'other' })
  })
})

describe('blockedSteps', () => {
  it('gives each engine its own switch and its settings page', () => {
    const chrome = blockedSteps(detectBrowser(UA.chromeLinux))
    expect(chrome.browser.join(' ')).toContain('site-information icon')
    expect(chrome.browser.join(' ')).toContain('chrome://settings/content/notifications')
    const edge = blockedSteps(detectBrowser(UA.edgeWin))
    expect(edge.browser.join(' ')).toContain('edge://settings/content/notifications')
    const firefox = blockedSteps(detectBrowser(UA.firefoxMac))
    expect(firefox.browser.join(' ')).toContain('Send notifications')
    expect(firefox.browser.join(' ')).toContain('about:preferences#privacy')
    const safari = blockedSteps(detectBrowser(UA.safariMac))
    expect(safari.browser.join(' ')).toContain('Safari → Settings → Websites → Notifications')
  })

  it('always names the OS switch for the browser and what hides banners', () => {
    const mac = blockedSteps(detectBrowser(UA.firefoxMac))
    expect(mac.os[0]).toBe('On the Mac: System Settings → Notifications → Firefox → turn on Allow notifications.')
    expect(mac.os[1]).toContain('Focus')
    const win = blockedSteps(detectBrowser(UA.edgeWin))
    expect(win.os[0]).toBe('On Windows: Settings → System → Notifications → turn Edge on.')
    expect(win.os[1]).toContain('Do not disturb')
    const linux = blockedSteps(detectBrowser(UA.chromeLinux))
    expect(linux.os[0]).toBe('On Linux (GNOME): Settings → Notifications → turn Chrome on.')
    expect(linux.os[1]).toContain('Do Not Disturb')
  })

  it('closes by saying no reload is needed', () => {
    expect(blockedSteps(detectBrowser(UA.chromeLinux)).closing).toBe('This page notices the change; no reload needed.')
  })
})

describe('insecureExplanation', () => {
  it('names the origin and says real instances are not affected', () => {
    const s = insecureExplanation('http://notify-lan.test:5304')
    expect(s).toContain('Desktop alerts need HTTPS')
    expect(s).toContain('(http://notify-lan.test:5304)')
    expect(s).toContain('Talaria instances reached over HTTPS are not affected.')
  })
})
