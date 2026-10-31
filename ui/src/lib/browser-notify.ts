// OS-level notifications from a browser tab. Two rules shape this file:
//
//  1. PERMISSION IS ASKED FROM A GESTURE, never on load. A prompt that
//     ambushes on arrival gets reflex-denied, and denied is near-permanent.
//     The in-context card (NotifyPrompt.svelte, notify-prompt.ts) is what
//     invites the click; the native prompt runs inside it.
//  2. AN OS NOTIFICATION IS FOR WHEN TALARIA IS NOT WHAT THE PERSON IS
//     LOOKING AT. In focus, the in-app toast already said it; a second copy
//     in the corner of the OS would be noise. Across tabs, notify-decide.ts
//     makes that call once per browser, not once per tab.
//
// The two cases split by where the person is. In a browser, this file's
// banners cover the open-but-background tab, and Web Push covers the CLOSED
// browser — /sw.js here, the VAPID/aes128gcm plane in the api's talaria-push.
// Both post into ONE tag namespace (the service worker registration's), so a
// row shown by the tab and pushed to the worker is one banner, not two.
//
// THE SUBSCRIPTION IS KEPT HEALTHY FROM BOOT TO SIGN-OUT. A browser whose
// permission is granted files (or re-files) its push subscription every time
// the app boots — no click needed once the grant exists — and replaces one
// bound to an old instance key. Signing out retires it at both ends.
//
// Inside the desktop shell the webview's Notification API reads denied and
// there is no site-settings UI. The shell posts through the OS instead
// (`desktop_notify`); the same pref still decides whether to fire. A quit
// desktop app is not listening — that is not Web Push.
import { inDesktopShell, shellShowNotification } from '@/lib/desktop-shell'
import { errorMessage, getJson, postJson } from '@/lib/fetch-json'
import { readText, writeText } from '@/lib/persist'

const PREF_KEY = 'talaria.browserNotify'

/** The banner's branding: a full-color icon and a monochrome badge (the
 *  small glyph Android and some Linux shells draw in the status area). */
export const NOTIFY_ICON = '/icons/notify-192.png'
export const NOTIFY_BADGE = '/icons/notify-badge-96.png'

/** `insecure`: the page is not a secure context (a dev stack on plain http
 *  reached by LAN IP or hostname), so the browser withholds notifications,
 *  push and service workers entirely — no prompt can change that. */
export type PermissionState = 'unsupported' | 'insecure' | 'default' | 'granted' | 'denied'

/** Inside the shell the OS grant is the shell's, not the webview's. A denied
 *  `Notification.permission` must not keep the toggle off. */
export function effectiveNotifyPermission(input: {
  inShell: boolean
  browser: PermissionState
}): PermissionState {
  return input.inShell ? 'granted' : input.browser
}

/** The browser's own answer, before the shell's override. The secure-context
 *  check comes FIRST: an insecure origin reports `denied`, which would send
 *  the person hunting for a site setting that cannot help. */
export function browserPermission(): PermissionState {
  if (typeof window !== 'undefined' && window.isSecureContext === false) return 'insecure'
  return typeof Notification === 'undefined' ? 'unsupported' : Notification.permission
}

export function permissionState(): PermissionState {
  return effectiveNotifyPermission({ inShell: inDesktopShell(), browser: browserPermission() })
}

/** Set the person's own off switch. Revoking browser permission means digging
 *  through browser site settings; this makes "stop showing these" one click
 *  while the grant stands. */
export function setBrowserNotifyPref(on: boolean): void {
  writeText(PREF_KEY, on ? 'on' : 'off')
}

/** Whether the person switched desktop alerts off in THIS browser. */
export function browserNotifyPrefOff(): boolean {
  return readText(PREF_KEY) === 'off'
}

/** The effective answer: nothing fires unless the browser granted it AND the
 *  person hasn't switched it off here. Default on: the grant IS the opt-in, and
 *  storage that refuses access (private mode) answers the same way — the grant
 *  still decides. */
export function browserNotifyEnabled(): boolean {
  return permissionState() === 'granted' && !browserNotifyPrefOff()
}

/** Ask. Must run inside a click handler (rule 1 above), and the native call
 *  is made BEFORE any await — Firefox and Safari drop the gesture across an
 *  await, and Chrome demotes the prompt to its quiet chip. */
export async function requestBrowserNotify(): Promise<PermissionState> {
  // The shell does not prompt the webview. Desktop apps post through the OS;
  // the pref set by this click is the on switch.
  if (inDesktopShell()) return 'granted'
  const now = browserPermission()
  if (now !== 'default') return now
  try {
    await Notification.requestPermission()
  } catch {
    // Some browsers reject instead of resolving; the state read below is
    // still the truth.
  }
  return permissionState()
}

/** Follow the permission LIVE: a grant or a block made in the browser's own
 *  site settings (or its address-bar chip) reaches the page without a
 *  reload. The Permissions API's `change` where the engine has it, and a
 *  re-read on focus and visibility everywhere (an engine whose Permissions
 *  API lacks `notifications` still gets there the next time the person
 *  looks). Returns the unsubscribe. */
export function watchPermission(fn: (p: PermissionState) => void): () => void {
  if (typeof window === 'undefined') return () => {}
  let last = permissionState()
  let stopped = false
  const check = () => {
    const p = permissionState()
    if (p === last) return
    last = p
    fn(p)
  }
  let status: PermissionStatus | null = null
  try {
    void navigator.permissions
      ?.query({ name: 'notifications' as PermissionName })
      .then((s) => {
        if (stopped) return
        status = s
        s.addEventListener('change', check)
      })
      .catch(() => {})
  } catch {
    /* no Permissions API */
  }
  window.addEventListener('focus', check)
  document.addEventListener('visibilitychange', check)
  return () => {
    stopped = true
    status?.removeEventListener('change', check)
    window.removeEventListener('focus', check)
    document.removeEventListener('visibilitychange', check)
  }
}

/** The one gate on this TAB showing a banner at all. Pure, so it is testable
 *  without a browser. Which tab of several shows it is notify-decide.ts. */
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

/** One OS banner, as the page posts it. */
export interface OsNote {
  id: string
  title: string
  body?: string
  href?: string
  kind?: string
  /** A row refreshed in place (a newer DM): re-show under the same tag. */
  renotify?: boolean
  /** The row version (createdAt) — the banner's timestamp, and what the
   *  bannered store records. */
  createdAt?: string
  /** Defaults to the row id: one OS copy per row. */
  tag?: string
  /** The "N new" summary banner: a click opens the bell, not a row. */
  summary?: boolean
}

/** The options every banner carries, from the page and from the worker. */
export function bannerOptions(n: OsNote): NotificationOptions {
  const tag = n.tag ?? n.id
  const ts = n.createdAt ? Date.parse(n.createdAt) : NaN
  return {
    body: n.body || undefined,
    tag,
    icon: NOTIFY_ICON,
    badge: NOTIFY_BADGE,
    data: { id: n.id, href: n.href ?? '', v: n.createdAt ?? '', ...(n.summary ? { summary: true } : {}) },
    // Not in every lib.dom yet; every engine Talaria supports reads them.
    ...({ renotify: !!n.renotify && !!tag, timestamp: Number.isFinite(ts) ? ts : undefined } as object),
  }
}

/** Post one banner. Through the service worker registration whenever one
 *  exists — the same tag namespace a push lands in, so the two sources
 *  collapse into one copy — and through `new Notification` only without one.
 *  In the shell, through the OS bridge. Fails soft: the in-app record is the
 *  row; a refused banner is a degraded corner, not an error. `onOpen` is the
 *  `new Notification` fallback's click (a registration's clicks reach the
 *  worker, which relays them to the page). */
export async function showOsNotification(
  n: OsNote,
  onOpen: (n: { id: string; href: string }) => void,
): Promise<boolean> {
  if (inDesktopShell()) {
    // Passed as a VARIABLE, not a literal: the extra hints (kind, renotify)
    // reach a shell that knows them and are simply not forwarded by one that
    // does not (I-2 in docs/NOTIFICATIONS.md).
    const shellNote = {
      title: n.title,
      body: n.body,
      tag: n.tag ?? n.id,
      href: n.href,
      kind: n.kind,
      renotify: n.renotify,
    }
    try {
      await shellShowNotification(shellNote)
      return true
    } catch {
      return false
    }
  }
  if (permissionState() !== 'granted') return false
  const opts = bannerOptions(n)
  try {
    const reg = 'serviceWorker' in navigator ? await navigator.serviceWorker.getRegistration() : undefined
    if (reg) {
      await reg.showNotification(n.title, opts)
      return true
    }
  } catch {
    /* fall through to the page's own constructor */
  }
  if (typeof Notification === 'undefined') return false
  try {
    const note = new Notification(n.title, opts)
    note.onclick = () => {
      window.focus()
      note.close()
      onOpen({ id: n.id, href: n.href ?? '' })
    }
    return true
  } catch {
    return false
  }
}

/** The tag of the local banner that confirms alerts are on. */
export const ENABLED_TAG = 'talaria-enabled'

/** "Turn on", from any surface (the card, the bell, Settings). The native
 *  prompt is requested SYNCHRONOUSLY in the caller's click, before anything
 *  awaits. On a grant: the pref goes on, this browser files its push
 *  subscription, and a local banner proves the path end to end ("Desktop
 *  alerts are on", through the same registration a push lands in). Answers
 *  the permission it ended in; the caller records the grant on the account
 *  and explains anything else (notify-prompt.ts `turnOnResult`). */
export async function turnOnDesktopAlerts(): Promise<PermissionState> {
  const asked = requestBrowserNotify()
  const after = await asked
  if (after !== 'granted') return after
  setBrowserNotifyPref(true)
  if (!inDesktopShell()) await ensurePushSubscription()
  await showOsNotification(
    {
      id: '',
      tag: ENABLED_TAG,
      title: 'Desktop alerts are on',
      body: 'Talaria will alert you here when someone needs you.',
    },
    () => {},
  )
  return after
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

/** Is a subscription bound to this instance key? `null` when the engine does
 *  not say (`options.applicationServerKey` is null) — unknown is KEPT, so a
 *  browser that hides the key never loops resubscribing. */
export function sameServerKey(bound: ArrayBuffer | null | undefined, want: Uint8Array): boolean | null {
  if (!bound) return null
  const a = new Uint8Array(bound)
  if (a.length !== want.length) return false
  for (let i = 0; i < a.length; i++) if (a[i] !== want[i]) return false
  return true
}

export type PushOutcome = 'subscribed' | 'unsupported' | 'denied' | 'failed'

/** What the Settings row says about alerts with every Talaria tab closed
 *  (Web Push) in this browser. */
export type PushStatus =
  | { state: 'unknown' }
  | { state: 'registered' }
  | { state: 'failed'; reason: string }
  | { state: 'unsupported' }

let pushStatus: PushStatus = { state: 'unknown' }
const pushListeners = new Set<(s: PushStatus) => void>()
const setPushStatus = (s: PushStatus) => {
  pushStatus = s
  for (const fn of pushListeners) fn(s)
}
export function currentPushStatus(): PushStatus {
  return pushStatus
}
export function onPushStatus(fn: (s: PushStatus) => void): () => void {
  pushListeners.add(fn)
  return () => pushListeners.delete(fn)
}

let inFlight: Promise<PushOutcome> | null = null

/** Register the worker and file this browser as a push recipient. Run at
 *  boot when the grant already stands (no gesture is needed once granted),
 *  when the permission turns granted, and from the Turn on click. Idempotent
 *  and single-flight. An existing subscription is re-POSTed, not skipped:
 *  the upsert is idempotent, and "always send what we have" heals a row the
 *  server pruned or a database reset dropped. A subscription bound to an
 *  OLD instance key (a restored database, a re-minted key) is replaced: the
 *  push services answer 403 for it forever, and Chromium never fires
 *  `pushsubscriptionchange` to say so. */
export function ensurePushSubscription(): Promise<PushOutcome> {
  inFlight ??= subscribeOnce().finally(() => {
    inFlight = null
  })
  return inFlight
}

async function subscribeOnce(): Promise<PushOutcome> {
  if (
    inDesktopShell() ||
    browserPermission() === 'insecure' ||
    typeof Notification === 'undefined' ||
    typeof navigator === 'undefined' ||
    !('serviceWorker' in navigator) ||
    typeof window === 'undefined' ||
    !('PushManager' in window)
  ) {
    setPushStatus({ state: 'unsupported' })
    return 'unsupported'
  }
  if (Notification.permission !== 'granted') return 'denied'
  try {
    const reg = await navigator.serviceWorker.register('/sw.js')
    await navigator.serviceWorker.ready
    const key = await getJson<{ publicKey: string }>('/api/push/key')
    const want = urlB64ToUint8Array(key.publicKey)
    let sub = await reg.pushManager.getSubscription()
    if (sub && sameServerKey(sub.options.applicationServerKey, want) === false) {
      // Bound to a key this instance no longer signs with: retire it at both
      // ends, then subscribe afresh. The wire shape is read BEFORE
      // unsubscribe(), which empties the object.
      const old = subscriptionWire(sub)
      await sub.unsubscribe().catch(() => false)
      await postJson('/api/push/unsubscribe', { endpoint: old.endpoint }).catch(() => {})
      sub = null
    }
    sub ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: want })
    await postJson('/api/push/subscribe', subscriptionWire(sub))
    setPushStatus({ state: 'registered' })
    return 'subscribed'
  } catch (e) {
    setPushStatus({ state: 'failed', reason: errorMessage(e) })
    return 'failed'
  }
}

/** The off switch's other half: a pref alone stops the TAB's notifications,
 *  but a closed browser's copies come from the server — the subscription
 *  itself must go. Both ends are retired, and best-effort: an endpoint the
 *  push service already forgot still answers the unsubscribe POST (absent
 *  is the desired state, not an error), and a failure here leaves the
 *  delivery loop's own 404/410 prune to finish the job. Bounded: sign-out
 *  waits on this at most `timeoutMs`. */
export async function stopPushSubscription(timeoutMs = 2000): Promise<void> {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return
  const work = (async () => {
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
  })()
  let timer: ReturnType<typeof setTimeout> | undefined
  await Promise.race([work, new Promise<void>((r) => (timer = setTimeout(r, timeoutMs)))])
  clearTimeout(timer)
  setPushStatus({ state: 'unknown' })
}

/** Close every banner this origin still has up — on screen and in the OS
 *  notification centre (GNOME's list, macOS's, Windows' Action Center): a
 *  sign-out must not leave the signed-out person's mentions for the next
 *  person at this machine to read. Bounded like the unsubscribe. The shell
 *  posts natively and clears its own. */
export async function closeShownNotifications(timeoutMs = 1000): Promise<void> {
  if (inDesktopShell() || typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return
  const work = (async () => {
    try {
      const reg = await navigator.serviceWorker.getRegistration()
      for (const n of (await reg?.getNotifications()) ?? []) n.close()
    } catch {
      /* nothing to close, or no registration */
    }
  })()
  let timer: ReturnType<typeof setTimeout> | undefined
  await Promise.race([work, new Promise<void>((r) => (timer = setTimeout(r, timeoutMs)))])
  clearTimeout(timer)
}
