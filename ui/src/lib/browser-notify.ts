// OS-level notifications from a browser tab. Two rules shape this file:
//
//  1. PERMISSION IS ASKED FROM A GESTURE, never on load. A prompt that
//     ambushes on arrival gets reflex-denied, and denied is near-permanent.
//  2. AN OS NOTIFICATION IS FOR WHEN TALARIA IS NOT WHAT THE PERSON IS
//     LOOKING AT. In focus, the in-app toast already said it; a second copy
//     in the corner of the OS would be noise. shouldBrowserNotify() is the
//     one gate every caller goes through.
//
// This covers the open-but-background case, which the plain Notification API
// does from a tab. A closed browser needs Web Push (service worker, VAPID
// keys, a server-side sender) — a different and server-carrying commitment,
// deliberately not made here.
const PREF_KEY = 'talaria.browserNotify'

export type PermissionState = 'unsupported' | 'default' | 'granted' | 'denied'

export function permissionState(): PermissionState {
  if (typeof Notification === 'undefined') return 'unsupported'
  return Notification.permission
}

/** The person's own off switch. Revoking browser permission means digging
 *  through browser site settings; a pref makes "stop showing these" one
 *  click while the grant stands. Default on: the grant IS the opt-in. */
function browserNotifyPrefOn(): boolean {
  try {
    return localStorage.getItem(PREF_KEY) !== 'off'
  } catch {
    return true // private mode and friends — the grant still decides
  }
}

export function setBrowserNotifyPref(on: boolean): void {
  try {
    localStorage.setItem(PREF_KEY, on ? 'on' : 'off')
  } catch {
    // No storage available: the permission alone carries the setting.
  }
}

/** The effective answer: nothing fires unless the browser granted it AND the
 *  person hasn't switched it off here. */
export function browserNotifyEnabled(): boolean {
  return permissionState() === 'granted' && browserNotifyPrefOn()
}

/** Ask. Must run inside a click handler (rule 1 above). */
export async function requestBrowserNotify(): Promise<PermissionState> {
  if (typeof Notification === 'undefined') return 'unsupported'
  try {
    if (Notification.permission === 'default') await Notification.requestPermission()
  } catch {
    // Some browsers reject instead of resolving; the state read below is
    // still the truth.
  }
  return permissionState()
}

/** The one gate. Pure, so it is testable without a browser. */
export function shouldBrowserNotify(input: {
  focused: boolean
  visible: boolean
  permission: PermissionState
  enabled: boolean
}): boolean {
  if (!input.enabled || input.permission !== 'granted') return false
  // Looking at Talaria means the in-app toast is enough. Anything else
  // (another tab, another window, minimized) is exactly the case the OS
  // notification exists for. Focus alone can lie — a focused window on
  // another virtual desktop — so both must agree Talaria is front.
  return !(input.focused && input.visible)
}

/** Fire one. Fails soft on purpose: the in-app toast already landed, so a
 *  browser that refuses (or forbids the constructor from a page) is a
 *  degraded corner, not an error worth surfacing mid-poll. */
export function fireBrowserNotification(n: { title: string; body?: string; tag?: string; href?: string }): void {
  if (typeof Notification === 'undefined') return
  try {
    const note = new Notification(n.title, { body: n.body || undefined, tag: n.tag || undefined })
    note.onclick = () => {
      window.focus()
      note.close()
      if (n.href) window.location.assign(n.href)
    }
  } catch {
    // Swallowed — see above.
  }
}
