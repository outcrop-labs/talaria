// The user's notification inbox — polled here (30s, hidden tabs included),
// made live by the firehose (user-events.svelte.ts invalidates it on every
// `notification` and `notifications` event and on every stream reopen);
// refetches also piggyback on route changes.
//
// The routing/digest vocabulary lives in `lib/notify-classes.ts` (framework-
// free, shared with the server) and is re-exported here so client call sites
// keep one import.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { errorMessage, getJson, patchJson, putJson } from '@/lib/fetch-json'
import { toastError } from '@/lib/toast.svelte'
import type { DigestPref, NotifyDelivery, NotifyPrefs, NotifySettings, Notification } from './notify-classes'
import type { PromptMemory } from './notify-prompt'
import type { NotificationsPayloadV2Fields } from './notify-wire'

export * from './notify-classes'

// ── Queries ──────────────────────────────────────────────────────────────────

/** The bell's one read. Since the notification contract it also carries the
 *  v2 fields (`NotificationsPayloadV2Fields`: unreadForYou, seq, classes,
 *  channels, dnd, emailFrequency, features, next) and each row carries the v2
 *  row fields (`NotificationRowV2` in notify-wire.ts) — all optional here, and
 *  read by nothing yet: the wave-2 streams gate on `features`. */
export interface NotificationsPayload extends NotifySettings, NotificationsPayloadV2Fields {
  notifications: Notification[]
  unread: number
  delivery: NotifyDelivery
  /** Whether this user may change `delivery` — admins only. */
  canSetDelivery: boolean
  /** The desktop-alerts soft ask's memory (notify-prompt.ts). Absent from an
   *  api that predates it — and then the card never shows. */
  prompt?: PromptMemory
}

export function useNotifications() {
  return createQuery(() => ({
    queryKey: ['notifications'],
    queryFn: (): Promise<NotificationsPayload> => getJson<NotificationsPayload>('/api/notifications'),
    refetchInterval: 30_000,
    // THE ONE QUERY THAT POLLS HIDDEN. It feeds the OS banner, and a banner
    // is for exactly the moment Talaria is not on screen — a poll that slept
    // in a background tab meant an event lost in an SSE gap reached nobody
    // until the person came back. Browsers throttle a page hidden for more
    // than five minutes to about one timer a minute; Web Push is the
    // hidden-page path, this is the floor under it.
    refetchIntervalInBackground: true,
  }))
}

/** Record the person's answer to the soft ask, on the account: "Not now"
 *  (counted and spaced: 7 days, 30 days, then never) or the browser's grant
 *  (never again). Fails quiet — the card's answer is a courtesy, and the
 *  worst case is being asked once more. */
export async function saveNotifyPrompt(action: 'dismissed' | 'granted'): Promise<PromptMemory | null> {
  try {
    const j = await patchJson<{ prompt?: PromptMemory }>('/api/notifications', {
      prompt: action === 'dismissed' ? { dismissed: true } : { granted: true },
    })
    return j.prompt ?? null
  } catch {
    return null
  }
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
      toastError('Mark as read failed', e)
    }
    await qc.invalidateQueries({ queryKey: ['notifications'] })
    await qc.invalidateQueries({ queryKey: ['home'] })
    await qc.invalidateQueries({ queryKey: ['unreads'] })
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

