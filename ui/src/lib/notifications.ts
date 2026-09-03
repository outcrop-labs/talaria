// The user's notification inbox — polled here (30s), made live by the SSE
// subscription in NotificationToasts; refetches also piggyback on route changes.
//
// The routing/digest vocabulary lives in `lib/notify-classes.ts` (framework-
// free, shared with the server) and is re-exported here so client call sites
// keep one import.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { errorMessage, getJson, patchJson, putJson } from '@/lib/fetch-json'
import { pushToast } from '@/lib/toast.svelte'
import type { DigestPref, NotifyDelivery, NotifyPrefs, NotifySettings, Notification } from './notify-classes'

export * from './notify-classes'

// ── Queries ──────────────────────────────────────────────────────────────────

export interface NotificationsPayload extends NotifySettings {
  notifications: Notification[]
  unread: number
  delivery: NotifyDelivery
  /** Whether this user may change `delivery` — admins only. */
  canSetDelivery: boolean
}

export function useNotifications() {
  return createQuery(() => ({
    queryKey: ['notifications'],
    queryFn: (): Promise<NotificationsPayload> => getJson<NotificationsPayload>('/api/notifications'),
    refetchInterval: 30_000,
  }))
}

/** Mark notifications read: by ids, by href (everything pointing at the place
 *  just opened), or all of them when called bare. The server gives ids
 *  priority when both arrive.
 *
 *  Invalidation is the whole point of this hook existing: a mark-read that
 *  left the caches alone would clear the server's rows and leave the bell and
 *  the brief showing yesterday's unread — the exact "never disappear" this
 *  wiring exists to end. `['home']` rides along because the inbox brief's
 *  notification arm reads the same rows. */
export function useMarkNotificationsRead() {
  const qc = useQueryClient()
  return async (what?: { ids?: string[]; href?: string }) => {
    try {
      await putJson<{ ok: true }>('/api/notifications', what ?? {})
    } catch (e) {
      // Call sites fire this with `void` — without a catch a failed
      // mark-read is an unhandled rejection, and the badge quietly lies.
      pushToast({ title: 'Mark as read failed', body: errorMessage(e), tone: 'danger' })
    }
    await qc.invalidateQueries({ queryKey: ['notifications'] })
    await qc.invalidateQueries({ queryKey: ['home'] })
  }
}

/** Everything a save gives back: the per-user settings AND the instance switch.
 *
 *  `delivery` rides on the response for the same reason it rides on the read —
 *  the panel's job is to show what will actually happen, and turning the master
 *  switch off changes the meaning of every row above it in the same instant. A
 *  save that returned only the per-user half would leave the panel drawing the
 *  new switch position from its own optimistic guess, which is exactly how a
 *  kill switch comes to show "Off" on a screen that is still sending. */
export interface NotifySettingsResult extends NotifySettings {
  delivery: NotifyDelivery
  canSetDelivery: boolean
}

/** Save one class's destination, the digest switch, or — for an admin — the
 *  instance-wide email master switch. Returns the server's effective settings,
 *  or an error string; the panel shows it rather than pretending the save
 *  landed.
 *
 *  A member who sends `delivery` gets a 403 and NOTHING is saved (the route
 *  refuses the whole PATCH rather than half-applying it), so the error path
 *  here is the whole story for that case. */
export async function saveNotifySettings(
  patch: { prefs?: Partial<NotifyPrefs>; digest?: DigestPref; delivery?: NotifyDelivery },
): Promise<NotifySettingsResult | { error: string }> {
  try {
    const j = await patchJson<NotifySettingsResult>('/api/notifications', patch)
    if (!j.prefs || !j.digest || !j.delivery) return { error: 'could not save your notification settings' }
    return { prefs: j.prefs, digest: j.digest, delivery: j.delivery, canSetDelivery: j.canSetDelivery === true }
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

