// Talaria's service worker — the closed-tab half of notifications. It
// exists for exactly two events and touches no other traffic (no fetch
// handler: every request passes through untouched):
//
//   push: the server's ciphertext, decrypted by the browser into the
//     notification row's {id, title, body, href}. Show an OS notification
//     ONLY when no Talaria window is visible — an open tab already got the
//     in-app toast through /api/me/events, and a second copy in the corner
//     of the OS would be noise. This is shouldBrowserNotify()'s rule
//     (browser-notify.ts), enforced where the tab plane can't reach.
//
//   notificationclick: the person answered. Bring a Talaria window to the
//     front and land it on the notification's href; only when nothing is
//     open does a window get opened.

/** base64url → raw bytes, for the VAPID key /api/push/key hands out. */
function urlB64ToUint8Array(b64u) {
  const pad = '='.repeat((4 - (b64u.length % 4)) % 4)
  const b64 = (b64u + pad).replace(/-/g, '+').replace(/_/g, '/')
  const raw = atob(b64)
  const out = new Uint8Array(raw.length)
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i)
  return out
}

/** The PushSubscription in the wire shape /api/push/subscribe validates. */
function subscriptionWire(sub) {
  const json = sub.toJSON()
  return {
    endpoint: json.endpoint,
    keys: {
      p256dh: (json.keys && json.keys.p256dh) || '',
      auth: (json.keys && json.keys.auth) || '',
    },
  }
}

self.addEventListener('push', (event) => {
  let note = null
  try {
    note = event.data ? event.data.json() : null
  } catch {
    note = null // a payload that isn't our JSON carries nothing to show
  }
  if (!note || !note.id) return
  event.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({
        type: 'window',
        includeUncontrolled: true,
      })
      if (wins.some((w) => w.visibilityState === 'visible')) return
      await self.registration.showNotification(note.title, {
        body: note.body || '',
        // The notification row's id: one OS copy per row, so a re-delivery
        // replaces its own earlier copy instead of stacking.
        tag: note.id,
        data: { href: note.href || '' },
      })
    })(),
  )
})

self.addEventListener('notificationclick', (event) => {
  const href = event.notification.data && event.notification.data.href
  event.notification.close()
  event.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({
        type: 'window',
        includeUncontrolled: true,
      })
      const win = wins[0]
      if (win) {
        // navigate() is a WindowClient method not every engine has; without
        // it the focus still lands the person in Talaria, one click from
        // the bell the notification came from.
        if (href && typeof win.navigate === 'function') {
          try {
            await win.navigate(href)
          } catch {
            // A busy page or an edge the engine refuses — focus anyway.
          }
        }
        await win.focus()
        return
      }
      await self.clients.openWindow(href || '/')
    })(),
  )
})

// The subscription's quiet death: browsers rotate or expire push
// subscriptions and report it here. Resubscribe against the instance's
// CURRENT public key and re-file the row — otherwise every later delivery
// 404s and the server prunes a browser that still wants its notifications.
self.addEventListener('pushsubscriptionchange', (event) => {
  event.waitUntil(
    (async () => {
      try {
        const keyResp = await fetch('/api/push/key')
        if (!keyResp.ok) return
        const { publicKey } = await keyResp.json()
        const sub = await self.registration.pushManager.subscribe({
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
        // settings toggle's next pass re-files this browser. Nothing inside
        // the worker can do more.
      }
    })(),
  )
})
