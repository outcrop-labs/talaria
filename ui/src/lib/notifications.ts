// The user's notification inbox — polled; refetches piggyback on route changes.
//
// This module is ALSO where notification ROUTING is declared, because both
// halves need it: the client draws the settings panel from `NOTIFY_CLASSES`,
// and `server/notifications.ts` imports the same list to decide where a
// notification goes. The client cannot reach `server/`, but `server/` already
// imports from `@/lib` — so the list lives here and exists exactly once
// (same reason `OFF_BOARD_STATUSES` lives in `lib/task-const.ts`).
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface Notification {
  id: string
  kind: string
  title: string
  body: string
  href: string
  readAt: string | null
  createdAt: string
}

// ── Routing: classes, destinations, defaults ─────────────────────────────────

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

export type NotifyClass =
  | 'mention'
  | 'dm'
  | 'approval_pending'
  | 'judge_escalation'
  | 'agent_blocked'
  | 'gap_reported'
  | 'work_complete'

/** THE DEFAULTS, and why they are where they are.
 *
 *  One line decides them: **mail goes out when a person is the only one who can
 *  unblock something, and it stays blocked until they act.** Everything else is
 *  an outcome you will see the next time you look, and outcomes are the
 *  firehose — every status move on every ticket you watch.
 *
 *  Defaulting the whole list to email would spam a new workspace on day one and
 *  teach everybody to filter Talaria into a folder, which costs us the two
 *  classes that genuinely needed to reach them. Defaulting the whole list to
 *  in-app reproduces exactly the bug this milestone exists to fix: an approval,
 *  an escalation or a stopped agent sits in a bell nobody is looking at
 *  overnight. So the split is by *who is blocked*, not by importance.
 */
export const NOTIFY_CLASSES: ReadonlyArray<{
  id: NotifyClass
  label: string
  blurb: string
  fallback: NotifyRoute
}> = [
  {
    id: 'mention',
    label: 'Mentions & shares',
    blurb: 'Someone @mentioned you, assigned you a ticket, or shared a doc with you.',
    // A person deliberately addressed YOU. Low volume, high signal.
    fallback: 'both',
  },
  {
    id: 'dm',
    label: 'Direct messages',
    blurb: 'A teammate or an agent messaged you directly.',
    fallback: 'both',
  },
  {
    id: 'approval_pending',
    label: 'Approvals waiting on you',
    blurb: 'Work is parked until you approve or reject it.',
    // The definition of blocked-on-a-human.
    fallback: 'both',
  },
  {
    id: 'judge_escalation',
    label: 'Judge escalations',
    blurb: 'The quality gate stopped work and handed it to a person.',
    fallback: 'both',
  },
  {
    id: 'agent_blocked',
    label: 'Blocked agents',
    blurb: 'An agent hit a problem and stopped. It stays stopped until someone looks.',
    // Fans out to admins, so it is the noisiest of the "blocked" classes — but
    // an agent that stopped at 02:00 and is discovered at 09:00 is seven hours
    // of nothing, and the person who can restart it is not in the app.
    fallback: 'both',
  },
  {
    id: 'gap_reported',
    label: 'Capability gaps',
    blurb: 'An agent reported something it could not do, for a human to ratify.',
    // A queue item, not an interruption: nothing is waiting on THIS minute, and
    // the Studio's Suggested queue is where it gets ratified. Written ONCE per
    // work-shape (server/gaps.ts dedupes by signature), so the bell says "there
    // is a new kind of gap" and never counts the repeats.
    fallback: 'in_app',
  },
  {
    id: 'work_complete',
    label: 'Work finished',
    blurb: 'A ticket moved, or research you asked for is ready.',
    // The firehose. Every status move on every ticket you watch or are assigned.
    fallback: 'in_app',
  },
]

export type NotifyPrefs = Record<NotifyClass, NotifyRoute>

export function isNotifyRoute(v: unknown): v is NotifyRoute {
  return v === 'in_app' || v === 'email' || v === 'both'
}
export function isNotifyClass(v: unknown): v is NotifyClass {
  return typeof v === 'string' && NOTIFY_CLASSES.some((c) => c.id === v)
}

/** `kind` (what the writer called it) → the class a user has an opinion about.
 *  Kinds are free-form strings at ~10 call sites and predate this table; the
 *  map is how they stay free-form without every new one inventing a setting. */
const KIND_CLASS: Readonly<Record<string, NotifyClass>> = {
  // Someone pointed at you on purpose.
  mention: 'mention',
  'kb-comment': 'mention',
  'task-assigned': 'mention',
  'plan-share': 'mention',
  'research-share': 'mention',
  // Addressed to you, in a thread of your own.
  dm: 'dm',
  'agent-outreach': 'dm',
  // Blocked on a human.
  'agent-problem': 'agent_blocked',
  'workbench-repo-request': 'agent_blocked',
  // Outcomes.
  research: 'work_complete',
  'task-status': 'work_complete',
  // `judge_escalation` (server/judge.ts) and `gap_reported` (server/gaps.ts)
  // are NOT in this table on purpose: their writers name the CLASS as the kind,
  // which `notifyClassOf` accepts directly. A kind that is already the class it
  // belongs to has nothing to map.
}

/** The class a notification belongs to. Every class id is also accepted as a
 *  kind, so a new writer can name the class directly and skip the table.
 *
 *  An UNRECOGNIZED kind lands in `work_complete` — the quiet bucket — on
 *  purpose: a notification kind added by someone who never opened this file
 *  must not start mailing the whole org because it fell through a default. */
export function notifyClassOf(kind: string): NotifyClass {
  return KIND_CLASS[kind] ?? (isNotifyClass(kind) ? kind : 'work_complete')
}

/** Fill a stored (partial, possibly hand-edited) prefs blob out to the full
 *  table, dropping anything that isn't a route we understand. */
export function resolveNotifyPrefs(stored: unknown): NotifyPrefs {
  const raw = (stored ?? {}) as Record<string, unknown>
  return Object.fromEntries(
    NOTIFY_CLASSES.map((c) => [c.id, isNotifyRoute(raw[c.id]) ? raw[c.id] : c.fallback]),
  ) as NotifyPrefs
}

// ── The daily digest switch ──────────────────────────────────────────────────
//
// The digest is not a notification CLASS — it is one mail that SUMMARISES the
// queues, so it has no `in_app` reading and does not belong in the table above.
// It rides in the same `notify_prefs` jsonb blob under one reserved key, which
// is why that key is declared here and not spelled as a string literal in the
// three places that touch it (the settings panel, the API, server/digest.ts).

/** The reserved key inside `users.notify_prefs`. Reserved: it can never be a
 *  class id, because a class id is also accepted as a notification kind and
 *  "digest" is not an event anything writes. */
export const DIGEST_PREF_KEY = 'digest'

export type DigestPref = 'on' | 'off'

/** The EXPLICIT choice, or null when the person has never touched the switch.
 *  The distinction matters — see `digestEnabled`. */
export function storedDigestPref(stored: unknown): DigestPref | null {
  const v = ((stored ?? {}) as Record<string, unknown>)[DIGEST_PREF_KEY]
  return v === 'on' || v === 'off' ? v : null
}

/** Does this person get the daily digest?
 *
 *  ONE definition, because two would be a lie on screen: the switch in Settings
 *  shows the answer this function gives, and the job sends on the answer this
 *  function gives. If they could disagree, a person who reads "Daily digest —
 *  On" and receives nothing has no way to find out which half is wrong.
 *
 *  An explicit choice wins outright, in both directions. With no explicit
 *  choice it is DERIVED, in the direction that cannot spam: someone who has
 *  routed every class to in-app has said as loudly as this app lets them "do
 *  not email me", so they are not mailed a digest either. */
export function digestEnabled(stored: unknown): boolean {
  const explicit = storedDigestPref(stored)
  if (explicit) return explicit === 'on'
  const table = resolveNotifyPrefs(stored)
  return NOTIFY_CLASSES.some((c) => table[c.id] !== 'in_app')
}

// ── Queries ──────────────────────────────────────────────────────────────────

export interface NotifySettings {
  prefs: NotifyPrefs
  /** The EFFECTIVE answer (`digestEnabled`), not the raw stored key — the
   *  control has to show what will actually happen. */
  digest: DigestPref
}

/** The INSTANCE-wide email master switch, off until an admin turns it on (see
 *  `getNotifyDelivery` in server/notifications.ts for why off is the default).
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

export function useMarkNotificationsRead() {
  const qc = useQueryClient()
  return async (ids?: string[]) => {
    await fetch('/api/notifications', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(ids ? { ids } : {}),
    })
    await qc.invalidateQueries({ queryKey: ['notifications'] })
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
  const r = await fetch('/api/notifications', {
    method: 'PATCH',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(patch),
  }).catch(() => null)
  const j = (await r?.json().catch(() => null)) as {
    prefs?: NotifyPrefs
    digest?: DigestPref
    delivery?: NotifyDelivery
    canSetDelivery?: boolean
    error?: string
  } | null
  if (!r?.ok || !j?.prefs || !j.digest || !j.delivery) {
    return { error: j?.error ?? 'could not save your notification settings' }
  }
  return { prefs: j.prefs, digest: j.digest, delivery: j.delivery, canSetDelivery: j.canSetDelivery === true }
}
