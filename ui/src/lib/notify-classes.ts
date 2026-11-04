// Notification VOCABULARY, the legacy half: the row's wave-1 shape, the
// legacy route a class is delivered by, the digest switch and the instance
// email switch. Framework-free ON PURPOSE (`lib/notifications.ts` re-exports
// it beside its svelte-query hooks).
//
// THE CLASS LIST IS GONE FROM HERE. The server owns it: talaria-notify's
// registry (classes.rs) serves every class's id, label, blurb, category and
// defaults as `classes[]` on GET /api/notifications (`ClassInfo` in
// notify-wire.ts, I-12), and the settings matrix renders from that. A second
// list in the client is the first one drifting, and scripts/check-invariants.mjs
// fails one (`notify-class-list-in-the-client`).

export interface Notification {
  id: string
  kind: string
  title: string
  body: string
  href: string
  readAt: string | null
  createdAt: string
}

// ── Routing: destinations ──────────────────────────────────────────────────────

/** Where a notification is DELIVERED. The row is always written — it is the
 *  record, and the digest reads it — so this chooses delivery, not existence:
 *
 *  · `in_app`  inbox only. It waits for you and badges the bell.
 *  · `email`   mail only. The row still lands (history stays complete) but it
 *              arrives already read, so a class you read in your mail doesn't
 *              also nag you with an unread count you'll never clear.
 *  · `both`    mail AND an unread inbox row.
 */
export type NotifyRoute = 'in_app' | 'email' | 'both'
export const NOTIFY_ROUTES: ReadonlyArray<{ id: NotifyRoute; label: string }> = [
  { id: 'in_app', label: 'In-app' },
  { id: 'email', label: 'Email' },
  { id: 'both', label: 'Both' },
]

/** A notification class id, as the registry names it (`classes[].id`). No
 *  longer a closed union: the registry grows (W2-P's `participating` and
 *  `channel_activity`, W2-E's `account`) without a client release. */
export type NotifyClass = string

/** The legacy route per class (`prefs` on GET /api/notifications). The server
 *  derives it from the matrix and still accepts it from a stale bundle (I-13);
 *  the matrix itself reads `channels`. */
export type NotifyPrefs = Record<NotifyClass, NotifyRoute>

/** The digest switch's own type. The reserved `notify_prefs` key it lives
 *  under (`"digest"`) is the Rust api's now — `talaria-notify` owns the
 *  vocabulary and the derived answer; the client renders what it is served. */
export type DigestPref = 'on' | 'off'

export interface NotifySettings {
  prefs: NotifyPrefs
  /** The EFFECTIVE answer (`digestEnabled`), not the raw stored key — the
   *  control has to show what will actually happen. */
  digest: DigestPref
}

/** The INSTANCE-wide email master switch, off until an admin turns it on (the
 *  delivery read is the Rust notify engine's — `DELIVERY_KEY` in
 *  api/src/notify.rs — and off-by-default is its rule).
 *
 *  It rides on the same read as the per-user prefs so the panel can explain the
 *  gap: routing a class to "Email" does nothing while this is off, and someone
 *  who set it and hears nothing needs to be told that here rather than left to
 *  conclude the feature is broken.
 *
 *  Settings → Notifications renders it: the switch itself for an admin
 *  (`canSetDelivery`), and — when it is off — the explanation for everyone
 *  else. It was documented here for a while before any control existed, which
 *  is its own kind of bug: an emergency switch reachable only by hand-crafting
 *  a PATCH is not an emergency switch. */
export interface NotifyDelivery {
  emailEnabled: boolean
}
