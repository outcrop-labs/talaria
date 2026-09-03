// OS-level notifications from a browser tab. Two rules shape this file:
//
//  1. PERMISSION IS ASKED FROM A GESTURE, never on load. A prompt that
//     ambushes on arrival gets reflex-denied, and denied is near-permanent.
//  2. AN OS NOTIFICATION IS FOR WHEN TALARIA IS NOT WHAT THE PERSON IS
//     LOOKING AT. In focus, the in-app toast already said it; a second copy
//     in the corner of the OS would be noise. shouldBrowserNotify() is the
//     one gate every caller goes through.
//
// The two cases split by where the browser is: this file's Notification API
// covers the open-but-background tab, and Web Push covers the CLOSED
// browser — /sw.js here, the VAPID/aes128gcm plane in the API's src/push.rs.
// Push rides rule 2 as surely as the tab plane does (the worker suppresses
// when a Talaria window is visible) and rule 1 doubly (subscription happens
// in the settings toggle's click, after the grant).
import { getJson, postJson } from '@/lib/fetch-json'

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

// ── Web Push (the closed browser) ────────────────────────────────────────────

/** applicationServerKey is raw bytes; /api/push/key hands the key
 *  base64url. Padded standard base64 comes out the far end unchanged —
 *  re-padding and translating the alphabet is the whole job. The buffer is
 *  a plain ArrayBuffer (not ArrayBufferLike) because pushManager.subscribe
 *  asks for BufferSource, and a SharedArrayBuffer is not one. */
export function urlB64ToUint8Array(b64u: string): Uint8Array<ArrayBuffer> {
  const pad = '='.repeat((4 - (b64u.length % 4)) % 4)
  const b64 = (b64u + pad).replace(/-/g, '+').replace(/_/g, '/')
  const raw = atob(b64)
  const out = new Uint8Array(new ArrayBuffer(raw.length))
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i)
  return out
}

/** The PushSubscription in the wire shape /api/push/subscribe validates:
 *  { endpoint, keys: { p256dh, auth } } — exactly what toJSON() gives,
 *  narrowed to the two halves the server encrypts against. */
export function subscriptionWire(sub: PushSubscription): {
  endpoint: string
  keys: { p256dh: string; auth: string }
} {
  const json = sub.toJSON()
  return {
    endpoint: json.endpoint ?? '',
    keys: {
      p256dh: json.keys?.p256dh || '',
      auth: json.keys?.auth || '',
    },
  }
}

export type PushOutcome = 'subscribed' | 'unsupported' | 'denied' | 'failed'

/** Register the worker and file this browser as a push recipient. Called
 *  from the settings toggle's click, AFTER the permission grant — the
 *  gesture that rule 1 demands for the prompt is the same one that files
 *  the subscription. An existing subscription is re-POSTed, not skipped:
 *  the upsert is idempotent, and "always send what we have" heals a row the
 *  server pruned or a database reset dropped without asking the user to
 *  notice anything was wrong. */
export async function ensurePushSubscription(): Promise<PushOutcome> {
  if (
    typeof Notification === 'undefined' ||
    !('serviceWorker' in navigator) ||
    !('PushManager' in window)
  ) {
    return 'unsupported'
  }
  if (Notification.permission !== 'granted') return 'denied'
  try {
    const reg = await navigator.serviceWorker.register('/sw.js')
    await navigator.serviceWorker.ready
    const key = await getJson<{ publicKey: string }>('/api/push/key')
    let sub = await reg.pushManager.getSubscription()
    if (!sub) {
      sub = await reg.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: urlB64ToUint8Array(key.publicKey),
      })
    }
    await postJson('/api/push/subscribe', subscriptionWire(sub))
    return 'subscribed'
  } catch {
    return 'failed'
  }
}

/** The off switch's other half: a pref alone stops the TAB's notifications,
 *  but a closed browser's copies come from the server — the subscription
 *  itself must go. Both ends are retired, and best-effort: an endpoint the
 *  push service already forgot still answers the unsubscribe POST (absent
 *  is the desired state, not an error), and a failure here leaves the
 *  delivery loop's own 404/410 prune to finish the job. */
export async function stopPushSubscription(): Promise<void> {
  if (!('serviceWorker' in navigator)) return
  try {
    const reg = await navigator.serviceWorker.getRegistration()
    const sub = await reg?.pushManager.getSubscription()
    if (sub) {
      // The wire shape must be read BEFORE unsubscribe(): after it, the
      // subscription object is empty and the endpoint is gone with it.
      const wire = subscriptionWire(sub)
      await sub.unsubscribe()
      await postJson('/api/push/unsubscribe', { endpoint: wire.endpoint })
    }
  } catch {
    // See above — the prune finishes what this could not.
  }
}
