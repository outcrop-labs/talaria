/// <reference lib="webworker" />
// Talaria's service worker — the closed-tab half of notifications. It
// exists for these events and touches no other traffic (no fetch handler:
// every request passes through untouched, which is also why taking control
// of open pages at once is safe):
//
//   install / activate: take over at once (skipWaiting, clients.claim), so
//     the tab that just registered is controlled and a new worker never
//     waits behind an old one.
//
//   push: the server's ciphertext, decrypted by the browser into the
//     notification row's {id, title, body, href, kind, renotify, createdAt}.
//     ONE BANNER PER ROW PER BROWSER: the open tabs post banners too (the
//     leader tab, notify-decide.ts), into this same registration's tag
//     namespace, and both record what they showed in the shared bannered
//     store. So a push is SKIPPED when the person is looking at Talaria (a
//     focused, visible window — Chromium exempts exactly that case from its
//     silent-push penalty), or when this version of the row is already
//     bannered or on screen. On WebKit nothing is skipped: Safari revokes a
//     subscription after a few pushes that show nothing, so it may banner
//     while a tab is focused until the server gates push on presence.
//
//   notificationclick: the person answered. The row is marked read (a
//     same-origin PUT), the window already at the href — else the most
//     recently focused one — is FOCUSED FIRST (inside the click's
//     activation), then asked to route there in place; only if it does not
//     answer within a second is it navigated, and only with no window at all
//     is one opened. The href is checked first: a path on this origin
//     (lib/notify-open.ts's rule, inlined), else no navigation at all — an
//     open window stays where it is, and only a new window opens at `/`.
//
// A build entry (vite.config.ts), not a public/ verbatim asset: it is
// bundled to an unhashed /sw.js so the registration URL in
// browser-notify.ts is the same in dev and prod. Must stay import-free —
// service workers load as classic scripts, and the whole point of this file
// is to be one self-contained bundle. The bannered store and the href rule
// are therefore INLINED copies of lib/notify-bannered.ts and
// lib/notify-open.ts (their names are pinned by notify-bannered.test.ts).

// The app's lib is DOM, and when both libs load the DOM `self` (Window) wins
// the global — bind the worker's view of it once, typed from the webworker lib.
const sw = self as unknown as ServiceWorkerGlobalScope

const ICON = '/icons/notify-192.png'
const BADGE = '/icons/notify-badge-96.png'
/** How long a page has to answer "open this" before the worker navigates it. */
const OPEN_ACK_MS = 1000
/** A notification row's id (inlined: lib/notify-open.ts `isRowId`). */
const ROW_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

interface PushNote {
  id?: string
  title?: string
  body?: string
  href?: string
  kind?: string
  renotify?: boolean
  createdAt?: string
}

sw.addEventListener('install', (event: ExtendableEvent) => {
  event.waitUntil(sw.skipWaiting())
})

sw.addEventListener('activate', (event: ExtendableEvent) => {
  event.waitUntil(sw.clients.claim())
})

// ── The bannered store (inlined: lib/notify-bannered.ts) ──────────────────────

const BANNERED_DB = 'talaria-notify'
const BANNERED_STORE = 'bannered'

function openBannered(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open(BANNERED_DB, 1)
    r.onupgradeneeded = () => {
      if (!r.result.objectStoreNames.contains(BANNERED_STORE)) r.result.createObjectStore(BANNERED_STORE)
    }
    r.onsuccess = () => resolve(r.result)
    r.onerror = () => reject(r.error)
  })
}

async function bannered(id: string): Promise<{ v: string; at: number } | undefined> {
  try {
    const db = await openBannered()
    return await new Promise((resolve) => {
      const r = db.transaction(BANNERED_STORE, 'readonly').objectStore(BANNERED_STORE).get(id)
      r.onsuccess = () => resolve(r.result as { v: string; at: number } | undefined)
      r.onerror = () => resolve(undefined)
    })
  } catch {
    return undefined
  }
}

async function markBannered(id: string, v: string): Promise<void> {
  try {
    const db = await openBannered()
    await new Promise<void>((resolve) => {
      const r = db.transaction(BANNERED_STORE, 'readwrite').objectStore(BANNERED_STORE).put({ v, at: Date.now() }, id)
      r.onsuccess = () => resolve()
      r.onerror = () => resolve()
    })
  } catch {
    /* a store that cannot be written never stops a banner */
  }
}

/** Safari (and every WebKit) — the engine that revokes silent pushes. */
function isWebKit(): boolean {
  const ua = navigator.userAgent
  return /AppleWebKit/.test(ua) && !/Chrome|Chromium|Edg|OPR/.test(ua)
}

/** The in-app path an href may open, or null (inlined: lib/notify-open.ts
 *  `safeInAppHref` — sw.test.ts runs both over one table). A leading `/`,
 *  not `//`, no backslash or control, no `.`/`..` segment in any `%2e`
 *  spelling, and the resolved path must stay on this origin when it is
 *  resolved AGAIN: `/..//evil.example` resolves to the path `//evil.example`,
 *  which `navigate()` and `openWindow()` read as another host. */
function safeHref(raw: unknown, origin: string): string | null {
  const h = typeof raw === 'string' ? raw.trim() : ''
  if (!h || h.length > 2048 || !h.startsWith('/') || h.startsWith('//')) return null
  for (let i = 0; i < h.length; i++) {
    const c = h.charCodeAt(i)
    if (c < 0x20 || c === 0x7f || c === 0x5c) return null
  }
  const dot = (h.split(/[?#]/, 1)[0] ?? '').split('/').some((seg) => {
    const x = seg.toLowerCase().replaceAll('%2e', '.')
    return x === '.' || x === '..'
  })
  if (dot) return null
  try {
    const u = new URL(h, origin)
    if (u.origin !== origin) return null
    const path = u.pathname + u.search + u.hash
    if (path.startsWith('//') || new URL(path, origin).origin !== origin) return null
    return path
  } catch {
    return null
  }
}

sw.addEventListener('push', (event: PushEvent) => {
  let note: PushNote | null = null
  try {
    note = event.data ? (event.data.json() as PushNote) : null
  } catch {
    note = null // a payload that isn't our JSON carries nothing to show
  }
  if (!note || !note.id) return
  const n = note as PushNote & { id: string }
  event.waitUntil(
    (async () => {
      const version = n.createdAt || ''
      if (!isWebKit()) {
        const wins = await sw.clients.matchAll({ type: 'window', includeUncontrolled: true })
        // The person is looking at Talaria: the page toasts it — except a
        // per-device TEST push, which is always shown: its whole point is a
        // banner on the device in front of you.
        if (n.kind !== 'test' && wins.some((w) => w.focused && w.visibilityState === 'visible')) return
        // This version is already bannered (the leader tab got there first).
        const seen = await bannered(n.id)
        if (seen && (!version || seen.v >= version)) return
        // Already on screen under its tag, and not a re-alert.
        if (!n.renotify && (await sw.registration.getNotifications({ tag: n.id })).length > 0) return
      }
      await markBannered(n.id, version)
      const ts = version ? Date.parse(version) : NaN
      await sw.registration.showNotification(n.title ?? '', {
        body: n.body || '',
        // The notification row's id: one OS copy per row, so a re-delivery
        // replaces its own earlier copy instead of stacking.
        tag: n.id,
        icon: ICON,
        badge: BADGE,
        data: { id: n.id, href: n.href || '', v: version },
        ...({ renotify: !!n.renotify, timestamp: Number.isFinite(ts) ? ts : undefined } as object),
      })
      // Tell the open pages, so a leader that was about to post the same row
      // stands down.
      const all = await sw.clients.matchAll({ type: 'window', includeUncontrolled: true })
      for (const c of all) c.postMessage({ type: 'talaria:bannered', id: n.id, v: version })
    })(),
  )
})

/** Ask one window to route in place; true when it answered within the bound. */
function askToOpen(win: WindowClient, msg: { id: string; href: string; summary?: boolean }): Promise<boolean> {
  return new Promise((resolve) => {
    const channel = new MessageChannel()
    const timer = setTimeout(() => {
      channel.port1.close()
      resolve(false)
    }, OPEN_ACK_MS)
    channel.port1.onmessage = (e: MessageEvent) => {
      if (e.data && e.data.type === 'talaria:notification-opened') {
        clearTimeout(timer)
        channel.port1.close()
        resolve(true)
      }
    }
    try {
      win.postMessage({ type: 'talaria:notification-open', ...msg }, [channel.port2])
    } catch {
      clearTimeout(timer)
      resolve(false)
    }
  })
}

sw.addEventListener('notificationclick', (event: NotificationEvent) => {
  const data = (event.notification.data || {}) as { id?: string; href?: string; summary?: boolean }
  event.notification.close()
  const origin = sw.location.origin
  // null: no openable link (none, or one the rule refused). An open window
  // then stays on the page it is on; with no window, Talaria opens home.
  const href = safeHref(data.href, origin)
  const id = typeof data.id === 'string' ? data.id : ''
  // The "N new" summary banner names no row: a click opens the bell.
  const summary = data.summary === true
  const work = (async () => {
    // Opening it is reading it. Same-origin (fetch's default credentials),
    // so the session cookie rides (SameSite=Lax) and no CSRF token is needed.
    // Only a row's id: the summary's and the confirmation banner's are not.
    if (ROW_ID.test(id) && !summary) {
      void fetch('/api/notifications', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ids: [id] }),
      }).catch(() => {})
    }
    const target = href ? new URL(href, origin).href : null
    const wins = await sw.clients.matchAll({ type: 'window', includeUncontrolled: true })
    // matchAll lists the most recently focused window first.
    let win: WindowClient | undefined = (target ? wins.find((w) => w.url === target) : undefined) ?? wins[0]
    if (!win) {
      await sw.clients.openWindow(href ?? '/')
      return
    }
    // FOCUS FIRST, inside the click's activation: a focus after an await on
    // a navigation targets a client that may no longer exist.
    try {
      win = (await win.focus()) ?? win
    } catch {
      /* no activation (a synthetic click): the route still changes */
    }
    if (await askToOpen(win, { id, href: href ?? '', ...(summary ? { summary } : {}) })) return
    // The page did not answer (an older build, a frozen tab): navigate it,
    // and focus the client navigate() RETURNS — the old one is gone. With no
    // link there is nowhere to send it: it was focused above.
    if (target && typeof win.navigate === 'function') {
      try {
        const moved = await win.navigate(target)
        if (moved) await moved.focus().catch(() => undefined)
      } catch {
        /* an uncontrolled page refuses navigate(); it was focused above */
      }
    }
  })()
  try {
    event.waitUntil(work)
  } catch {
    /* an untrusted (synthetic) event refuses waitUntil; the work runs */
  }
})

/** base64url → raw bytes, for the VAPID key /api/push/key hands out. */
function urlB64ToUint8Array(b64u: string): Uint8Array<ArrayBuffer> {
  const pad = '='.repeat((4 - (b64u.length % 4)) % 4)
  const b64 = (b64u + pad).replace(/-/g, '+').replace(/_/g, '/')
  const raw = atob(b64)
  const out = new Uint8Array(raw.length)
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i)
  return out
}

/** The PushSubscription in the wire shape /api/push/subscribe validates. */
function subscriptionWire(sub: PushSubscription) {
  const json = sub.toJSON()
  return {
    endpoint: json.endpoint,
    keys: {
      p256dh: (json.keys && json.keys.p256dh) || '',
      auth: (json.keys && json.keys.auth) || '',
    },
  }
}

// The subscription's quiet death: browsers rotate or expire push
// subscriptions and report it here (Firefox does; Chromium never has — the
// page's boot-time heal in browser-notify.ts covers it). Resubscribe against
// the instance's CURRENT public key and re-file the row — otherwise every
// later delivery 404s and the server prunes a browser that still wants its
// notifications.
sw.addEventListener('pushsubscriptionchange', (event: PushSubscriptionChangeEvent) => {
  event.waitUntil(
    (async () => {
      try {
        const keyResp = await fetch('/api/push/key')
        if (!keyResp.ok) return
        const { publicKey } = (await keyResp.json()) as { publicKey: string }
        const sub = await sw.registration.pushManager.subscribe({
          userVisibleOnly: true,
          applicationServerKey: urlB64ToUint8Array(publicKey),
        })
        const resp = await fetch('/api/push/subscribe', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(subscriptionWire(sub)),
        })
        if (!resp.ok) throw new Error(`subscribe answered ${resp.status}`)
      } catch {
        // The old row dies on its next delivery's 404/410 prune, and the
        // page's next boot re-files this browser. Nothing inside the worker
        // can do more.
      }
    })(),
  )
})
