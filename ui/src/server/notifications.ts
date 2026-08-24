// Notifications — a user's inbox, and the fan-out that gets one out of the
// building.
//
// `addNotification` is the single writer: ten call sites (mentions, DMs, ticket
// moves, agent problems, research, shares) all land here, so this is the one
// place that can decide a notification is worth an email. Nothing about a call
// site changes — it writes a row exactly as before and delivery happens on the
// way past.
//
// Four rules shape everything below.
//
//   1. A notification is a SIDE EFFECT of someone else's request. The person
//      who @mentioned you is waiting on a POST; their request must not slow
//      down, and must not fail, because our SMTP server is unreachable. So the
//      row is written inline and the mail goes on a QUEUE THE SCHEDULER DRAINS.
//      No request path ever waits on a socket to a mail server.
//   2. MAIL IS OFF UNTIL AN ADMIN TURNS IT ON — `getNotifyDelivery`. Five of
//      the seven classes default to email and every existing user's
//      `notify_prefs` is `{}`, so shipping this switched on mails an entire
//      workspace that never asked for it, on the first deploy, with no way to
//      stop it short of another deploy.
//   3. THE QUEUE IS BOUNDED. An unbounded queue is not a safety mechanism; it
//      is the same outage with a delay and a memory leak on top. It has a
//      depth, a drop policy, a breaker for a dead transport, and counters that
//      say what it did.
//   4. Nothing is swallowed. A send that fails is logged with the recipient,
//      the kind and the provider's reason; a drop is logged with what was
//      dropped. "Fire and forget" is only the first half; the forgetting is
//      what turns a broken mail server into a mystery.
//   5. A FAILED SEND DEGRADES TO THE INBOX. Mail is the second channel and the
//      notification row is the first, so nothing about a mail that did not
//      arrive may cost the person the row: it is written unread, it is only
//      filed read once a provider has accepted the message, and a failed send
//      is retried before it is given up on — out loud, and never silently
//      removed from the queue.
import { getSetting, setSetting } from './audit'
import { db } from './db/pg'
import { emailButton, emailEscape, emailShell, sendEmail } from './email'
import { instanceBaseUrl } from './instance'
import { markBriefStale } from './daily-brief-stale'
import { publishUser } from './realtime'
import { registerJob } from './scheduler'
import {
  DIGEST_PREF_KEY,
  NOTIFY_CLASSES,
  digestEnabled,
  isNotifyRoute,
  notifyClassOf,
  resolveNotifyPrefs,
  type DigestPref,
  type NotifyClass,
  type NotifyPrefs,
  type NotifyRoute,
  type NotifySettings,
} from '@/lib/notify-classes'

export type { DigestPref, NotifyClass, NotifyPrefs, NotifyRoute, NotifySettings }
export { notifyClassOf } from '@/lib/notify-classes'

export interface Notification {
  id: string
  kind: string
  title: string
  body: string
  href: string
  readAt: string | null
  createdAt: string
}

export interface NotificationInput {
  kind: string
  title: string
  body?: string
  href?: string
}

// ── Preferences ──────────────────────────────────────────────────────────────

/** Postgres 42703 — undefined_column. The one failure with a specific cause
 *  worth naming: `users.notify_prefs` arrived in a migration, and any process
 *  running this tree against a database that has not applied it yet (a
 *  mid-deploy replica, a rollback) hits exactly this. */
const missingPrefsColumn = (e: unknown): boolean => (e as { code?: string } | null)?.code === '42703'

// Read failures are logged, not swallowed — but this read sits under a route
// the client polls every 30 seconds, and a log line per poll per user turns a
// legible failure into a wall. One line a minute says the same thing.
let prefsWarnedAt = 0
const PREFS_WARN_EVERY_MS = 60_000

/** The raw `notify_prefs` blob. One read, two readings — the routing table and
 *  the digest switch live in the same jsonb column.
 *
 *  FORGIVING BY DESIGN. It returns undefined rather than throwing, which the
 *  resolvers turn into the defaults, because a preference is a MODIFIER on
 *  behaviour that has a perfectly good answer without it. The alternative is
 *  what this replaced: `addNotification` caught the failure and carried on
 *  while the notifications route did not, so a database missing one column
 *  served 500s for the entire panel — inbox, unread count and all — over a
 *  column that only decides where mail goes.
 *
 *  This does NOT hide a database outage: every other read on this route is a
 *  plain query that will fail on its own and be seen. It hides exactly one
 *  thing, the absence of one column, behind exactly the answer that column
 *  would have given for a user who has never touched it. */
async function prefsBlob(userId: string): Promise<unknown> {
  try {
    const sql = await db()
    const rows = (await sql`select notify_prefs as prefs from users where id = ${userId}`) as unknown as Array<{
      prefs: unknown
    }>
    return rows[0]?.prefs
  } catch (e) {
    if (Date.now() - prefsWarnedAt > PREFS_WARN_EVERY_MS) {
      prefsWarnedAt = Date.now()
      if (missingPrefsColumn(e)) {
        console.error(
          '[notifications] users.notify_prefs does not exist on this database, so every user is getting the DEFAULT' +
            ' notification routing and nobody can change it. This process is running ahead of the schema — apply the' +
            ' migrations (they run on first query at boot) or roll the code back to match.',
        )
      } else {
        console.error(`[notifications] could not read notify_prefs for ${userId} — falling back to the defaults`, e)
      }
    }
    return undefined
  }
}

/** A user's full routing table: what they saved, filled out with the defaults
 *  for every class they never touched. Never throws for a missing user — an
 *  unknown id gets the defaults, and the caller's own query is what will fail
 *  if the user really doesn't exist. */
export async function getNotifyPrefs(userId: string): Promise<NotifyPrefs> {
  return resolveNotifyPrefs(await prefsBlob(userId))
}

/** Everything the Settings panel shows: the routing table AND the effective
 *  digest answer, from one row, so the two can never be read a moment apart. */
export async function getNotifySettings(userId: string): Promise<NotifySettings> {
  const raw = await prefsBlob(userId)
  return { prefs: resolveNotifyPrefs(raw), digest: digestEnabled(raw) ? 'on' : 'off' }
}

/** Merge a patch into the stored blob and return the effective settings.
 *  Unknown classes and unknown routes are dropped rather than stored — the
 *  column is jsonb and this is the only writer that keeps it honest.
 *
 *  `digest` is the one non-class key the blob carries (see DIGEST_PREF_KEY):
 *  the daily digest is a summary of the queues, not an event class, and it is
 *  stored here rather than in a column of its own because the column is jsonb
 *  and a migration for one enum would be the wrong trade. It is filtered by the
 *  same rule as everything else — an unrecognised value is dropped, never
 *  written, because `digestEnabled` DERIVES an answer when the key is absent
 *  and a garbage value must fall back to that derivation, not disable it. */
export async function setNotifySettings(
  userId: string,
  patch: { prefs?: Partial<NotifyPrefs>; digest?: DigestPref },
): Promise<NotifySettings> {
  const clean: Record<string, string> = {}
  for (const c of NOTIFY_CLASSES) {
    const v = patch.prefs?.[c.id]
    if (v !== undefined && isNotifyRoute(v)) clean[c.id] = v
  }
  if (patch.digest === 'on' || patch.digest === 'off') clean[DIGEST_PREF_KEY] = patch.digest
  const sql = await db()
  const rows = (await sql`
    update users set notify_prefs = coalesce(notify_prefs, '{}'::jsonb) || ${sql.json(clean)}::jsonb
    where id = ${userId}
    returning notify_prefs as prefs
  `) as unknown as Array<{ prefs: unknown }>
  if (!rows[0]) throw new Error('no such user')
  return { prefs: resolveNotifyPrefs(rows[0].prefs), digest: digestEnabled(rows[0].prefs) ? 'on' : 'off' }
}

// ── The master switch ────────────────────────────────────────────────────────

export interface NotifyDelivery {
  /** Does this instance send notification email AT ALL? */
  emailEnabled: boolean
}

const DELIVERY_KEY = 'notify_delivery'

/** OFF, and this is the deliberate part.
 *
 *  The per-user table in lib/notifications.ts is tuned for a workspace that
 *  CHOSE email: five of the seven classes default to `both`, which is right for
 *  someone who set this up on purpose. It is not right as the first thing an
 *  existing instance does after an upgrade. Every user in every database
 *  already out there has `notify_prefs = '{}'` — not "email is fine", but "has
 *  never been asked". Reading a default as consent is how a product ships a
 *  feature by mailing everyone who ever signed up, from an address nobody
 *  recognises, about mentions from six months ago.
 *
 *  So the two questions are kept apart, and both must say yes. The per-user
 *  table answers "where would this person like this class delivered". This
 *  answers "is this deployment sending mail yet" — an operator decision, made
 *  once, by someone who has configured a provider and a From address and knows
 *  the domain is verified.
 *
 *  It is the same shape and the same default as `getOutreachConfig`, for the
 *  same reason: that switch is off by default because it MESSAGES PEOPLE. So
 *  does this one. Off is also the only default that makes this a kill switch
 *  rather than a feature flag — the state an operator can return to at 3am
 *  without a deploy is worth more than a day of earlier email.
 *
 *  The cost of being wrong in this direction is that an admin turns it on a
 *  week later. The cost of being wrong in the other direction cannot be
 *  undone: the mail has already arrived. */
const DELIVERY_DEFAULT: NotifyDelivery = { emailEnabled: false }

// Cached because `addNotification` asks on every notification and the answer
// changes about once a year. The DRAIN does not use the cache — see below.
const DELIVERY_TTL_MS = 10_000

/** Is this instance sending notification mail?
 *
 *  `fresh` skips the cache. The drain passes it, so flipping the switch off
 *  takes effect on the next drain rather than up to a TTL later: a kill switch
 *  that keeps sending for another ten seconds after you throw it is a worse
 *  thing to hand an operator than no switch at all. */
export async function getNotifyDelivery(opts: { fresh?: boolean } = {}): Promise<NotifyDelivery> {
  const s = state()
  if (!opts.fresh && s.delivery && Date.now() - s.delivery.at < DELIVERY_TTL_MS) return s.delivery.value
  const stored = await getSetting<Partial<NotifyDelivery>>(DELIVERY_KEY, {})
  // Anything that is not exactly `true` is off. A hand-edited settings row must
  // not be able to turn mail on by accident.
  const value: NotifyDelivery = { emailEnabled: stored.emailEnabled === true }
  s.delivery = { at: Date.now(), value }
  return value
}

/** Turn notification email on or off for the whole instance. Admin-level. */
export async function setNotifyDelivery(patch: Partial<NotifyDelivery>): Promise<NotifyDelivery> {
  const current = await getNotifyDelivery({ fresh: true })
  const next: NotifyDelivery = {
    emailEnabled: patch.emailEnabled === undefined ? current.emailEnabled : patch.emailEnabled === true,
  }
  await setSetting(DELIVERY_KEY, next)
  state().delivery = { at: Date.now(), value: next }
  console.warn(`[notifications] email delivery is now ${next.emailEnabled ? 'ON' : 'OFF'} for this instance`)
  return next
}

// ── THE ONE DOOR OUT ─────────────────────────────────────────────────────────
//
// Every mail Talaria sends ON BEHALF OF A NOTIFICATION OR A DIGEST leaves
// through `sendGatedMail`, and nothing else in the codebase may call
// `sendEmail` for that purpose. `scripts/check-invariants.mjs` fails the build
// on a fifth caller of the transport, because this is the second time this
// exact bug has been written:
//
//   · `addNotification` asked the switch, so notification mail was gated.
//   · `runDigest` called `sendEmail` directly and never asked, so an admin who
//     turned email OFF still got a daily digest mailed to every user in the
//     workspace — from a control that is named "email delivery", is audited,
//     and did not do what its name said.
//
// The fix is not another `if (enabled)` at the second call site. Two copies of
// a rule is how the first one drifts. It is one function that both paths must
// go through, and a CI rule that fails on the third path that tries not to.
//
// It reads the switch FRESH on every single mail, not once per batch. A kill
// switch you throw at 03:00 has to stop the drain that is already running, not
// the one after it — and one small settings read per mail is nothing next to an
// SMTP round trip.

export interface GatedSendResult {
  ok: boolean
  /** The instance switch is OFF. Explicitly NOT a failure: nothing was
   *  attempted, no transport was involved, and nothing should be retried until
   *  an admin turns it back on. Callers that count failures (the outbox
   *  breaker, the digest's retry) must not count this. */
  blocked?: boolean
  error?: string
}

// One log line a minute is enough to say "mail is off and things are being
// refused". Per mail, on a workspace-wide digest pass, it is the outage.
let blockedWarnedAt = 0
const BLOCKED_WARN_EVERY_MS = 60_000
let blockedSinceWarn = 0

/** Send one piece of notification-or-digest mail, IF this instance is allowed
 *  to send mail at all.
 *
 *  `unsubscribeUrl` becomes `List-Unsubscribe`. Every mail that goes to a
 *  person because of a standing preference — which is all of them here — needs
 *  it: a mail client that can offer one-tap unsubscribe does not offer "report
 *  spam" instead, and one spam report costs the sending domain more than a
 *  year of digests earns it. Omitted when this deployment has no verified
 *  domain, because a header pointing at a URL that does not resolve is worse
 *  than no header. */
export async function sendGatedMail(mail: {
  to: string
  subject: string
  html: string
  text?: string
  unsubscribeUrl?: string | null
  /** What is being refused, for the log line. */
  what: string
}): Promise<GatedSendResult> {
  let delivery: NotifyDelivery
  try {
    delivery = await getNotifyDelivery({ fresh: true })
  } catch (e) {
    // We do not know whether we are allowed to send, so we do not send. Failing
    // OPEN here would mean a transient database blip mails the workspace out of
    // an instance whose operator had switched mail off — the one outcome that
    // cannot be taken back.
    console.error(`[mail] could not read the delivery switch — not sending "${mail.what}"`, e)
    return { ok: false, error: 'could not read the email delivery switch' }
  }
  if (!delivery.emailEnabled) {
    blockedSinceWarn++
    if (Date.now() - blockedWarnedAt > BLOCKED_WARN_EVERY_MS) {
      console.warn(
        `[mail] email delivery is OFF for this instance — refused ${blockedSinceWarn} mail(s), most recently "${mail.what}".` +
          ' An admin can turn it back on under Settings → Notifications.',
      )
      blockedWarnedAt = Date.now()
      blockedSinceWarn = 0
    }
    return { ok: false, blocked: true, error: 'email delivery is switched off for this instance' }
  }
  return sendEmail({
    to: mail.to,
    subject: mail.subject,
    html: mail.html,
    text: mail.text,
    headers: mail.unsubscribeUrl ? { 'List-Unsubscribe': `<${mail.unsubscribeUrl}>` } : undefined,
  })
}

// ── The outbox ───────────────────────────────────────────────────────────────
//
// Serial, not a fan of parallel sends: a ticket that notifies eight watchers
// would otherwise open eight SMTP connections at once, and a burst is exactly
// when a provider starts rate-limiting.
//
// What this replaced was a single promise chain with `.then(send)` appended per
// notification. That chain had no depth, so a dead SMTP server did not stop
// anything — it accumulated. Every mention and DM in the product appended
// another closure holding another notification behind a link that was going to
// wait for a connection timeout, and the only bound on the whole structure was
// the heap. Nothing reported the depth, and nothing could drop from it.
//
// So: a real queue, with the four things a queue needs to be safe.
//   DEPTH    — MAX_QUEUED, and past it new mail is refused at the door.
//   DROP     — refuse the NEWEST rather than evicting the oldest. Load shedding
//              at the entrance is predictable and it never throws away work
//              already accepted. The in-app row has landed either way, so a
//              dropped mail costs a notification its second delivery channel,
//              not its existence.
//   BREAKER  — after enough consecutive failures the drain stops trying for a
//              while. Without it a dead transport burns the full send deadline
//              per item forever, on every tick, and the log is the outage.
//   COUNTERS — sent, failed, dropped, discarded. A queue you cannot see is
//              indistinguishable from a queue that is working.

interface QueuedMail {
  userId: string
  n: NotificationInput
  queuedAt: number
  /** The notification row this mail duplicates, when — and ONLY when — the row
   *  is to be filed read once the mail is actually accepted by a provider. Null
   *  for `both`, where the in-app copy is wanted unread either way.
   *
   *  It is an id and not a boolean because the read-mark now happens AFTER
   *  delivery, in the drain, rather than at insert time. See `addNotification`. */
  markReadId: string | null
  /** How many times a transport has already refused this one. */
  attempts: number
}

interface OutboxState {
  queue: QueuedMail[]
  dropped: number
  discarded: number
  sent: number
  failed: number
  /** Retried to the limit and given up on. Distinct from `failed`, which counts
   *  ATTEMPTS: one abandoned mail is MAX_SEND_ATTEMPTS failures. */
  abandoned: number
  consecutiveFailures: number
  /** Epoch ms before which the drain does not attempt a send. */
  pausedUntil: number
  draining: Promise<MailDrainResult> | null
  delivery: { at: number; value: NotifyDelivery } | null
}

// On globalThis so a Vite HMR reload reuses one outbox instead of forking a
// second queue and a second set of counters.
const g = globalThis as unknown as { __talariaNotifyOutbox?: OutboxState }

function state(): OutboxState {
  return (g.__talariaNotifyOutbox ??= {
    queue: [],
    dropped: 0,
    discarded: 0,
    sent: 0,
    failed: 0,
    abandoned: 0,
    consecutiveFailures: 0,
    pausedUntil: 0,
    draining: null,
    delivery: null,
  })
}

/** How many mails may wait. Sized for the realistic worst case — an agent fleet
 *  fanning a burst of problems out to every admin — and small enough that the
 *  memory is irrelevant. If this is ever hit, the answer is not a bigger number
 *  (the transport is not keeping up and a larger queue only delays the same
 *  outcome) — it is the alert the log line is there to produce. */
const MAX_QUEUED = 500

/** How long one drain may spend sending before it yields to the next tick.
 *  Shorter than the job's interval so a drain does not routinely overlap its
 *  own schedule. */
const DRAIN_BUDGET_MS = 25_000

/** Consecutive failures before the breaker opens, and how long it stays open.
 *  Five because a single bad address should not stop everyone else's mail, and
 *  five in a row is a transport, not an address. */
const BREAKER_AFTER = 5
const BREAKER_COOLDOWN_MS = 5 * 60_000

/** How many times one mail is offered to the transport before it is given up
 *  on. Three because the failures worth retrying are the transient ones — a
 *  timeout, a 429, a provider blip — and a bad address fails identically three
 *  times and costs nothing but three log lines. Past it the mail is abandoned
 *  OUT LOUD, and the notification is still sitting unread in the recipient's
 *  in-app inbox, which is the whole point of not pre-filing it read. */
const MAX_SEND_ATTEMPTS = 3

/** Accept one mail for later sending, or refuse it. NEVER sends, never blocks,
 *  never throws — this is called from a request path. */
function enqueueMail(userId: string, n: NotificationInput, markReadId: string | null): void {
  const s = state()
  if (s.queue.length >= MAX_QUEUED) {
    s.dropped++
    // The first drop is the interesting one; after that, one line per hundred
    // is enough to show it is still happening without becoming the outage.
    if (s.dropped === 1 || s.dropped % 100 === 0) {
      console.error(
        `[notifications] mail queue is full at ${MAX_QUEUED} — dropping "${n.kind}" for user ${userId}` +
          ` (${s.dropped} dropped since boot). The in-app notification still landed. The transport is not keeping up.`,
      )
    }
    return
  }
  s.queue.push({ userId, n, queuedAt: Date.now(), markReadId, attempts: 0 })
}

export interface MailDrainResult {
  sent: number
  failed: number
  /** Failed this pass and put BACK on the queue for the next one. */
  requeued: number
  /** Failed MAX_SEND_ATTEMPTS times and given up on. The notification is still
   *  unread in the recipient's inbox; only the email was lost. */
  abandoned: number
  /** Queued mail thrown away because delivery is switched off. */
  discarded: number
  /** Still queued when this drain gave up its slot. */
  remaining: number
  /** Refused at the door since boot. */
  dropped: number
  /** The breaker was open, so nothing was attempted. */
  paused: boolean
}

/** The mail is out, so the row it duplicates can be filed read.
 *
 *  Only ever called after a provider ACCEPTED the message — never for a mail
 *  that was queued, dropped, discarded, refused by the master switch, or sent to
 *  a user with no address. A notification whose email did not go out has to stay
 *  unread, because the inbox is then the only place the person will ever see it. */
async function markReadOnDelivery(notificationId: string): Promise<void> {
  try {
    const sql = await db()
    await sql`update notifications set read_at = now() where id = ${notificationId} and read_at is null`
  } catch (e) {
    // Harmless in the direction that matters: the row stays unread, so the
    // person sees it twice rather than not at all.
    console.error(`[notifications] mailed ${notificationId} but could not file the row read:`, e)
  }
}

/** Send what is queued, within a time budget. Called by the `notification-mail`
 *  job — and directly by verification, which is why it returns numbers rather
 *  than only logging them.
 *
 *  Concurrent calls share one drain: the queue is a single serial consumer by
 *  design, and two drains would be the parallel-connection burst the queue
 *  exists to avoid. */
export function drainNotificationMail(opts: { budgetMs?: number } = {}): Promise<MailDrainResult> {
  const s = state()
  if (s.draining) return s.draining
  const run = drainOnce(s, opts.budgetMs ?? DRAIN_BUDGET_MS).finally(() => {
    s.draining = null
  })
  s.draining = run
  return run
}

async function drainOnce(s: OutboxState, budgetMs: number): Promise<MailDrainResult> {
  const result: MailDrainResult = {
    sent: 0,
    failed: 0,
    requeued: 0,
    abandoned: 0,
    discarded: 0,
    remaining: s.queue.length,
    dropped: s.dropped,
    paused: false,
  }

  // Read the switch FRESH, every drain, and treat it as authoritative over
  // anything already queued. "Off" has to mean no mail leaves — including the
  // mail that was accepted while it was still on. That is what makes it a kill
  // switch rather than a setting that takes effect eventually.
  let delivery: NotifyDelivery
  try {
    delivery = await getNotifyDelivery({ fresh: true })
  } catch (e) {
    // The switch could not be read, so we do not know whether we are allowed to
    // send. Fail CLOSED and keep the queue: sending on an unread switch is the
    // one outcome that cannot be taken back, and a transient database blip must
    // not also destroy the backlog.
    console.error('[notifications] could not read the delivery switch — sending nothing this pass', e)
    return result
  }

  if (!delivery.emailEnabled) {
    if (s.queue.length) {
      result.discarded = s.queue.length
      s.discarded += result.discarded
      // Dropped rather than held. Held mail is a trap: an admin who turns email
      // on for the first time three months in would get the entire backlog of
      // every mention since boot delivered in one burst, which is precisely the
      // mass-mailing this switch exists to prevent.
      s.queue.length = 0
      console.warn(
        `[notifications] email delivery is off — discarded ${result.discarded} queued mail(s).` +
          ' Every one of them is still in its recipient\'s in-app inbox.',
      )
    }
    result.remaining = 0
    return result
  }

  if (Date.now() < s.pausedUntil) {
    result.paused = true
    return result
  }

  const deadline = Date.now() + budgetMs
  // Bounded by what was queued when this pass STARTED. A failed mail goes back
  // on the queue now (see below) and without this bound a transport that is
  // refusing everything would spin the same handful of items round the loop for
  // the whole budget instead of yielding to the next tick. Each item gets at
  // most one attempt per drain.
  let offers = s.queue.length
  while (offers-- > 0 && s.queue.length && Date.now() < deadline) {
    // PEEKED, not shifted. What this replaced took the item off the queue before
    // the send, so a failure destroyed it: three mails timed out and simply
    // vanished — nothing requeued, nothing retried, and (because the row had
    // been filed read at insert time) no in-app trace of them either. The queue
    // drained to zero and the notifications were gone. Nothing leaves this queue
    // now until it has been delivered, refused by the switch, or given up on out
    // loud.
    const item = s.queue[0]
    if (!item) break
    const r = await sendNotificationEmail(item.userId, item.n)
    if (r.blocked) {
      // The switch went off WHILE this drain was running. Same rule as the top
      // of this function, applied to the mail in hand as well as the backlog:
      // stop now, throw the rest away, and never report a refused mail as sent.
      // `item` is PEEKED, not shifted, so it is still counted in queue.length.
      const lost = s.queue.length
      result.discarded += lost
      s.discarded += lost
      s.queue.length = 0
      console.warn(
        `[notifications] email delivery was switched off mid-drain — discarded ${lost} queued mail(s).` +
          " Every one of them is still in its recipient's in-app inbox.",
      )
      break
    }
    if (r.ok) {
      s.queue.shift()
      s.sent++
      result.sent++
      s.consecutiveFailures = 0
      // `delivered` and `ok` are not the same answer: a recipient with no
      // address is `ok` and nothing left the building, so the row must stay
      // unread. Only a message a provider ACCEPTED files it read.
      if (r.delivered && item.markReadId) await markReadOnDelivery(item.markReadId)
    } else {
      s.queue.shift()
      s.failed++
      result.failed++
      s.consecutiveFailures++
      item.attempts++
      if (item.attempts < MAX_SEND_ATTEMPTS) {
        // Back of the queue, not the front: everything else queued deserves an
        // attempt before this one gets a second. Pushed unconditionally even at
        // MAX_QUEUED — it was accepted once already, and refusing it here would
        // be the silent loss this whole change is about.
        s.queue.push(item)
        result.requeued++
      } else {
        s.abandoned++
        result.abandoned++
        console.error(
          `[notifications] giving up on the "${item.n.kind}" email for user ${item.userId} after` +
            ` ${item.attempts} attempts (last error: ${r.error}). The notification itself is unread in their` +
            ' in-app inbox — the email was lost, the notification was not.',
        )
      }
      if (s.consecutiveFailures >= BREAKER_AFTER) {
        s.pausedUntil = Date.now() + BREAKER_COOLDOWN_MS
        result.paused = true
        console.error(
          `[notifications] ${s.consecutiveFailures} sends failed in a row — pausing delivery for` +
            ` ${Math.round(BREAKER_COOLDOWN_MS / 60_000)} minutes. ${s.queue.length} mail(s) still queued;` +
            ' they will be attempted after the pause, or dropped if the queue fills first.',
        )
        break
      }
    }
  }

  result.remaining = s.queue.length
  return result
}

export interface NotificationMailStats {
  queued: number
  /** How many the queue may hold before it starts refusing at the door, so a
   *  reader can say "412 of 500" instead of picking its own idea of "a lot". */
  capacity: number
  /** How long the oldest thing on the queue has been waiting. THE number for a
   *  stuck outbox: depth alone cannot tell a busy minute from a queue that has
   *  not moved since Tuesday, and this can. Null when the queue is empty. */
  oldestQueuedMs: number | null
  sent: number
  failed: number
  abandoned: number
  dropped: number
  discarded: number
  /** The breaker is open — the drain is short-circuiting and nothing is being
   *  attempted until this reaches zero. */
  pausedForMs: number
}

/** What the outbox is doing, for verification and for anything that wants to
 *  report on it.
 *
 *  IT HAS A READER NOW, and that was the whole problem with it. The queue is the
 *  one part of notification delivery with no user-visible surface at all: a
 *  breaker that opens takes the drain out of the loop, the job's own log line
 *  went quiet in exactly that state, and a queue stuck at 200 mails reported
 *  nothing to anybody. server/alerts.ts turns these numbers into the alerts
 *  /observability draws, so the outbox is now something a person can see rather
 *  than something you have to already suspect to go looking for.
 *
 *  PROCESS-LOCAL, like `unhealthyJobs()` and for the same reason: the queue is
 *  in this process's memory (that is why the job is `perInstance`), so this
 *  describes the instance that answered the request. */
export function notificationMailStats(): NotificationMailStats {
  const s = state()
  const oldest = s.queue.reduce<number | null>((min, m) => (min === null || m.queuedAt < min ? m.queuedAt : min), null)
  return {
    queued: s.queue.length,
    capacity: MAX_QUEUED,
    oldestQueuedMs: oldest === null ? null : Math.max(0, Date.now() - oldest),
    sent: s.sent,
    failed: s.failed,
    abandoned: s.abandoned,
    dropped: s.dropped,
    discarded: s.discarded,
    pausedForMs: Math.max(0, s.pausedUntil - Date.now()),
  }
}

// The scheduler owns the timing — this module owns the work. `perInstance`
// because the queue is in THIS process's memory: a Redis lease would let one
// instance win the tick and leave every other instance's queue undrained
// forever. See JobSpec.perInstance.
registerJob({
  name: 'notification-mail',
  // Fast enough that a mention email is not noticeably late, slow enough that
  // an empty queue costs nothing. Longer than DRAIN_BUDGET_MS so a full drain
  // does not routinely collide with the next tick.
  everyMs: 30_000,
  // Let a deploy settle before this instance starts mailing anyone. Nothing
  // else may be derived from this number, and nothing else may be tuned against
  // it: it is a settle window, not a coordination point.
  firstRunDelayMs: 20_000,
  // The outside bound for ONE drain, and it is read now — `unhealthyJobs()`
  // calls a run that outlives it hung rather than slow, which is the only way a
  // drain that never returns becomes visible to anybody. `perInstance` skips
  // the Redis lease, so this is no longer only a lease TTL. The real worst case
  // is one full budget (DRAIN_BUDGET_MS) plus the send already in flight when
  // the budget expires (EMAIL_SEND_TIMEOUT_MS); two minutes is that, rounded up.
  maxRunMs: 2 * 60_000,
  perInstance: true,
  run: async () => {
    const r = await drainNotificationMail()
    // "Nothing to do" means NOTHING TO DO — an empty queue and a closed breaker.
    // It used to also mean "the breaker is open and N mails are going nowhere",
    // because that pass sends nothing, fails nothing and discards nothing, so a
    // stuck outbox logged the same quiet line as a healthy idle one. A pass that
    // left mail on the queue has something to say, and the two states it can be
    // in — paused, or simply out of budget — are not the same news.
    if (!r.sent && !r.failed && !r.discarded && !r.remaining) return null
    if (!r.sent && !r.failed && !r.discarded) {
      const { oldestQueuedMs } = notificationMailStats()
      const waited = oldestQueuedMs === null ? '' : `, oldest waiting ${Math.round(oldestQueuedMs / 60_000)} minute(s)`
      return r.paused
        ? `NOTHING SENT — delivery is paused after repeated failures; ${r.remaining} mail(s) still queued${waited}.` +
            ' Every one of them is still in its recipient\'s in-app inbox.'
        : `nothing sent this pass — ${r.remaining} mail(s) still queued${waited}`
    }
    return (
      `sent ${r.sent}, failed ${r.failed}` +
      (r.requeued ? `, requeued ${r.requeued} for the next pass` : '') +
      (r.abandoned ? `, GAVE UP on ${r.abandoned} after ${MAX_SEND_ATTEMPTS} attempts (still unread in-app)` : '') +
      (r.discarded ? `, discarded ${r.discarded} (delivery off)` : '') +
      (r.remaining ? `, ${r.remaining} still queued` : '') +
      (r.dropped ? `, ${r.dropped} dropped since boot` : '') +
      (r.paused ? ' — delivery is paused after repeated failures' : '')
    )
  },
})

// ── The email itself ─────────────────────────────────────────────────────────

/** Absolute URL for an in-app path, or null when this deployment has no
 *  verified domain to build one from. */
async function appUrl(path: string): Promise<string | null> {
  const base = await instanceBaseUrl()
  if (!base) return null
  return `${base}${path.startsWith('/') ? '' : '/'}${path}`
}

export const NOTIFY_SETTINGS_PATH = '/settings/notifications'

/** Compose one notification email. The shared house style it is built from —
 *  `emailShell`, `emailButton`, the escaping — lives in server/email.ts, which
 *  is what the digest also builds on; this function is the notification-shaped
 *  arrangement of it and has one caller, below. */
function notificationEmailParts(
  n: NotificationInput,
  links: { deepLink: string | null; settingsUrl: string | null; whatIsThis: string },
): { subject: string; html: string; text: string } {
  const body = (n.body ?? '').trim()
  const cta = links.deepLink
    ? emailButton(links.deepLink, 'Open in Talaria')
    : // No verified instance domain, so no absolute link exists. Say where it
      // is rather than shipping a dead <a href="/boards/…"> that resolves
      // against the mail client.
      n.href
        ? `<p style="margin:24px 0;font-size:13px;color:#8a8a84">Open Talaria and go to <code>${emailEscape(n.href)}</code>.</p>`
        : ''
  const footer =
    `${emailEscape(links.whatIsThis)}` +
    (links.settingsUrl
      ? ` · <a href="${emailEscape(links.settingsUrl)}" style="color:#8a8a84">change what Talaria emails you</a>`
      : ` · change what Talaria emails you under Settings → Notifications`)
  return {
    subject: n.title,
    html: emailShell(
      emailEscape(n.title),
      (body ? `<p style="white-space:pre-wrap">${emailEscape(body)}</p>` : '') + cta,
      footer,
    ),
    text: [n.title, body, links.deepLink ?? n.href ?? '', '', `${links.whatIsThis}. Change what Talaria emails you: ${links.settingsUrl ?? 'Settings → Notifications'}`]
      .filter(Boolean)
      .join('\n\n'),
  }
}

/** Send one notification as mail. Resolves the recipient, builds the deep link,
 *  and REPORTS every reason it couldn't — a person who turned email on and
 *  hears nothing must be able to find out why from the server log.
 *
 *  Returns the outcome rather than only logging it: the drain counts failures
 *  to decide when the transport is dead, and a function that only writes to the
 *  log cannot be part of that decision. Never throws — a send is the last thing
 *  that should be able to take down the loop draining the queue.
 *
 *  A recipient with no address is `ok`. It is not a delivery failure and must
 *  not count toward the breaker: no number of address-less users means the mail
 *  server is broken.
 *
 *  `blocked` is the third outcome and it is neither: the instance switch went
 *  off. The drain stops on it rather than counting it, because "sent" must
 *  never name a mail that was refused at the gate.
 *
 *  `delivered` separates "nothing went wrong" from "a provider took it". Only
 *  the second may file the in-app row read, and the address-less recipient above
 *  is exactly the case where the two answers differ. */
async function sendNotificationEmail(
  userId: string,
  n: NotificationInput,
): Promise<GatedSendResult & { delivered?: boolean }> {
  try {
    const sql = await db()
    const rows = (await sql`select email from users where id = ${userId}`) as unknown as Array<{ email: string | null }>
    const to = rows[0]?.email?.trim()
    if (!to) {
      console.warn(`[notifications] no email address for user ${userId} — "${n.kind}" stays in-app only`)
      return { ok: true }
    }
    const deepLink = n.href ? await appUrl(n.href) : null
    if (n.href && !deepLink) {
      console.warn('[notifications] no verified instance domain — sending mail without a clickable link (Admin → Org → Domain)')
    }
    const settingsUrl = await appUrl(NOTIFY_SETTINGS_PATH)
    const parts = notificationEmailParts(n, {
      deepLink,
      settingsUrl,
      whatIsThis: 'Sent by Talaria because of your notification settings',
    })
    const r = await sendGatedMail({
      to,
      subject: parts.subject,
      html: parts.html,
      text: parts.text,
      unsubscribeUrl: settingsUrl,
      what: `notification "${n.kind}"`,
    })
    // A refusal by the master switch is not a transport failure and must not
    // count toward the breaker: the mail server is fine, the operator said no.
    if (!r.ok && !r.blocked) console.error(`[notifications] email to ${to} for "${n.kind}" failed: ${r.error}`)
    return r.ok ? { ...r, delivered: true } : r
  } catch (e) {
    // Building the mail failed — the recipient lookup, the base URL, anything.
    // Report it as a failed send rather than letting it escape into the drain.
    const error = e instanceof Error ? e.message : String(e)
    console.error(`[notifications] could not build the "${n.kind}" email for user ${userId}: ${error}`)
    return { ok: false, error }
  }
}

// ── The choke point ──────────────────────────────────────────────────────────

/** Write a notification and route it. The row always lands — it is the record,
 *  and the inbox and the digest both read it — so the preference decides
 *  DELIVERY: `in_app` badges the bell, `email` mails it and files the row read
 *  ONCE THE MAIL IS ACCEPTED, `both` does both and leaves it unread.
 *
 *  TWO gates, and both must open for mail: the instance switch
 *  (`getNotifyDelivery`) and then this user's route for this class.
 *
 *  Mail is QUEUED, never sent here. The caller is servicing somebody else's
 *  request; it must not wait on, or fail with, an SMTP timeout. Everything on
 *  this path is a database write on a connection it was going to use anyway
 *  plus an array push. */
export async function addNotification(userId: string, n: NotificationInput): Promise<void> {
  const cls = notifyClassOf(n.kind)
  // Both reads are forgiving (see `prefsBlob`, and `getNotifyDelivery` is
  // cached), and both fall back to the answer that sends LESS mail.
  const route = (await getNotifyPrefs(userId))[cls]
  const wantsMail = route === 'email' || route === 'both'
  const willMail = wantsMail && (await getNotifyDelivery().catch(() => DELIVERY_DEFAULT)).emailEnabled

  // THE ROW LANDS UNREAD, ALWAYS. `email` files it read afterwards so a class
  // you read in your mail does not also nag you with an unread count — but
  // "afterwards" is doing all the work in that sentence, and this line used to
  // read `route === 'email' && willMail ? new Date() : null`, filing it read at
  // INSERT time on the strength of an intention to send.
  //
  // Everything between that intention and an inbox is allowed to fail. The queue
  // can refuse it at MAX_QUEUED. The drain can discard it when an admin throws
  // the switch. The provider can time out, three times, and the mail can be
  // abandoned. Every one of those leaves the person with no email AND a
  // notification that was born read: not in the bell count, not in the unread
  // list, findable only by scrolling an inbox they have no reason to open. The
  // notification is the record; the email is a delivery of it, and a delivery
  // that did not happen must not mark the record consumed.
  //
  // So the id travels with the queued mail and the read-mark happens in the
  // drain, once a provider has accepted the message. See `markReadOnDelivery`.
  const sql = await db()
  const rows = (await sql`
    insert into notifications (user_id, kind, title, body, href, read_at)
    values (${userId}, ${n.kind}, ${n.title}, ${n.body ?? ''}, ${n.href ?? ''}, null)
    returning id
  `) as unknown as Array<{ id: string }>

  // PUBLISH, NOW THAT THERE IS SOMETHING TO PUBLISH. This is the row's second
  // job after existing: every surface that shows notifications — the bell, the
  // brief's 'worth knowing' section — learns the row landed the moment it does,
  // instead of discovering it on the next poll. Two events, two topics in one:
  // the notification event is the bell's (id-shaped; it refetches through its
  // ordinary route), and the brief nudge clears this person's sweep throttle so
  // the brief's own read picks the change up live. Both detached — the row is
  // written, which is the part the caller needed, and neither fan-out may cost
  // the request it rode in on (rule 1 above, applied to pub/sub).
  const notificationId = rows[0]?.id
  if (notificationId) publishUser(userId, { type: 'notification', notificationId })
  void markBriefStale([userId]).catch((e: unknown) =>
    console.error(`[notifications] brief nudge failed for ${userId}:`, e),
  )
  // `both` deliberately passes null: that route wants the in-app copy to stay
  // unread whatever the mail does.
  if (willMail) enqueueMail(userId, n, route === 'email' ? (rows[0]?.id ?? null) : null)
}

// ── Reads ────────────────────────────────────────────────────────────────────

export async function listNotifications(userId: string, limit = 50): Promise<Notification[]> {
  const sql = await db()
  return (await sql`
    select id, kind, title, body, href, read_at as "readAt", created_at as "createdAt"
    from notifications where user_id = ${userId}
    order by created_at desc limit ${limit}
  `) as unknown as Notification[]
}

export async function unreadCount(userId: string): Promise<number> {
  const sql = await db()
  const rows = await sql`select count(*)::int as n from notifications where user_id = ${userId} and read_at is null`
  return (rows[0] as { n: number }).n
}

/** Mark specific notifications read, or all of the user's when ids is omitted.
 *
 *  Also nudges the brief, detached: unread notifications are one of the brief's
 *  sources, so a read here is a line resolving there — and without the nudge
 *  the person's own brief would keep naming a notification they just handled
 *  until the next scheduled sweep. */
export async function markNotificationsRead(userId: string, ids?: string[]): Promise<void> {
  const sql = await db()
  if (ids && ids.length > 0) {
    await sql`update notifications set read_at = now() where user_id = ${userId} and id = any(${ids}) and read_at is null`
  } else {
    await sql`update notifications set read_at = now() where user_id = ${userId} and read_at is null`
  }
  void markBriefStale([userId]).catch(() => {})
}
