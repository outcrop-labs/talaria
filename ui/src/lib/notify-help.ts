// Where a blocked desktop alert is unblocked — per browser engine AND per OS.
//
// A person whose alerts are blocked has two switches to find, and most only
// know of one: the browser's own permission for this site, and the operating
// system's permission for the browser itself (plus a Focus / Do Not Disturb
// mode that silently swallows banners). Saying "allow it in your browser's
// site settings" — the old copy — sent people looking for the first and
// never told them about the second.
//
// Pure: the user-agent string goes in, the sentences come out, so every
// engine's steps are pinned by a test instead of discovered in the field.

export type Engine = 'chromium' | 'firefox' | 'safari' | 'other'
export type Os = 'mac' | 'windows' | 'linux' | 'other'

export interface BrowserInfo {
  engine: Engine
  /** The product name a person recognizes: Chrome, Edge, Brave, Firefox, Safari. */
  name: string
  os: Os
}

/** Read the engine, the product and the OS from a user-agent string. `brave`
 *  is `'brave' in navigator` — Brave sends Chrome's UA on purpose. */
export function detectBrowser(ua: string, opts: { brave?: boolean } = {}): BrowserInfo {
  const os: Os = /Macintosh|Mac OS X/.test(ua)
    ? 'mac'
    : /Windows/.test(ua)
      ? 'windows'
      : /Linux|X11|CrOS/.test(ua)
        ? 'linux'
        : 'other'
  if (/Firefox\//.test(ua)) return { engine: 'firefox', name: 'Firefox', os }
  if (/Edg\//.test(ua)) return { engine: 'chromium', name: 'Edge', os }
  if (/OPR\//.test(ua)) return { engine: 'chromium', name: 'Opera', os }
  if (/Chrome\/|Chromium\/|HeadlessChrome\//.test(ua)) {
    return { engine: 'chromium', name: opts.brave ? 'Brave' : 'Chrome', os }
  }
  if (/Safari\//.test(ua) && /Version\//.test(ua)) return { engine: 'safari', name: 'Safari', os }
  return { engine: 'other', name: 'your browser', os }
}

export interface BlockedSteps {
  /** The browser's own switch for this site. */
  browser: string[]
  /** The operating system's switch for the browser, and what hides banners. */
  os: string[]
  /** The line that says nothing else is needed. */
  closing: string
}

const SETTINGS_URL: Record<string, string> = {
  Chrome: 'chrome://settings/content/notifications',
  Edge: 'edge://settings/content/notifications',
  Brave: 'brave://settings/content/notifications',
  Opera: 'opera://settings/content/notifications',
}

/** The steps that undo a block, for this browser on this OS. */
export function blockedSteps(info: BrowserInfo): BlockedSteps {
  const browser: string[] = []
  switch (info.engine) {
    case 'chromium':
      browser.push(
        'Click the site-information icon at the left of the address bar, then set Notifications to Allow.',
        `Or open ${SETTINGS_URL[info.name] ?? SETTINGS_URL.Chrome} and move this site from “Not allowed” to “Allowed”.`,
      )
      break
    case 'firefox':
      browser.push(
        'Click the site-information icon in the address bar, then Connection secure → More information → Permissions, and allow Send notifications.',
        'Or open about:preferences#privacy → Permissions → Notifications → Settings, and set this site to Allow.',
      )
      break
    case 'safari':
      browser.push('Open Safari → Settings → Websites → Notifications, and set this site to Allow.')
      break
    default:
      browser.push("Open your browser's settings for this site and allow notifications.")
  }
  const os: string[] = []
  switch (info.os) {
    case 'mac':
      os.push(
        `On the Mac: System Settings → Notifications → ${info.name} → turn on Allow notifications.`,
        'A Focus mode (Do Not Disturb) hides banners while it is on.',
      )
      break
    case 'windows':
      os.push(
        `On Windows: Settings → System → Notifications → turn ${info.name} on.`,
        'Do not disturb hides banners while it is on.',
      )
      break
    case 'linux':
      os.push(
        `On Linux (GNOME): Settings → Notifications → turn ${info.name} on.`,
        'Do Not Disturb hides banners while it is on.',
      )
      break
    default:
      os.push(`Check that your system allows notifications from ${info.name}, and that Do Not Disturb is off.`)
  }
  return { browser, os, closing: 'This page notices the change; no reload needed.' }
}

/** Why a plain-http dev stack cannot alert, naming the origin it is on.
 *  Browsers give notifications, push and service workers to secure contexts
 *  only; every real Talaria instance is reached over HTTPS, so this is a
 *  development-stack sentence, not a fix to go looking for. */
export function insecureExplanation(origin: string): string {
  return (
    `Desktop alerts need HTTPS. This development stack is served over plain http (${origin}), ` +
    'so the browser blocks them here. Talaria instances reached over HTTPS are not affected.'
  )
}
