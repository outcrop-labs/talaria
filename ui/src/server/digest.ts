// The daily digest, and the approval SLA. Two scheduled jobs, one purpose:
// turn "a human has to look at this" from a hope into a deadline.
//
// WHY THIS FILE EXISTS
//   Talaria's guardrail model parks work on people constantly — an outbound
//   email an agent drafted, a build plan it will not start without sign-off,
//   finished work waiting for QA, a repo that does not exist yet. Every one of
//   those is correct behaviour and every one of them is a hole in the floor:
//   the agent stops, the person is not in the app, and NOTHING makes a noise.
//   The gate is only a gate if something enforces the time limit on it.
//
//   Job 1 — the digest — is the once-a-day floor: whatever else happened, you
//   get one message that says how much is on you. Job 2 — escalation — is the
//   ceiling: an individual approval that ages past patience nags its owner, and
//   then goes over their head, so "on holiday" costs hours instead of a week.
//
// THE RULES THAT SHAPE THE DIGEST
//   1. NOTHING WAITING, NOTHING SENT. An empty daily email is how a sender gets
//      filtered to trash, and the day the queue is empty is exactly the day the
//      digest has nothing to earn. `buildDigest` returns null and the job skips
//      the recipient entirely.
//   2. THE SUBJECT CARRIES THE CONTENT. "3 approvals waiting, 2 in review, 1
//      blocked" tells a person on a phone whether to open it. "You have
//      updates" does not, and after a week of it nobody opens either.
//   3. IT COUNTS WHAT THE SCREEN COUNTS. The queue numbers come from
//      `homeQueues()` — the same pass Home renders — and the approvals from the
//      one census in server/approvals.ts. A digest that disagrees with the app
//      is worse than no digest, because now you distrust both.
//   4. IT DISCLOSES NOTHING THE APP WOULD NOT. One email, one visibility model:
//      every line of it is something the recipient could already open in
//      Talaria. The queues are board-membership scoped by `homeQueues`; the
//      approvals are scoped by `approvalAudience`, which is the authority the
//      DECISION ROUTE enforces — board editors for board-anchored approvals (a
//      subset of board membership), the owner alone for a personal confirm-send,
//      the admins for org-scoped things an admin can already open. There is no
//      "you cannot see this but you should know about it" tier, because the
//      version of this file that had one mailed a person's private email subject
//      to an admin who could not have approved it anyway.
//
// ASSUMPTIONS, STATED
//   · NO TIME ZONE PER USER. The `users` table has no timezone column and this
//     agent may not add a migration this run, so the send hour is ORG-WIDE:
//     app_settings `digest_config` = { hour, timeZone, … }, defaulting to 08:00
//     in TZ (or UTC). When a per-user zone lands, `recipientZone()` below is the
//     one function that has to change.
//   · THE OPT-OUT IS ONE RESERVED KEY. The notification prefs table
//     (lib/notifications.ts) routes CLASSES, and a digest is not a class, so the
//     switch rides in the same jsonb blob under `digest`. Settings →
//     Notifications writes it; `digestEnabled` there decides, and this file's
//     `digestOptedIn` is that call and nothing else.
import { getSetting, setSetting } from './audit'
import { emailButton, emailEscape, emailShell } from './email'
import { homeQueues, type WorkItem } from './home'
import { instanceBaseUrl } from './instance'
// `sendGatedMail`, not `sendEmail`. THE INSTANCE MASTER SWITCH LIVES ON THAT
// PATH AND ONLY ON THAT PATH. This file used to call the transport directly, so
// an admin who turned email off still had a digest mailed to every user in the
// workspace every morning — a control that is named "email delivery", is
// audited, and did not do the thing its name says. The fix was deliberately NOT
// a second `if (enabled)` here: two copies of a rule is how the first one
// drifts. scripts/check-invariants.mjs now fails the build on a new caller of
// `sendEmail`, so the third bypass cannot be written.
import { addNotification, NOTIFY_SETTINGS_PATH, sendGatedMail } from './notifications'
import {
  KIND_LABEL,
  approvalCensus,
  mayDecide,
  sweepUnannounced,
  type ApprovalCensus,
  type PendingApproval,
} from './approvals'
import { registerJob } from './scheduler'
import { db } from './db/pg'
import { digestEnabled } from '@/lib/notifications'

// ── Configuration ────────────────────────────────────────────────────────────

export interface DigestConfig {
  /** Local hour (0–23) the digest is sent at. */
  hour: number
  /** IANA zone the hour is read in. */
  timeZone: string
  /** How long an approval may sit before its owner is nagged. */
  nagAfterMinutes: number
  /** How long before it goes over the owner's head to the admins. */
  escalateAfterMinutes: number
  /** How many items of each kind the email lists before "and N more". */
  listLimit: number
}

const CONFIG_KEY = 'digest_config'
const DIGEST_STATE_KEY = 'digest_state'
const ESCALATION_STATE_KEY = 'approval_escalation_state'

const DEFAULTS: DigestConfig = {
  hour: 8,
  timeZone: process.env.TZ || 'UTC',
  // Four hours: long enough that an approval raised mid-morning is not a ping
  // before lunch, short enough that same-day work stays same-day.
  nagAfterMinutes: 4 * 60,
  // A full day. Past this the owner is demonstrably not coming, and an agent
  // that has been stopped for 24 hours is a cost, not an inconvenience.
  escalateAfterMinutes: 24 * 60,
  listLimit: 5,
}

export async function digestConfig(): Promise<DigestConfig> {
  const stored = await getSetting<Partial<DigestConfig>>(CONFIG_KEY, {})
  const hour = Number.isInteger(stored.hour) && stored.hour! >= 0 && stored.hour! <= 23 ? stored.hour! : DEFAULTS.hour
  const positive = (v: unknown, fallback: number) => (typeof v === 'number' && v > 0 ? v : fallback)
  return {
    hour,
    timeZone: typeof stored.timeZone === 'string' && stored.timeZone.trim() ? stored.timeZone.trim() : DEFAULTS.timeZone,
    nagAfterMinutes: positive(stored.nagAfterMinutes, DEFAULTS.nagAfterMinutes),
    escalateAfterMinutes: positive(stored.escalateAfterMinutes, DEFAULTS.escalateAfterMinutes),
    listLimit: positive(stored.listLimit, DEFAULTS.listLimit),
  }
}

// ── Recipients ───────────────────────────────────────────────────────────────

interface Recipient {
  id: string
  email: string
  name: string | null
  role: 'admin' | 'member'
  prefs: unknown
}

async function recipients(): Promise<Recipient[]> {
  const sql = await db()
  return (await sql`
    select id, email, name, role, notify_prefs as prefs
    from users where email is not null and length(trim(email)) > 0
    order by created_at asc
  `) as unknown as Recipient[]
}

/** Does this person want the digest?
 *
 *  The rule itself is `digestEnabled` in lib/notifications.ts — explicit switch
 *  first, derived from the routing table when nobody has touched it — and it
 *  lives there rather than here for one reason: Settings → Notifications draws
 *  the switch from the same function. This job and that screen giving different
 *  answers would mean a person reading "Daily digest — On" and receiving
 *  nothing, with no way to tell which half was lying.
 *
 *  Settings now WRITES `notify_prefs.digest` (PATCH /api/notifications →
 *  `setNotifySettings`), so the off switch is real and reaches this line. */
export function digestOptedIn(prefs: unknown): boolean {
  return digestEnabled(prefs)
}

/** The zone the send hour is read in for this person. Org-wide today; the one
 *  place to change when the user record grows a zone. */
function recipientZone(_user: Recipient, config: DigestConfig): string {
  return config.timeZone
}

/** Local hour and calendar date in a zone. An invalid zone must not take the
 *  whole job down — a typo in a settings row would otherwise silence the
 *  digest for everyone, which is precisely the failure mode this file exists
 *  to end — so it falls back to UTC and says so. */
export function localMoment(timeZone: string, at: Date): { hour: number; date: string; zone: string } {
  const read = (zone: string) => {
    const parts = new Intl.DateTimeFormat('en-CA', {
      timeZone: zone,
      hour12: false,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
    }).formatToParts(at)
    const get = (type: string) => parts.find((p) => p.type === type)?.value ?? ''
    // 'en-CA' + hour12:false yields 24 for midnight in some ICU versions.
    const hour = Number(get('hour')) % 24
    return { hour, date: `${get('year')}-${get('month')}-${get('day')}`, zone }
  }
  try {
    return read(timeZone)
  } catch {
    console.warn(`[digest] unknown time zone "${timeZone}" in ${CONFIG_KEY} — falling back to UTC`)
    return read('UTC')
  }
}

// ── The digest content ───────────────────────────────────────────────────────

export interface DigestContent {
  userId: string
  email: string
  /** The subject line. Countful by construction — see `headline`. */
  subject: string
  approvals: PendingApproval[]
  review: WorkItem[]
  blocked: WorkItem[]
  triage: WorkItem[]
  /** A kind of approval that could not be read this pass, if any. The email
   *  says so rather than quietly under-counting. */
  incomplete: boolean
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

/** The content bar: a person who reads only this knows whether to open it. */
function headline(parts: { approvals: number; review: number; blocked: number; triage: number }): string | null {
  const phrases: string[] = []
  if (parts.approvals) phrases.push(`${plural(parts.approvals, 'item')} need${parts.approvals === 1 ? 's' : ''} your approval`)
  if (parts.review) phrases.push(`${plural(parts.review, 'ticket')} in review`)
  if (parts.blocked) phrases.push(`${plural(parts.blocked, 'ticket')} blocked`)
  if (parts.triage) phrases.push(`${plural(parts.triage, 'ticket')} to triage`)
  if (!phrases.length) return null
  return phrases.join(', ')
}

/** Everything waiting on one person, or null when the honest answer is nothing.
 *
 *  Null is a first-class result, not a failure: rule 1 at the top of the file.
 *  A caller that treats null as "send an empty one" has broken the digest. */
export async function buildDigest(
  // No `role`. It used to be here, and dropping it is the fix: the digest is
  // assembled from what this USER can reach, and there is no admin bypass to
  // pass in. Anything that wants one has to change `approvalAudience`, where
  // the question is asked once and answered against the decision routes.
  user: { id: string; email: string },
  census?: ApprovalCensus,
): Promise<DigestContent | null> {
  const [queues, all] = await Promise.all([homeQueues(user.id), census ? Promise.resolve(census) : approvalCensus()])

  // ONE VISIBILITY MODEL (rule 4). `homeQueues` is board-membership scoped, and
  // this filter is the decision route's own authority — for the board-anchored
  // kinds that is board EDITORS, a subset of the same membership. The role is
  // deliberately not consulted: being an admin is not a way to read a board you
  // were never added to, and the predicate that used to say "admins own the
  // unowned" is what put a stranger's ticket, and a colleague's private email
  // subject, into an admin's inbox.
  const approvals = all.approvals.filter((a) => mayDecide(all, a, user.id))

  // The review queue is BOTH an approval and a Home queue. It is counted once,
  // as "in review", and the approval census entry for the same ticket is
  // dropped here — a digest that says "4 need your approval, 3 in review" for
  // three tickets and one email is a digest nobody can act on.
  const approvalsOutsideReview = approvals.filter((a) => a.kind !== 'ticket_review')

  const counts = {
    approvals: approvalsOutsideReview.length,
    review: queues.review.count,
    blocked: queues.blocked.count,
    triage: queues.triage.count,
  }
  const bar = headline(counts)
  if (!bar) return null

  return {
    userId: user.id,
    email: user.email,
    subject: `Talaria: ${bar}`,
    approvals: approvalsOutsideReview,
    review: queues.review.items,
    blocked: queues.blocked.items,
    triage: queues.triage.items,
    incomplete: all.failedKinds.length > 0,
  }
}

// ── Rendering ────────────────────────────────────────────────────────────────

const listHtml = (rows: Array<{ label: string; sub: string; href: string | null }>): string =>
  `<ul style="margin:6px 0 0;padding-left:18px">${rows
    .map(
      (r) =>
        `<li style="margin:0 0 6px">${
          r.href ? `<a href="${emailEscape(r.href)}" style="color:#1a1a18">${emailEscape(r.label)}</a>` : emailEscape(r.label)
        }${r.sub ? `<br><span style="color:#8a8a84;font-size:12px">${emailEscape(r.sub)}</span>` : ''}</li>`,
    )
    .join('')}</ul>`

function section(title: string, rows: Array<{ label: string; sub: string; href: string | null }>, total: number): string {
  if (!rows.length) return ''
  const more = total > rows.length ? `<p style="margin:6px 0 0;font-size:12px;color:#8a8a84">and ${total - rows.length} more</p>` : ''
  return `<h2 style="margin:22px 0 0;font-size:13px;text-transform:uppercase;letter-spacing:0.06em;color:#8a8a84">${emailEscape(title)}</h2>${listHtml(rows)}${more}`
}

/** The opt-out, spelled as an instruction, for a deployment that has no
 *  verified domain to build a link out of.
 *
 *  The version this replaced degraded to "change what Talaria emails you under
 *  Settings → Notifications" — a description of a screen, not a way off the
 *  list. It did not say which control, or that one switch stops THIS mail, and
 *  the plain-text part of the digest carried no opt-out line at all. An email
 *  whose only exit is "go and look for it" is an email people report as spam
 *  instead, and one report costs the sending domain more than a year of
 *  digests earns it. */
const DIGEST_OPT_OUT_STEPS = 'To stop this daily email, open Talaria and set Settings → Notifications → Daily digest to Off.'

export function renderDigest(
  content: DigestContent,
  links: { base: string | null; listLimit: number },
): { subject: string; html: string; text: string; unsubscribeUrl: string | null } {
  const url = (path: string): string | null => (links.base ? `${links.base}${path.startsWith('/') ? '' : '/'}${path}` : null)
  const cap = <T,>(rows: T[]): T[] => rows.slice(0, links.listLimit)
  const ticketRow = (t: WorkItem) => ({
    label: `${t.ticketRef ? `${t.ticketRef} · ` : ''}${t.title}`,
    sub: t.board,
    href: url(`/boards/${t.boardId}/${t.id}`),
  })

  const body =
    `<p style="margin:0">${emailEscape(content.subject.replace(/^Talaria: /, ''))}.</p>` +
    section(
      'Waiting on your approval',
      cap(content.approvals).map((a) => ({ label: a.title, sub: a.detail, href: url(a.href) })),
      content.approvals.length,
    ) +
    section('In review', cap(content.review).map(ticketRow), content.review.length) +
    section('Blocked', cap(content.blocked).map(ticketRow), content.blocked.length) +
    section('To triage', cap(content.triage).map(ticketRow), content.triage.length) +
    (content.incomplete
      ? `<p style="margin:18px 0 0;font-size:12px;color:#8a8a84">One source could not be read when this was built, so an approval may be missing from the list above. Open Talaria to see the live queue.</p>`
      : '') +
    (links.base ? emailButton(`${links.base}/`, 'Open Talaria') : '')

  const settings = url(NOTIFY_SETTINGS_PATH)
  const footer =
    'Your daily Talaria digest — sent only when something is waiting on you' +
    (settings
      ? ` · <a href="${emailEscape(settings)}" style="color:#8a8a84">turn the daily digest off</a>`
      : ` · ${emailEscape(DIGEST_OPT_OUT_STEPS)}`)

  const textLines = [
    content.subject,
    '',
    ...(content.approvals.length
      ? ['WAITING ON YOUR APPROVAL', ...cap(content.approvals).map((a) => `- ${a.title} — ${a.detail}`), '']
      : []),
    ...(content.review.length
      ? ['IN REVIEW', ...cap(content.review).map((t) => `- ${t.ticketRef ? `${t.ticketRef} · ` : ''}${t.title} (${t.board})`), '']
      : []),
    ...(content.blocked.length
      ? ['BLOCKED', ...cap(content.blocked).map((t) => `- ${t.ticketRef ? `${t.ticketRef} · ` : ''}${t.title} (${t.board})`), '']
      : []),
    ...(content.triage.length
      ? ['TO TRIAGE', ...cap(content.triage).map((t) => `- ${t.ticketRef ? `${t.ticketRef} · ` : ''}${t.title} (${t.board})`), '']
      : []),
    links.base ?? 'Open Talaria to act on these.',
    '',
    '--',
    'Your daily Talaria digest, sent only when something is waiting on you.',
    // The opt-out is in the TEXT part too. It was not, and a mail client that
    // renders text/plain (or a person reading the raw source) had no way off
    // the list at all.
    settings ? `Turn the daily digest off: ${settings}` : DIGEST_OPT_OUT_STEPS,
  ]

  return {
    subject: content.subject,
    html: emailShell(emailEscape(content.subject), body, footer),
    text: textLines.join('\n'),
    // `List-Unsubscribe` for the send. Null on a deployment with no verified
    // domain: a header pointing at a URL that does not resolve is worse than no
    // header, and the footer above carries the instruction instead.
    unsubscribeUrl: settings,
  }
}

// ── The digest job ───────────────────────────────────────────────────────────

interface DigestState {
  /** userId → the LOCAL calendar date they were last sent a digest for. */
  sentOn: Record<string, string>
}

/** Only send inside a window that starts at the configured hour. Without it, an
 *  instance that boots at 22:00 would see "hour >= 8, nothing sent today" and
 *  mail the entire workspace at ten at night.
 *
 *  The window deliberately does NOT wrap past midnight. A `hour: 23` workspace
 *  gets a one-hour window rather than 23:00–01:00, because the "already sent"
 *  mark is keyed by LOCAL CALENDAR DATE: a wrapping window would send at 23:10
 *  for date D and again at 00:10 for date D+1. One shorter window beats two
 *  emails. */
const SEND_WINDOW_HOURS = 2

export interface DigestRunResult {
  considered: number
  sent: number
  skippedNothingWaiting: number
  skippedOptedOut: number
  skippedOutsideWindow: number
  /** Built, and then refused by the instance master switch. Its own counter and
   *  NOT `failed`: nothing was attempted, no transport was involved, and the
   *  operator meant it. Counting it as a failure would put a red job in
   *  /observability for a switch working exactly as designed. */
  skippedDeliveryOff: number
  failed: number
  /** Mailed, and then the ledger write that records it FAILED. At most one per
   *  pass, because the pass stops there — see `runDigest`. Its own counter and
   *  not `failed`: the mail went, the memory of it did not, and the person may
   *  be mailed a second time tomorrow morning. */
  unrecorded: number
  /** True when the pass ended early rather than at the end of the recipient
   *  list, because it could no longer record what it had done. The next tick
   *  inside the send window picks up where it stopped. */
  stoppedEarly: boolean
}

/** One pass of the digest. Exported so a verification run can drive it directly
 *  and so the scheduler is not the only thing that can prove it works.
 *
 *  THE LEDGER IS WRITTEN AS THE PASS GOES, NOT AFTER IT. `sentOn` is what stops
 *  a person being mailed twice in one day, and it used to be accumulated in this
 *  local map and written ONCE, after the recipient loop. Every way a pass can
 *  end early — the process is killed mid-deploy, the container is OOM-killed,
 *  SIGTERM lands between two sends, the box loses power — threw away every mark
 *  the pass had made, and the next tick (fifteen minutes later, still inside the
 *  send window) mailed the whole workspace again. The mails had gone; only the
 *  memory of them had not.
 *
 *  RESUMABLE WAS THE ALTERNATIVE AND IT IS THE SAME THING. There is no cursor to
 *  resume from: `sentOn` IS the cursor, keyed per user, and a pass that persists
 *  each mark as it makes it is already resumable from wherever it stopped. So
 *  the fix is to write the mark, not to invent a second structure that would
 *  need writing just as durably.
 *
 *  ONE SETTINGS WRITE PER MAIL. Next to an SMTP round trip that is nothing, and
 *  it only happens on the send path — the skip marks (opted out, nothing
 *  waiting) still ride to the end of the pass, because losing one of those costs
 *  a recomputation and not a duplicate email.
 *
 *  THE RESIDUAL WINDOW IS ONE MAIL WIDE, and it is stated rather than hidden: a
 *  crash between the provider accepting a message and the ledger write landing
 *  re-mails that ONE person. Closing it entirely needs the mark and the send in
 *  one transaction, which no mail provider offers. */
export async function runDigest(now = new Date()): Promise<DigestRunResult> {
  const config = await digestConfig()
  const people = await recipients()
  const state = await getSetting<DigestState>(DIGEST_STATE_KEY, { sentOn: {} })
  const sentOn: Record<string, string> = { ...(state.sentOn ?? {}) }
  const result: DigestRunResult = {
    considered: people.length,
    sent: 0,
    skippedNothingWaiting: 0,
    skippedOptedOut: 0,
    skippedOutsideWindow: 0,
    skippedDeliveryOff: 0,
    failed: 0,
    unrecorded: 0,
    stoppedEarly: false,
  }
  if (!people.length) return result

  // Keep the map from growing past the people who still exist. Computed once
  // and applied on every write, so a mid-pass save prunes exactly as the final
  // one does — two writers of one blob that disagreed about its shape would be
  // a way for the last write to resurrect a departed user's mark.
  const live = new Set(people.map((p) => p.id))
  const persist = () =>
    setSetting(DIGEST_STATE_KEY, {
      sentOn: Object.fromEntries(Object.entries(sentOn).filter(([id]) => live.has(id))),
    } satisfies DigestState)

  const base = await instanceBaseUrl()
  // One census — and one audience resolution — for the whole pass. Every
  // recipient filters the same list against the same authority, which is both
  // cheaper and the only way two people can never be told different things
  // about the same approval.
  const census = await approvalCensus()

  for (const person of people) {
    const { hour, date } = localMoment(recipientZone(person, config), now)
    const inWindow = hour >= config.hour && hour < config.hour + SEND_WINDOW_HOURS
    if (!inWindow || sentOn[person.id] === date) {
      if (!inWindow) result.skippedOutsideWindow++
      continue
    }
    if (!digestOptedIn(person.prefs)) {
      result.skippedOptedOut++
      // Marked as handled for the day so a person who opts out is not
      // reconsidered on every 15-minute tick inside the window.
      sentOn[person.id] = date
      continue
    }
    try {
      const content = await buildDigest({ id: person.id, email: person.email }, census)
      if (!content) {
        result.skippedNothingWaiting++
        sentOn[person.id] = date
        continue
      }
      const mail = renderDigest(content, { base, listLimit: config.listLimit })
      const sent = await sendGatedMail({
        to: person.email,
        subject: mail.subject,
        html: mail.html,
        text: mail.text,
        unsubscribeUrl: mail.unsubscribeUrl,
        what: 'daily digest',
      })
      if (sent.ok) {
        result.sent++
        sentOn[person.id] = date
        // The mark is written NOW, before the next recipient is even considered.
        try {
          await persist()
        } catch (e) {
          // The mail went and we cannot record that it did. Everything already
          // marked is on disk (that is the point of writing as we go), so the
          // damage is bounded to this one person — and it stays bounded only if
          // the pass stops here: carrying on would mail the rest of the
          // workspace against a ledger we already know we cannot write, which is
          // precisely the "one crash re-mails everybody" this change removes.
          // The next tick is fifteen minutes away and still inside the window.
          result.unrecorded++
          result.stoppedEarly = true
          console.error(
            `[digest] mailed ${person.email} but could not record it — stopping this pass so the rest of the` +
              ' workspace is not mailed against a ledger that cannot be written. They may be mailed again:',
            e,
          )
          return result
        }
      } else if (sent.blocked) {
        // The master switch is off. NOT marked sent — if an admin turns mail
        // back on later in the same send window, that day's digest still goes —
        // and not counted as a failure, because nothing failed.
        result.skippedDeliveryOff++
      } else {
        // NOT marked sent: a provider outage should retry on the next tick
        // inside the window, not silently cost someone their digest.
        result.failed++
        console.error(`[digest] could not send to ${person.email}: ${sent.error}`)
      }
    } catch (e) {
      result.failed++
      console.error(`[digest] building the digest for ${person.email} failed:`, e)
    }
  }

  // The skip marks (opted out, nothing waiting) and the prune. The sends are
  // already on disk; this is the cheap half, and losing it to a crash costs a
  // recomputation rather than a duplicate email — which is why it is the half
  // that is still allowed to wait until the end.
  try {
    await persist()
  } catch (e) {
    console.error('[digest] could not record the skips for this pass — they will be recomputed on the next tick:', e)
  }
  return result
}

registerJob({
  name: 'daily-digest',
  // Not hourly: the send window is anchored to a local hour and a 15-minute
  // tick keeps the mail near the top of that hour without a cron expression.
  everyMs: 15 * 60_000,
  // Let a deploy settle. A crash-looping instance must never reach a job that
  // mails the whole workspace.
  firstRunDelayMs: 60_000,
  maxRunMs: 10 * 60_000,
  run: async () => {
    const r = await runDigest()
    if (!r.sent && !r.failed && !r.skippedDeliveryOff && !r.unrecorded) return null
    const line =
      `sent ${r.sent} digest(s) — ${r.skippedNothingWaiting} had nothing waiting, ${r.skippedOptedOut} opted out, ${r.failed} failed` +
      (r.skippedDeliveryOff ? `, ${r.skippedDeliveryOff} held back because email delivery is off for this instance` : '')
    // A pass that could not write its own ledger is a FAILED run, not a run with
    // a footnote: the next tick may re-mail whoever it could not record, and the
    // only way an operator hears about that is the scheduler's error path.
    if (r.stoppedEarly) {
      throw new Error(
        `${line} — then STOPPED: the "already sent today" ledger could not be written after mailing ${r.unrecorded}` +
          ' recipient(s), who may be mailed again on the next tick. The rest of the workspace was not attempted.',
      )
    }
    return line
  },
})

// ── Approval announcement, nag and escalation ────────────────────────────────
//
// THREE STAGES, ONE AUDIENCE RULE
//   announce (on sight) → nag the owner (4h) → escalate (24h)
//
//   Every stage addresses `census.audience` for that approval, or a subset of
//   it, and NOTHING addresses anyone outside it. That set is the authority the
//   decision route enforces (see the table at the top of server/approvals.ts):
//   the owner alone for a personal confirm-send, the board's editors for a
//   workbench plan or a ticket sign-off, the admins for org-scoped things.
//
//   The version of this job that shipped before escalated with
//   `tell([...admins, ...owners])` for every kind. That put the SUBJECT LINE of
//   one person's private confirm-send — "Owen wants to send an email: <their
//   subject>" — in an org admin's notification list and in their mailbox, for an
//   approval `decideAction` would have refused that admin outright. Being an
//   admin is not entitlement to a colleague's mail, and "it is stuck" is not a
//   reason to forward the contents to somebody who cannot action it either.
//
//   Where an approval ages out with nobody who may be TOLD what it says, the
//   fact still needs raising — an agent is stopped indefinitely. So the FACT
//   audience of that same approval (`census.audience.get(key).fact`, resolved
//   once, by the resolver, not fetched here) gets a STALL REPORT: how long, what
//   kind, and why it could not be shown to anybody. No title, no detail, no
//   link. That is a thing the recipient can act on (add an admin to that board,
//   grant board access, assign the ticket); the content is not.
//
//   "NOBODY MAY BE TOLD" IS NOT "NOBODY CAN DECIDE", and this stage used to say
//   the second when it meant the first. An approval bounded to
//   `{ by: 'admin', onBoard }` has an empty CONTENT audience whenever no admin
//   is a member of that board — while every admin in the workspace can still
//   decide it. The report that went out said "It needs an admin, and this
//   workspace has none" to the admin who could have granted it. See
//   `whyUnreachable` and `stallTitle`.

// THE DEPLOY-DAY BOUND, which this stage was the last one in the codebase not
// to have. comms-decay caps a sweep at 8 and says why; the outreach sweep caps
// at 3. The nag and escalate loop below walked the ENTIRE census, so the
// first pass on an instance upgrading into this feature took every approval
// that had ever been left pending — a year of them on an old workspace — and
// notified everyone entitled to decide each one, at once, within ninety seconds
// of the deploy.
//
// Bounding it is safe precisely because nothing is dropped: `pendingApprovals`
// sorts oldest-first, so a bounded pass always raises the most overdue ones,
// and everything it did not reach is simply still due five minutes later. The
// only cost of the cap is latency on a backlog, and the thing it buys is that
// the backlog arrives as a queue instead of as a wall.
const ESCALATE_MAX_PER_PASS = 8

// The FIRST pass ever is tighter still, and it is a different question from the
// steady-state cap. Every later pass sees "what aged out since the last tick";
// the first sees "everything that has ever been pending on this instance", and
// it is the one pass whose size nobody can predict before running it. Two per
// pass drains ~24/hour — visible, reversible, and slow enough that an operator
// who does not like what they are seeing can turn email off before it becomes
// the workspace's morning.
//
// WHAT THIS CAP DOES NOT COVER, said out loud because the comment that used to
// sit above it claimed otherwise. It bounds the NAG and ESCALATE stages only.
// The ANNOUNCE stage (`sweepUnannounced`, in server/approvals.ts) is bounded by
// AGE and not by COUNT: everything older than `nagAfterMinutes` is retired to
// these two stages, which are capped — but everything YOUNGER is announced in
// one pass, however many there are. On a steady workspace that is right, and it
// is the latency the announce stage exists to remove. On a first deploy it means
// the blast radius is "every approval raised in the last four hours", which is
// small on any workspace this has run on and is not zero. A count cap belongs
// next to that sweep, not here; naming it is what stops the next hand reading
// "bounded" and assuming both senses of the word.
const ESCALATE_MAX_FIRST_PASS = 2

interface EscalationState {
  /** When a pass first ran on this instance. ABSENT means the pass about to run
   *  is the first one — see ESCALATE_MAX_FIRST_PASS. Recorded rather than
   *  inferred from `seen` being empty, because an empty `seen` is also what a
   *  perfectly ordinary pass on a workspace with no pending approvals leaves
   *  behind, and that must not re-arm the first-run damping for ever. */
  firstPassAt?: string
  /** approval key → when each stage fired, and the `waitingSince` those stages
   *  were fired against. Absence means "not yet".
   *
   *  `waitingSince` is recorded because the stages fire ONCE, ever, and an
   *  approval whose clock legitimately restarts (a ticket that left review and
   *  came back inside one tick, an id reused) would otherwise inherit a spent
   *  record and never be raised again. A changed clock is a different wait. */
  seen: Record<string, { naggedAt?: string; escalatedAt?: string; waitingSince?: string }>
}

export interface EscalationRunResult {
  pending: number
  /** First-contact notifications for approvals raised since the last pass. */
  announced: number
  /** Raised since the last pass, and announced to NOBODY WHO MAY READ IT: the
   *  people who can unblock it were told it exists and not a word of what it
   *  says. A workspace with an admin missing from a board, said out loud on the
   *  tick it happens rather than discovered a day later by the stall report. */
  announcedFactOnly: number
  nagged: number
  escalated: number
  /** Aged past the nag threshold with no NAMED owner to nag. They wait for the
   *  escalation stage, which addresses everyone who can decide it; counted so
   *  the log can say so out loud. */
  ownerless: number
  /** Aged out and NOBODY in the workspace can decide it. The admins get a
   *  content-free stall report; counted because it is a configuration fault,
   *  not a busy week. */
  stalled: number
  /** Due to be raised and every notification write failed, or there was not
   *  even an admin to report a stall to. Nothing was marked, so the next pass
   *  tries again; counted so the log does not read as a clean run. */
  unaddressed: number
  /** Due to be raised and left for the next pass because this one hit its cap.
   *  Not an error and not a loss — the next tick is five minutes away and these
   *  are still the oldest things in the queue. Counted so a backlog draining
   *  over an hour is visible as a backlog draining, rather than as a job that
   *  keeps finding new work. */
  deferred: number
  /** True when the cap that applied was the FIRST-PASS one. Says out loud that
   *  this instance is metering itself into the feature. */
  firstPass: boolean
  incomplete: boolean
}

const ageMinutes = (iso: string, now: Date): number => (now.getTime() - new Date(iso).getTime()) / 60_000

/** Fan one message out to a set of people, once each. Notifications route
 *  themselves (in-app / email / both) through each person's own preferences —
 *  this job does not decide delivery, it decides that there IS something to
 *  deliver. The kind is `approval_pending`, which is the class a user has
 *  already tuned under "Approvals waiting on you"; an escalation is not a new
 *  category of thing, it is the same approval, louder and later. */
async function tell(userIds: string[], n: { title: string; body: string; href?: string }): Promise<number> {
  let told = 0
  for (const id of new Set(userIds)) {
    try {
      await addNotification(id, { kind: 'approval_pending', ...n })
      told++
    } catch (e) {
      console.error(`[approval-sla] could not notify ${id}:`, e)
    }
  }
  return told
}

// `KIND_LABEL` — what a content-free report may say an approval IS — is imported
// from server/approvals.ts, where the kinds and their authorities are declared.
// It was written out here, so the raise-time FACT announcement had no way to
// name a kind without importing from the job that AGES approvals or copying the
// table. A fifth kind gets its content-free name in the same file it gets its
// authority, and gets it once.

/** Why nobody could be told what it says, in words an admin can act on.
 *
 *  "Nobody can decide it" and "nobody may be told what it says" are not the same
 *  sentence, and this function used to know only the first one. `{ by: 'admin',
 *  onBoard }` — a repo request an agent raised off a ticket, a capability gap —
 *  resolves its CONTENT to `admins ∩ that board's editors` while every admin in
 *  the workspace can still ACT on it. In a workspace whose only admin is not a
 *  member of that board, the content audience is empty and this function said
 *  "It needs an admin, and this workspace has none" to the very admin who could
 *  have granted it that morning. The report was the first thing anybody heard
 *  about the request and the one sentence in it was false. */
function whyUnreachable(approval: PendingApproval): string {
  switch (approval.authority.by) {
    case 'board':
      return 'The board it belongs to has no members who can edit it, so nobody can approve or reject it. Add an editor to that board.'
    case 'user':
      return 'Only its owner can decide it, and that account no longer exists. It will never be answered as it stands.'
    case 'admin':
      return approval.authority.onBoard
        ? 'Any admin can decide it, but it quotes work on a board none of this workspace’s admins is a member of, so it ' +
            'cannot be shown to anybody as it stands. Add an admin to that board to read it — or decide it from Admin → ' +
            'Agents, where admin-only queues live.'
        : 'It needs an admin, and this workspace has none.'
    default:
      // `{ by: 'nobody' }` is reached from TWO different shapes and one sentence
      // cannot be true of both — the same defect as the admin branch above, one
      // case further down. A workbench plan gets it by having no ticket
      // (api/workbench.jobs PUT 403s a job with no task); a personal Google
      // confirm-send gets it by having no owner left to ask. Telling an admin
      // that somebody's drafted email "is not attached to a ticket" is as false,
      // and as unactionable, as telling them their workspace has no admin.
      return approval.kind === 'google_action'
        ? 'It is one person’s own outbound action and there is no owner account left to ask, so nobody can approve or reject it. ' +
            'Being an admin is not a way in: the decision route refuses anyone but the owner.'
        : 'It is not attached to a ticket, so no surface in Talaria can approve or reject it. It has to be abandoned by the agent that raised it.'
  }
}

/** The stall report's headline. Same distinction as `whyUnreachable`: for an
 *  admin-authority approval bounded to a board, somebody CAN decide it and
 *  nobody may be shown it, and a title that says otherwise is the false sentence
 *  again in bigger type. */
function stallTitle(approval: PendingApproval, waited: string): string {
  return approval.authority.by === 'admin' && approval.authority.onBoard
    ? `An approval has been waiting ${waited} and nobody here may be shown it`
    : `An approval has been waiting ${waited} and nobody can decide it`
}

const waitedFor = (minutes: number): string => {
  const hours = Math.floor(minutes / 60)
  if (hours < 1) return `${Math.max(1, Math.round(minutes))} minutes`
  if (hours < 48) return `${hours} hour${hours === 1 ? '' : 's'}`
  return `${Math.floor(hours / 24)} days`
}

/** One pass of the SLA. Exported for the same reason `runDigest` is. */
export async function runApprovalEscalation(now = new Date()): Promise<EscalationRunResult> {
  const config = await digestConfig()
  const census = await approvalCensus()
  const { approvals, failedKinds, audience } = census
  const state = await getSetting<EscalationState>(ESCALATION_STATE_KEY, { seen: {} })
  const seen: EscalationState['seen'] = { ...(state.seen ?? {}) }
  const firstPass = !state.firstPassAt
  // The budget for THIS pass, spent one notification fan-out at a time.
  const budget = firstPass ? ESCALATE_MAX_FIRST_PASS : ESCALATE_MAX_PER_PASS
  let raised = 0

  // Stage one, before any ageing: anything raised since the last pass is
  // announced to the people who can decide it. Bounded by the nag threshold, so
  // this can never turn into a second nag or a backlog blast on first deploy.
  const sweep = await sweepUnannounced(census, now, config.nagAfterMinutes)

  const result: EscalationRunResult = {
    pending: approvals.length,
    announced: sweep.announced,
    announcedFactOnly: sweep.factOnly,
    nagged: 0,
    escalated: 0,
    ownerless: 0,
    stalled: 0,
    unaddressed: sweep.unreachable,
    deferred: 0,
    firstPass,
    incomplete: failedKinds.length > 0,
  }

  // NOTE: no admin list is fetched here any more. A stall report addresses the
  // FACT half of the disclosure the census already resolved for that exact
  // approval — see `census.audience` — so "who may read this" and "who is told it
  // is stuck" are two halves of ONE answer instead of two answers that could
  // drift. The list this replaced was every admin in the workspace, resolved
  // here, for approvals whose authority might have had something narrower to say.

  for (const approval of approvals) {
    const age = ageMinutes(approval.waitingSince, now)
    const prior = seen[approval.key]
    // A record written against a different `waitingSince` describes a wait that
    // has since ended. Discard it rather than let a spent "escalated once, ever"
    // mark silence the new one.
    const record = prior && (prior.waitingSince === undefined || prior.waitingSince === approval.waitingSince) ? prior : {}
    const mark = (patch: { naggedAt?: string; escalatedAt?: string }) => {
      seen[approval.key] = { ...record, ...patch, waitingSince: approval.waitingSince }
    }

    // Escalated is the LAST stage: there is nothing louder, and this approval
    // has said everything it is ever going to say. Checked before anything
    // else because the two stages are not exclusive — an approval that was
    // already older than the escalation threshold the first time this job saw
    // it escalates without ever having been nagged, and without this line the
    // NEXT pass would then fire the nag ("everyone who can decide it is told in
    // 1 minutes") at a person who was told it had already gone wider.
    if (record.escalatedAt) continue

    // THE AUDIENCE, both halves. `content` is everyone the decision route would
    // let act AND may be shown what it says; `fact` is everyone who may be told
    // only that it is stuck, which is who a stall report is for.
    const who = audience.get(approval.key) ?? { content: [], fact: [] }
    const deciders = who.content
    // The named person on the hook — intersected, so an assignee who has since
    // lost edit access on the board is not sent a ticket they cannot open.
    const owners = approval.ownerUserIds.filter((id) => deciders.includes(id))

    if (age >= config.escalateAfterMinutes) {
      // The cap is checked HERE, once this approval is known to be due and
      // before anybody is written to, so a pass spends its budget on the oldest
      // approvals (the census is sorted oldest-first) and leaves the rest
      // untouched and unmarked for the next tick.
      if (raised >= budget) {
        result.deferred++
        continue
      }
      raised++
      if (!deciders.length) {
        // Nobody may be told what this says. The FACT is worth escalating — an
        // agent is stopped indefinitely — but the content is not, and the people
        // hearing it are being asked to fix the access, not to decide the
        // approval. No title, no detail, no deep link they would be 403'd from.
        const told = await tell(who.fact, {
          title: stallTitle(approval, waitedFor(age)),
          body: `${KIND_LABEL[approval.kind]} has been waiting ${waitedFor(age)}. ${whyUnreachable(approval)}`,
        })
        if (told) {
          mark({ escalatedAt: now.toISOString() })
          result.stalled++
        } else {
          result.unaddressed++
        }
        continue
      }
      // Wider, within the authority. The owner is told too — a person coming
      // back from leave should find out from Talaria that their approval went
      // wider, not from whoever had to make the call for them.
      const others = deciders.filter((id) => !owners.includes(id))
      const told = await tell(deciders, {
        title: `Escalated after ${waitedFor(age)}: ${approval.title}`,
        body:
          `${approval.detail}\n\n` +
          `This has been waiting ${waitedFor(age)} and nobody has decided it. ` +
          (others.length
            ? 'Everyone who can approve or reject it has now been told; whoever gets there first can decide it.'
            : 'You are the only person who can decide it — nothing else will pick it up.'),
        href: approval.href,
      })
      // Marked only when somebody was actually told. A run where every
      // notification write failed has not escalated anything — recording it as
      // escalated would retire the only mechanism that was ever going to raise
      // it, permanently.
      if (told) {
        mark({ escalatedAt: now.toISOString() })
        result.escalated++
      } else {
        result.unaddressed++
      }
      continue
    }

    if (age >= config.nagAfterMinutes && !record.naggedAt) {
      if (!owners.length) {
        // Nobody named to nag. Deliberately NOT marked — the escalation stage
        // above is what widens it to the rest of the deciders, and marking it
        // here would look like it had been handled.
        result.ownerless++
        continue
      }
      // Same cap, and after the ownerless check on purpose: that branch writes
      // to nobody, so it costs nothing and must not consume a pass's budget.
      if (raised >= budget) {
        result.deferred++
        continue
      }
      raised++
      const wider = deciders.length > owners.length
      const told = await tell(owners, {
        title: `Still waiting on you: ${approval.title}`,
        body:
          `${approval.detail}\n\nIt has been waiting ${waitedFor(age)}. ` +
          (wider
            ? `If it is not yours to decide, everyone else who can decide it is told in ${waitedFor(Math.max(0, config.escalateAfterMinutes - age))}.`
            : 'Nobody else can decide this one — it stays blocked until you do.'),
        href: approval.href,
      })
      // Same rule as above: a nag nobody received is not a nag.
      if (told) {
        mark({ naggedAt: now.toISOString() })
        result.nagged++
      } else {
        result.unaddressed++
      }
    }
  }

  // Forget approvals that are no longer pending, so a decided-and-recreated id
  // cannot inherit a stale "already escalated" mark. Only prune when the census
  // was COMPLETE: a kind that failed to read this pass would otherwise look
  // like a kind with nothing pending, and every one of its approvals would be
  // re-nagged the moment the table came back.
  const live = new Set(approvals.map((a) => a.key))
  // THE DAMPING IS SPENT BY A PASS THAT SAW SOMETHING, and by no other kind.
  //
  // `firstPassAt` used to be written at the end of EVERY completed pass. Two of
  // those passes have no business retiring it:
  //   · an EMPTY census — the ordinary first tick ninety seconds after a deploy
  //     on a workspace with nothing pending yet. It metered nothing, because
  //     there was nothing to meter, and the next pass — the one that finds the
  //     backlog the moment the first approval ages in — ran unbounded;
  //   · an INCOMPLETE census (`failedKinds` non-empty) — a transient database
  //     error on one kind. That pass could not SEE the backlog the first-pass
  //     cap exists to meter, so it cannot be the pass that decides the workspace
  //     has been safely metered into the feature.
  // Both burned the one bound standing between a deploy and a notification
  // blast. Only a COMPLETE census with at least one approval in it has actually
  // exercised the budget, so only that spends it; anything else leaves the
  // damping armed for the pass that does. The mark is still written ONCE ever —
  // an instance that re-armed it on every quiet pass would meter itself at two
  // per pass for the rest of its life.
  const spendsFirstPass = !firstPass || (!failedKinds.length && approvals.length > 0)
  const firstPassAt = state.firstPassAt ?? (spendsFirstPass ? now.toISOString() : null)
  await setSetting(ESCALATION_STATE_KEY, {
    ...(firstPassAt ? { firstPassAt } : {}),
    seen: failedKinds.length ? seen : Object.fromEntries(Object.entries(seen).filter(([key]) => live.has(key))),
  } satisfies EscalationState)
  return result
}

registerJob({
  name: 'approval-escalation',
  // Five minutes, and the interval now means something it did not before. The
  // SLA thresholds are hours, so the ageing stages barely notice — but this tick
  // also carries the ANNOUNCE stage, and there the interval IS the latency
  // between an agent stopping and a human hearing about it. Five minutes is the
  // worst case; the raiser calling `announceApproval` directly makes it none.
  //
  //   server/google/pending-actions.ts, at the end of `queueAction`:
  //     void announceApproval(`google_action:${row!.id}`).catch(() => {})
  //   server/workbench-mcp.ts, `announceRaised` — the file's ONE announcement
  //     path, called when `start_job` gates a heavy plan and when `request_repo`
  //     files a repository request.
  //
  // Both are safe to fire without awaiting (they only write notifications) and
  // both are idempotent against this sweep — `announceApproval` and the sweep
  // share one mark, so an approval is announced exactly once either way.
  everyMs: 5 * 60_000,
  firstRunDelayMs: 90_000,
  maxRunMs: 10 * 60_000,
  run: async () => {
    const r = await runApprovalEscalation()
    const parts: string[] = []
    if (r.announced) parts.push(`announced ${r.announced} newly raised`)
    if (r.announcedFactOnly)
      parts.push(
        `${r.announcedFactOnly} newly raised could not be shown to anybody — the people who can unblock them were told they exist`,
      )
    if (r.nagged) parts.push(`nagged ${r.nagged}`)
    if (r.escalated) parts.push(`escalated ${r.escalated}`)
    if (r.ownerless) parts.push(`${r.ownerless} overdue with nobody named to nag (waiting for the escalation threshold)`)
    if (r.stalled) parts.push(`${r.stalled} STUCK with nobody in this workspace able to decide them — reported to the admins`)
    if (r.unaddressed) parts.push(`${r.unaddressed} could not be raised with anybody at all`)
    if (r.deferred)
      parts.push(
        `${r.deferred} due and held for the next pass (capped at ${r.firstPass ? ESCALATE_MAX_FIRST_PASS : ESCALATE_MAX_PER_PASS}` +
          `${r.firstPass ? ' on this instance’s first pass' : ''}/pass)`,
      )
    if (r.incomplete) parts.push('one approval source could not be read')
    if (!parts.length) return null
    return `${parts.join(', ')} — ${r.pending} approval(s) pending`
  },
})
