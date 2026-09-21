// Notification VOCABULARY — classes, destinations, defaults, and the digest
// switch. Framework-free ON PURPOSE: the client draws the settings panel from
// `NOTIFY_CLASSES`, and the notification engine is the Rust api's
// (api/src/notify.rs carries the twin list and routes by `notify_class_of`) —
// so this file must never pull in svelte-query, which the client-hooks half
// of `lib/notifications.ts` imports. Same reason `OFF_BOARD_STATUSES` lives
// in `lib/task-const.ts`: the client's wire vocabulary exists exactly once.

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
