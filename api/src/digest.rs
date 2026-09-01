// The daily digest, and the approval SLA. Two scheduled jobs, one purpose:
// turn "a human has to look at this" from a hope into a deadline. Port of
// ui/src/server/digest.ts.
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
//   1. NOTHING WAITING, NOTHING SENT. An empty daily email is how a sender
//      gets filtered to trash, and the day the queue is empty is exactly the
//      day the digest has nothing to earn. `build_digest` returns None and the
//      job skips the recipient entirely.
//   2. THE SUBJECT CARRIES THE CONTENT. "3 approvals waiting, 2 in review, 1
//      blocked" tells a person on a phone whether to open it. "You have
//      updates" does not, and after a week of it nobody opens either.
//   3. IT COUNTS WHAT THE SCREEN COUNTS. The queue numbers come from
//      `home_queues` — the same pass Home renders — and the approvals from the
//      one census in approvals.rs. A digest that disagrees with the app is
//      worse than no digest, because now you distrust both.
//   4. IT DISCLOSES NOTHING THE APP WOULD NOT. One email, one visibility
//      model: every line of it is something the recipient could already open
//      in Talaria. The queues are board-membership scoped by `home_queues`;
//      the approvals are scoped by `may_decide`, which is the authority the
//      DECISION ROUTE enforces — board editors for board-anchored approvals (a
//      subset of board membership), the owner alone for a personal
//      confirm-send, the admins for org-scoped things an admin can already
//      open. There is no "you cannot see this but you should know about it"
//      tier, because the version of this file that had one mailed a person's
//      private email subject to an admin who could not have approved it
//      anyway.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::agent_auth::{epoch_ms_to_iso, iso_to_epoch_ms};
use crate::approvals::{
    ApprovalCensus, ApprovalDeps, ApprovalKind, Disclosure, PendingApproval, approval_census,
    kind_label, may_decide, sweep_unannounced,
};
use crate::daily_brief::config::zone_for;
use crate::email::{email_button, email_escape, email_shell};
use crate::gateway::settings::{get_setting, set_setting};
use crate::home::{WorkItem, home_queues};
use crate::notify::{
    GatedMail, NOTIFY_SETTINGS_PATH, NotificationInput, NotifyDeps, add_notification,
    digest_enabled, instance_base_url, send_gated_mail,
};
use crate::runs::define::run_definition;
use crate::runs::run::DefinitionForFn;
use crate::state::AppState;
use crate::tz::local_moment;

// ── Configuration ────────────────────────────────────────────────────────────

/// The digest's knobs, stored as one `app_settings` blob an admin edits
/// through Settings. Every field clamps to its default rather than throwing,
/// because a bad row must not stop a scheduled job for the whole workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct DigestConfig {
    /// Local hour (0–23) the digest is sent at.
    pub hour: i64,
    /// IANA zone the hour is read in when a person has not set their own.
    pub time_zone: String,
    /// How long an approval may sit before its owner is nagged (minutes).
    pub nag_after_minutes: f64,
    /// How long before it goes over the owner's head (minutes).
    pub escalate_after_minutes: f64,
    /// How many items of each kind the email lists before "and N more".
    pub list_limit: usize,
}

const CONFIG_KEY: &str = "digest_config";
const DIGEST_STATE_KEY: &str = "digest_state";
const ESCALATION_STATE_KEY: &str = "approval_escalation_state";

/// `process.env.TZ || 'UTC'` — read per call, not cached, so a config test
/// is not married to the box the binary booted on.
fn default_zone() -> String {
    std::env::var("TZ")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "UTC".into())
}

pub async fn digest_config(pg: &PgPool) -> DigestConfig {
    let stored = get_setting(pg, CONFIG_KEY, json!({})).await;
    // The hour is an integer in range or the default — `Number.isInteger`
    // there, `as_i64` here: 8.5 and "8" both fall back rather than half-fire.
    let hour = stored
        .get("hour")
        .and_then(|v| v.as_i64())
        .filter(|h| (0..=23).contains(h))
        .unwrap_or(8);
    let positive = |key: &str, fallback: f64| {
        stored
            .get(key)
            .and_then(|v| v.as_f64())
            .filter(|n| *n > 0.0)
            .unwrap_or(fallback)
    };
    DigestConfig {
        hour,
        time_zone: stored
            .get("timeZone")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(default_zone),
        // Four hours: long enough that an approval raised mid-morning is not
        // a ping before lunch, short enough that same-day work stays same-day.
        nag_after_minutes: positive("nagAfterMinutes", 4.0 * 60.0),
        // A full day. Past this the owner is demonstrably not coming, and an
        // agent that has been stopped for 24 hours is a cost, not an
        // inconvenience.
        escalate_after_minutes: positive("escalateAfterMinutes", 24.0 * 60.0),
        list_limit: positive("listLimit", 5.0) as usize,
    }
}

// ── Recipients ───────────────────────────────────────────────────────────────

/// One person the pass considers. The TS row carries `name` and `role` too;
/// nothing on this plane reads them — `role` was DELIBERATELY dropped from
/// `build_digest` (the digest is assembled from what the user can reach, and
/// there is no admin bypass to pass in) — so the query does not fetch them.
#[derive(Debug, Clone)]
pub struct Recipient {
    pub id: String,
    pub email: String,
    /// `users.notify_prefs` as stored — the digest's reserved key rides here.
    pub prefs: Option<Value>,
    /// The person's IANA zone, None = follow the workspace default. Carried in
    /// the same query as the rest of the row so the send loop needs no second
    /// fetch per person.
    pub timezone: Option<String>,
}

async fn recipients(pg: &PgPool) -> Result<Vec<Recipient>, sqlx::Error> {
    let rows: Vec<(String, String, Option<Value>, Option<String>)> = sqlx::query_as(
        "select id::text, email, notify_prefs, timezone \
         from users where email is not null and length(trim(email)) > 0 \
         order by created_at asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, email, prefs, timezone)| Recipient {
            id,
            email,
            prefs,
            timezone,
        })
        .collect())
}

/// The zone the send hour is read in for this person: their stored zone, the
/// workspace's otherwise — the same resolution `zone_for` gives the brief, so
/// a person's digest and their brief always speak the same clock.
pub fn recipient_zone(user: &Recipient, config: &DigestConfig) -> String {
    zone_for(user.timezone.as_deref(), &config.time_zone)
}

// ── The digest content ───────────────────────────────────────────────────────

/// Everything waiting on one person, or None when the honest answer is
/// nothing. The approvals are census rows (title/detail/href are CONTENT —
/// see rule 4); the queues carry `WorkItem`s.
#[derive(Debug)]
pub struct DigestContent {
    pub user_id: String,
    pub email: String,
    /// The subject line. Countful by construction — see `headline`.
    pub subject: String,
    pub approvals: Vec<PendingApproval>,
    pub review: Vec<WorkItem>,
    pub blocked: Vec<WorkItem>,
    pub triage: Vec<WorkItem>,
    /// A kind of approval that could not be read this pass, if any. The email
    /// says so rather than quietly under-counting.
    pub incomplete: bool,
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// The content bar: a person who reads only this knows whether to open it.
/// None is "nothing waiting" — a first-class result, not a failure (rule 1).
pub fn headline(approvals: usize, review: usize, blocked: usize, triage: usize) -> Option<String> {
    let mut phrases: Vec<String> = Vec::new();
    if approvals > 0 {
        // "1 item needs your approval" / "2 items need your approval" — the
        // verb agrees with the count, and the TS template spelt the s the same
        // awkward way this does.
        phrases.push(format!(
            "{} need{} your approval",
            plural(approvals, "item"),
            if approvals == 1 { "s" } else { "" }
        ));
    }
    if review > 0 {
        phrases.push(format!("{} in review", plural(review, "ticket")));
    }
    if blocked > 0 {
        phrases.push(format!("{} blocked", plural(blocked, "ticket")));
    }
    if triage > 0 {
        phrases.push(format!("{} to triage", plural(triage, "ticket")));
    }
    if phrases.is_empty() {
        return None;
    }
    Some(phrases.join(", "))
}

/// Everything waiting on one person, or None when the honest answer is nothing
/// (rule 1 — a caller that treats None as "send an empty one" has broken the
/// digest).
///
/// No `role`. The digest is assembled from what this USER can reach, and there
/// is no admin bypass to pass in: `may_decide` is the decision route's own
/// authority, and the role is deliberately not consulted.
pub async fn build_digest(
    pg: &PgPool,
    user: &Recipient,
    census: &ApprovalCensus,
) -> Result<Option<DigestContent>, sqlx::Error> {
    let queues = home_queues(pg, &user.id).await?;

    // ONE VISIBILITY MODEL (rule 4). `home_queues` is board-membership scoped,
    // and this filter is the decision route's own authority — for the
    // board-anchored kinds that is board EDITORS, a subset of the same
    // membership. Being an admin is not a way to read a board you were never
    // added to.
    let approvals: Vec<PendingApproval> = census
        .approvals
        .iter()
        .filter(|a| may_decide(census, a, &user.id))
        .cloned()
        .collect();

    // The review queue is BOTH an approval and a Home queue. It is counted
    // once, as "in review", and the census entry for the same ticket is
    // dropped here — a digest that says "4 need your approval, 3 in review"
    // for three tickets and one email is a digest nobody can act on.
    let approvals_outside_review: Vec<PendingApproval> = approvals
        .into_iter()
        .filter(|a| a.kind != ApprovalKind::TicketReview)
        .collect();

    let bar = headline(
        approvals_outside_review.len(),
        queues.review.count,
        queues.blocked.count,
        queues.triage.count,
    );
    let Some(bar) = bar else {
        return Ok(None);
    };

    Ok(Some(DigestContent {
        user_id: user.id.clone(),
        email: user.email.clone(),
        subject: format!("Talaria: {bar}"),
        approvals: approvals_outside_review,
        review: queues.review.items,
        blocked: queues.blocked.items,
        triage: queues.triage.items,
        incomplete: !census.failed_kinds.is_empty(),
    }))
}

// ── Rendering ────────────────────────────────────────────────────────────────

/// One rendered mail. `unsubscribe_url` becomes `List-Unsubscribe` for the
/// send; None on a deployment with no verified domain, where a header pointing
/// at a URL that does not resolve is worse than no header.
pub struct RenderedDigest {
    pub subject: String,
    pub html: String,
    pub text: String,
    pub unsubscribe_url: Option<String>,
}

struct ListRow {
    label: String,
    sub: String,
    href: Option<String>,
}

fn list_html(rows: &[ListRow]) -> String {
    let items: String = rows
        .iter()
        .map(|r| {
            let linked = match &r.href {
                Some(href) => format!(
                    "<a href=\"{}\" style=\"color:#1a1a18\">{}</a>",
                    email_escape(href),
                    email_escape(&r.label)
                ),
                None => email_escape(&r.label),
            };
            let sub = if r.sub.is_empty() {
                String::new()
            } else {
                format!(
                    "<br><span style=\"color:#8a8a84;font-size:12px\">{}</span>",
                    email_escape(&r.sub)
                )
            };
            format!("<li style=\"margin:0 0 6px\">{linked}{sub}</li>")
        })
        .collect();
    format!("<ul style=\"margin:6px 0 0;padding-left:18px\">{items}</ul>")
}

fn section(title: &str, rows: &[ListRow], total: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let more = if total > rows.len() {
        format!(
            "<p style=\"margin:6px 0 0;font-size:12px;color:#8a8a84\">and {} more</p>",
            total - rows.len()
        )
    } else {
        String::new()
    };
    format!(
        "<h2 style=\"margin:22px 0 0;font-size:13px;text-transform:uppercase;letter-spacing:0.06em;color:#8a8a84\">{}</h2>{}{more}",
        email_escape(title),
        list_html(rows)
    )
}

/// The opt-out, spelled as an instruction, for a deployment that has no
/// verified domain to build a link out of. An email whose only exit is "go and
/// look for it" is an email people report as spam instead, and one report
/// costs the sending domain more than a year of digests earns it.
const DIGEST_OPT_OUT_STEPS: &str = "To stop this daily email, open Talaria and set Settings \u{2192} Notifications \u{2192} Daily digest to Off.";

pub fn render_digest(content: &DigestContent, links: &(Option<String>, usize)) -> RenderedDigest {
    let (base, list_limit) = links;
    let url = |path: &str| -> Option<String> {
        base.as_ref().map(|b| {
            let slash = if path.starts_with('/') { "" } else { "/" };
            format!("{b}{slash}{path}")
        })
    };
    // One ticket section builder, used three times: cap at the list limit and
    // fold to the row the list renders.
    let ticket_rows = |items: &[WorkItem]| -> Vec<ListRow> {
        items
            .iter()
            .take(*list_limit)
            .map(|t| ListRow {
                label: t.label(),
                sub: t.board.clone(),
                href: url(&format!("/boards/{}/{}", t.board_id, t.id)),
            })
            .collect()
    };
    let approval_rows: Vec<ListRow> = content
        .approvals
        .iter()
        .take(*list_limit)
        .map(|a| ListRow {
            label: a.title.clone(),
            sub: a.detail.clone(),
            href: url(&a.href),
        })
        .collect();

    let stripped = content
        .subject
        .strip_prefix("Talaria: ")
        .unwrap_or(&content.subject);
    let mut body = format!("<p style=\"margin:0\">{}.</p>", email_escape(stripped));
    body += &section(
        "Waiting on your approval",
        &approval_rows,
        content.approvals.len(),
    );
    body += &section(
        "In review",
        &ticket_rows(&content.review),
        content.review.len(),
    );
    body += &section(
        "Blocked",
        &ticket_rows(&content.blocked),
        content.blocked.len(),
    );
    body += &section(
        "To triage",
        &ticket_rows(&content.triage),
        content.triage.len(),
    );
    if content.incomplete {
        body += "<p style=\"margin:18px 0 0;font-size:12px;color:#8a8a84\">One source could not be read when this was built, so an approval may be missing from the list above. Open Talaria to see the live queue.</p>";
    }
    if let Some(b) = base {
        body += &email_button(&format!("{b}/"), "Open Talaria");
    }

    let settings = url(NOTIFY_SETTINGS_PATH);
    let footer = match &settings {
        Some(s) => format!(
            "Your daily Talaria digest \u{2014} sent only when something is waiting on you \u{b7} <a href=\"{}\" style=\"color:#8a8a84\">turn the daily digest off</a>",
            email_escape(s)
        ),
        None => format!(
            "Your daily Talaria digest \u{2014} sent only when something is waiting on you \u{b7} {}",
            email_escape(DIGEST_OPT_OUT_STEPS)
        ),
    };

    let mut text_lines: Vec<String> = vec![content.subject.clone(), String::new()];
    if !content.approvals.is_empty() {
        text_lines.push("WAITING ON YOUR APPROVAL".into());
        for a in content.approvals.iter().take(*list_limit) {
            text_lines.push(format!("- {} \u{2014} {}", a.title, a.detail));
        }
        text_lines.push(String::new());
    }
    for (heading, items) in [
        ("IN REVIEW", &content.review),
        ("BLOCKED", &content.blocked),
        ("TO TRIAGE", &content.triage),
    ] {
        if !items.is_empty() {
            text_lines.push(heading.into());
            for t in items.iter().take(*list_limit) {
                text_lines.push(format!("- {} ({})", t.label(), t.board));
            }
            text_lines.push(String::new());
        }
    }
    text_lines.push(
        base.clone()
            .unwrap_or_else(|| "Open Talaria to act on these.".into()),
    );
    text_lines.push(String::new());
    text_lines.push("--".into());
    text_lines
        .push("Your daily Talaria digest, sent only when something is waiting on you.".into());
    // The opt-out is in the TEXT part too. It was not, and a mail client that
    // renders text/plain (or a person reading the raw source) had no way off
    // the list at all.
    text_lines.push(match &settings {
        Some(s) => format!("Turn the daily digest off: {s}"),
        None => DIGEST_OPT_OUT_STEPS.into(),
    });

    RenderedDigest {
        subject: content.subject.clone(),
        html: email_shell(&email_escape(&content.subject), &body, &footer),
        text: text_lines.join("\n"),
        unsubscribe_url: settings,
    }
}

// ── The digest job ───────────────────────────────────────────────────────────

/// userId → the LOCAL calendar date they were last sent a digest for. Written
/// as the pass goes, one settings write per mail, so a pass that dies
/// mid-deploy has already persisted every mark it made.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct DigestState {
    #[serde(rename = "sentOn", default)]
    sent_on: std::collections::BTreeMap<String, String>,
}

/// Only send inside a window that starts at the configured hour. Without it,
/// an instance that boots at 22:00 would see "hour >= 8, nothing sent today"
/// and mail the entire workspace at ten at night.
///
/// The window deliberately does NOT wrap past midnight. A `hour: 23`
/// workspace gets a one-hour window rather than 23:00–01:00, because the
/// "already sent" mark is keyed by LOCAL CALENDAR DATE: a wrapping window
/// would send at 23:10 for date D and again at 00:10 for date D+1. One shorter
/// window beats two emails.
const SEND_WINDOW_HOURS: i64 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DigestRunResult {
    pub considered: usize,
    pub sent: usize,
    pub skipped_nothing_waiting: usize,
    pub skipped_opted_out: usize,
    pub skipped_outside_window: usize,
    /// Built, and then refused by the instance master switch. Its own counter
    /// and NOT `failed`: nothing was attempted, no transport was involved, and
    /// the operator meant it. Counting it as a failure would put a red job in
    /// /observability for a switch working exactly as designed.
    pub skipped_delivery_off: usize,
    pub failed: usize,
    /// Mailed, and then the ledger write that records it FAILED. At most one
    /// per pass, because the pass stops there — see `run_digest`. Its own
    /// counter and not `failed`: the mail went, the memory of it did not, and
    /// the person may be mailed a second time tomorrow morning.
    pub unrecorded: usize,
    /// True when the pass ended early rather than at the end of the recipient
    /// list, because it could no longer record what it had done. The next tick
    /// inside the send window picks up where it stopped.
    pub stopped_early: bool,
}

/// Everything both jobs touch: the app state (pool and the secretbox the mail
/// door needs), the notification edge the SLA writes through, and the run
/// definitions the census reads. `now` is always a PARAMETER on the pass
/// functions — the house rule that keeps them testable.
#[derive(Clone)]
pub struct DigestDeps {
    pub state: AppState,
    pub notify: NotifyDeps,
    pub definition_for: DefinitionForFn,
}

pub fn real_digest_deps(state: &AppState, realtime: crate::realtime::RealtimeDeps) -> DigestDeps {
    DigestDeps {
        state: state.clone(),
        notify: NotifyDeps {
            pg: state.pg.clone(),
            realtime,
        },
        definition_for: Arc::new(run_definition),
    }
}

/// One pass of the digest.
///
/// THE LEDGER IS WRITTEN AS THE PASS GOES, NOT AFTER IT. `sent_on` is what
/// stops a person being mailed twice in one day, and it used to be accumulated
/// in a local map and written ONCE, after the recipient loop. Every way a pass
/// can end early — the process is killed mid-deploy, the container is
/// OOM-killed, SIGTERM lands between two sends, the box loses power — threw
/// away every mark the pass had made, and the next tick (fifteen minutes
/// later, still inside the send window) mailed the whole workspace again. The
/// mails had gone; only the memory of them had not.
///
/// RESUMABLE WAS THE ALTERNATIVE AND IT IS THE SAME THING. There is no cursor
/// to resume from: `sent_on` IS the cursor, keyed per user, and a pass that
/// persists each mark as it makes it is already resumable from wherever it
/// stopped.
///
/// ONE SETTINGS WRITE PER MAIL. Next to an SMTP round trip that is nothing,
/// and it only happens on the send path — the skip marks (opted out, nothing
/// waiting) still ride to the end of the pass, because losing one of those
/// costs a recomputation and not a duplicate email.
///
/// THE RESIDUAL WINDOW IS ONE MAIL WIDE, and it is stated rather than hidden:
/// a crash between the provider accepting a message and the ledger write
/// landing re-mails that ONE person. Closing it entirely needs the mark and
/// the send in one transaction, which no mail provider offers.
/// Err is the TS throw: a pass that could not even read its recipient list is
/// a FAILED run — the scheduler's error path is the only way an operator
/// hears "nobody got their digest and why", and a quiet zero-count pass would
/// read as "nothing to do".
pub async fn run_digest(deps: &DigestDeps, now_ms: i64) -> Result<DigestRunResult, String> {
    let pg = &deps.state.pg;
    let config = digest_config(pg).await;
    let people = match recipients(pg).await {
        Ok(p) => p,
        Err(e) => {
            return Err(format!(
                "digest pass could not read the recipient list: {e}"
            ));
        }
    };
    let mut result = DigestRunResult {
        considered: people.len(),
        ..Default::default()
    };
    if people.is_empty() {
        return Ok(result);
    }

    let state_value = get_setting(pg, DIGEST_STATE_KEY, json!({})).await;
    let mut sent_on: std::collections::BTreeMap<String, String> =
        serde_json::from_value::<DigestState>(state_value)
            .unwrap_or_default()
            .sent_on;

    // Keep the map from growing past the people who still exist. Computed once
    // and applied on every write, so a mid-pass save prunes exactly as the
    // final one does — two writers of one blob that disagreed about its shape
    // would be a way for the last write to resurrect a departed user's mark.
    let live: HashSet<String> = people.iter().map(|p| p.id.clone()).collect();
    // A real fn, not a closure: a closure returning an `async` block that
    // borrows its argument cannot prove the two lifetimes relate, and `async
    // move` would walk off with `live` on the first call. This shape borrows
    // everything per call and is awaited in place, on every mark.
    async fn persist(
        pg: &PgPool,
        live: &HashSet<String>,
        sent_on: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), sqlx::Error> {
        let pruned: std::collections::BTreeMap<String, String> = sent_on
            .iter()
            .filter(|(id, _)| live.contains(*id))
            .map(|(id, date)| (id.clone(), date.clone()))
            .collect();
        set_setting(
            pg,
            DIGEST_STATE_KEY,
            &json!({ "sentOn": serde_json::to_value(&pruned).expect("dates serialize") }),
        )
        .await
    }

    let base = instance_base_url(pg).await;
    // One census — and one audience resolution — for the whole pass. Every
    // recipient filters the same list against the same authority, which is
    // both cheaper and the only way two people can never be told different
    // things about the same approval.
    let census = approval_census(pg, &deps.definition_for).await;

    for person in &people {
        let moment = local_moment(&recipient_zone(person, &config), now_ms);
        let in_window = (moment.hour as i64) >= config.hour
            && (moment.hour as i64) < config.hour + SEND_WINDOW_HOURS;
        if !in_window || sent_on.get(&person.id).is_some_and(|d| *d == moment.date) {
            if !in_window {
                result.skipped_outside_window += 1;
            }
            continue;
        }
        if !digest_enabled(person.prefs.as_ref().unwrap_or(&Value::Null)) {
            result.skipped_opted_out += 1;
            // Marked as handled for the day so a person who opts out is not
            // reconsidered on every 15-minute tick inside the window.
            sent_on.insert(person.id.clone(), moment.date.clone());
            continue;
        }
        let content = match build_digest(pg, person, &census).await {
            Ok(c) => c,
            Err(e) => {
                result.failed += 1;
                tracing::error!(
                    "[digest] building the digest for {} failed: {e}",
                    person.email
                );
                continue;
            }
        };
        let Some(content) = content else {
            result.skipped_nothing_waiting += 1;
            sent_on.insert(person.id.clone(), moment.date.clone());
            continue;
        };
        let mail = render_digest(&content, &(base.clone(), config.list_limit));
        // The secretbox is read when a mail is actually about to go, not once
        // for the pass: a workspace whose keys failed to load should surface
        // it as this person's failed send (retried next tick), not as a reason
        // to skip every recipient's digest outright.
        let sent = match deps.state.secretbox().await {
            Ok(sb) => {
                send_gated_mail(
                    pg,
                    &sb,
                    &GatedMail {
                        to: &person.email,
                        subject: &mail.subject,
                        html: &mail.html,
                        text: Some(&mail.text),
                        unsubscribe_url: mail.unsubscribe_url.as_deref(),
                        what: "daily digest",
                    },
                )
                .await
            }
            Err(e) => crate::notify::GatedSendResult {
                ok: false,
                blocked: false,
                error: Some(e),
            },
        };
        if sent.ok {
            result.sent += 1;
            sent_on.insert(person.id.clone(), moment.date.clone());
            // The mark is written NOW, before the next recipient is even
            // considered.
            if let Err(e) = persist(pg, &live, &sent_on).await {
                // The mail went and we cannot record that it did. Everything
                // already marked is on disk (that is the point of writing as
                // we go), so the damage is bounded to this one person — and it
                // stays bounded only if the pass stops here: carrying on would
                // mail the rest of the workspace against a ledger we already
                // know we cannot write, which is precisely the "one crash
                // re-mails everybody" this change removes. The next tick is
                // fifteen minutes away and still inside the window.
                result.unrecorded += 1;
                result.stopped_early = true;
                tracing::error!(
                    "[digest] mailed {} but could not record it \u{2014} stopping this pass so the rest of the workspace is not mailed against a ledger that cannot be written. They may be mailed again: {e}",
                    person.email
                );
                return Ok(result);
            }
        } else if sent.blocked {
            // The master switch is off. NOT marked sent — if an admin turns
            // mail back on later in the same send window, that day's digest
            // still goes — and not counted as a failure, because nothing
            // failed.
            result.skipped_delivery_off += 1;
        } else {
            // NOT marked sent: a provider outage should retry on the next tick
            // inside the window, not silently cost someone their digest.
            result.failed += 1;
            tracing::error!(
                "[digest] could not send to {}: {}",
                person.email,
                sent.error.as_deref().unwrap_or("unknown error")
            );
        }
    }

    // The skip marks (opted out, nothing waiting) and the prune. The sends are
    // already on disk; this is the cheap half, and losing it to a crash costs
    // a recomputation rather than a duplicate email — which is why it is the
    // half that is still allowed to wait until the end.
    if let Err(e) = persist(pg, &live, &sent_on).await {
        tracing::error!(
            "[digest] could not record the skips for this pass \u{2014} they will be recomputed on the next tick: {e}"
        );
    }
    Ok(result)
}

pub fn digest_job_spec(deps: DigestDeps) -> crate::scheduler::JobSpec {
    use crate::scheduler::{JobName, JobSpec};
    JobSpec {
        name: JobName::DailyDigest,
        // Not hourly: the send window is anchored to a local hour and a
        // 15-minute tick keeps the mail near the top of that hour without a
        // cron expression.
        every_ms: 15 * 60_000,
        // Let a deploy settle. A crash-looping instance must never reach a job
        // that mails the whole workspace.
        first_run_delay_ms: Some(60_000),
        max_run_ms: Some(10 * 60_000),
        per_instance: false,
        run: {
            let run: crate::scheduler::JobFn = Arc::new(move || {
                let deps = deps.clone();
                Box::pin(async move {
                    let now = wall_ms();
                    let r = run_digest(&deps, now).await?;
                    if r.sent == 0
                        && r.failed == 0
                        && r.skipped_delivery_off == 0
                        && r.unrecorded == 0
                    {
                        return Ok(None);
                    }
                    let mut line = format!(
                        "sent {} digest(s) \u{2014} {} had nothing waiting, {} opted out, {} failed",
                        r.sent, r.skipped_nothing_waiting, r.skipped_opted_out, r.failed
                    );
                    if r.skipped_delivery_off > 0 {
                        line += &format!(
                            ", {} held back because email delivery is off for this instance",
                            r.skipped_delivery_off
                        );
                    }
                    // A pass that could not write its own ledger is a FAILED
                    // run, not a run with a footnote: the next tick may re-mail
                    // whoever it could not record, and the only way an operator
                    // hears about that is the scheduler's error path.
                    if r.stopped_early {
                        return Err(format!(
                            "{line} \u{2014} then STOPPED: the \"already sent today\" ledger could not be written after mailing {} recipient(s), who may be mailed again on the next tick. The rest of the workspace was not attempted.",
                            r.unrecorded
                        ));
                    }
                    Ok(Some(line))
                })
            });
            run
        },
    }
}

pub fn register_digest_job(deps: DigestDeps) {
    crate::scheduler::register_job(digest_job_spec(deps));
}

/// The wall clock, stamped once per tick by each job closure.
fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Approval announcement, nag and escalation ────────────────────────────────
//
// THREE STAGES, ONE AUDIENCE RULE
//   announce (on sight) → nag the owner (4h) → escalate (24h)
//
//   Every stage addresses `census.audience` for that approval, or a subset of
//   it, and NOTHING addresses anyone outside it. That set is the authority the
//   decision route enforces: the owner alone for a personal confirm-send, the
//   board's editors for a workbench plan or a ticket sign-off, the admins for
//   org-scoped things.
//
//   The version of this job that shipped before escalated to
//   `[...admins, ...owners]` for every kind. That put the SUBJECT LINE of one
//   person's private confirm-send — "Owen wants to send an email: <their
//   subject>" — in an org admin's notification list and in their mailbox, for
//   an approval the decide route would have refused that admin outright.
//   Being an admin is not entitlement to a colleague's mail, and "it is stuck"
//   is not a reason to forward the contents to somebody who cannot action it
//   either.
//
//   Where an approval ages out with nobody who may be TOLD what it says, the
//   fact still needs raising — an agent is stopped indefinitely. So the FACT
//   audience of that same approval gets a STALL REPORT: how long, what kind,
//   and why it could not be shown to anybody. No title, no detail, no link.
//   That is a thing the recipient can act on (add an admin to that board,
//   grant board access, assign the ticket); the content is not.

/// The deploy-day bound. The nag and escalate loop walks the ENTIRE census, so
/// the first pass on an instance upgrading into this feature takes every
/// approval that had ever been left pending — a year of them on an old
/// workspace — and notifies everyone entitled to decide each one, at once,
/// within ninety seconds of the deploy.
///
/// Bounding it is safe precisely because nothing is dropped: the census sorts
/// oldest-first, so a bounded pass always raises the most overdue ones, and
/// everything it did not reach is simply still due five minutes later. The
/// only cost of the cap is latency on a backlog, and the thing it buys is that
/// the backlog arrives as a queue instead of as a wall.
const ESCALATE_MAX_PER_PASS: usize = 8;

/// The FIRST pass ever is tighter still, and it is a different question from
/// the steady-state cap. Every later pass sees "what aged out since the last
/// tick"; the first sees "everything that has ever been pending on this
/// instance", and it is the one pass whose size nobody can predict before
/// running it. Two per pass drains ~24/hour — visible, reversible, and slow
/// enough that an operator who does not like what they are seeing can turn
/// email off before it becomes the workspace's morning.
///
/// WHAT THIS CAP DOES NOT COVER, said out loud because the comment that used
/// to sit above it claimed otherwise. It bounds the NAG and ESCALATE stages
/// only. The ANNOUNCE stage (`sweep_unannounced`) is bounded by AGE and not by
/// COUNT: everything older than the nag threshold is retired to these two
/// stages, which are capped — but everything YOUNGER is announced in one pass,
/// however many there are. A count cap belongs next to that sweep, not here;
/// naming it is what stops the next hand reading "bounded" and assuming both
/// senses of the word.
const ESCALATE_MAX_FIRST_PASS: usize = 2;

/// When each stage fired for one approval, and the `waiting_since` those
/// stages were fired against. Absence means "not yet".
///
/// `waiting_since` is recorded because the stages fire ONCE, ever, and an
/// approval whose clock legitimately restarts (a ticket that left review and
/// came back inside one tick, an id reused) would otherwise inherit a spent
/// record and never be raised again. A changed clock is a different wait.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct SeenRecord {
    #[serde(rename = "naggedAt", skip_serializing_if = "Option::is_none", default)]
    nagged_at: Option<String>,
    #[serde(
        rename = "escalatedAt",
        skip_serializing_if = "Option::is_none",
        default
    )]
    escalated_at: Option<String>,
    #[serde(
        rename = "waitingSince",
        skip_serializing_if = "Option::is_none",
        default
    )]
    waiting_since: Option<String>,
}

/// The SLA's whole ledger, one `app_settings` blob. `first_pass_at` absent
/// means the pass about to run is the first one — recorded rather than
/// inferred from `seen` being empty, because an empty `seen` is also what a
/// perfectly ordinary pass on a workspace with no pending approvals leaves
/// behind, and that must not re-arm the first-run damping for ever.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct EscalationState {
    #[serde(
        rename = "firstPassAt",
        skip_serializing_if = "Option::is_none",
        default
    )]
    first_pass_at: Option<String>,
    #[serde(default)]
    seen: std::collections::BTreeMap<String, SeenRecord>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EscalationRunResult {
    pub pending: usize,
    /// First-contact notifications for approvals raised since the last pass.
    pub announced: usize,
    /// Raised since the last pass, and announced to NOBODY WHO MAY READ IT:
    /// the people who can unblock it were told it exists and not a word of
    /// what it says. A workspace with an admin missing from a board, said out
    /// loud on the tick it happens rather than discovered a day later by the
    /// stall report.
    pub announced_fact_only: usize,
    pub nagged: usize,
    pub escalated: usize,
    /// Aged past the nag threshold with no NAMED owner to nag. They wait for
    /// the escalation stage, which addresses everyone who can decide it;
    /// counted so the log can say so out loud.
    pub ownerless: usize,
    /// Aged out and NOBODY in the workspace can decide it. The admins get a
    /// content-free stall report; counted because it is a configuration
    /// fault, not a busy week.
    pub stalled: usize,
    /// Due to be raised and every notification write failed, or there was not
    /// even an admin to report a stall to. Nothing was marked, so the next
    /// pass tries again; counted so the log does not read as a clean run.
    pub unaddressed: usize,
    /// Due to be raised and left for the next pass because this one hit its
    /// cap. Not an error and not a loss — the next tick is five minutes away
    /// and these are still the oldest things in the queue. Counted so a
    /// backlog draining over an hour is visible as a backlog draining, rather
    /// than as a job that keeps finding new work.
    pub deferred: usize,
    /// True when the cap that applied was the FIRST-PASS one. Says out loud
    /// that this instance is metering itself into the feature.
    pub first_pass: bool,
    pub incomplete: bool,
}

/// `ageMinutes`: minutes since `waiting_since`, or None for an unparseable
/// clock — TS's `new Date(iso).getTime()` giving NaN, whose every comparison
/// is false. Callers carry that falseness with `is_some_and`.
fn age_minutes(iso: &str, now_ms: i64) -> Option<f64> {
    iso_to_epoch_ms(iso).map(|ms| (now_ms - ms) as f64 / 60_000.0)
}

/// Fan one message out to a set of people, once each. Notifications route
/// themselves (in-app / email / both) through each person's own preferences —
/// this job does not decide delivery, it decides that there IS something to
/// deliver. The kind is `approval_pending`, which is the class a user has
/// already tuned under "Approvals waiting on you"; an escalation is not a new
/// category of thing, it is the same approval, louder and later.
async fn tell(
    deps: &NotifyDeps,
    user_ids: &[String],
    title: &str,
    body: &str,
    href: Option<&str>,
) -> usize {
    let mut told = 0;
    let mut seen: HashSet<&str> = HashSet::new();
    for id in user_ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        match add_notification(
            deps,
            id,
            &NotificationInput {
                kind: "approval_pending",
                title,
                body: Some(body),
                href,
            },
        )
        .await
        {
            Ok(()) => told += 1,
            Err(e) => tracing::error!("[approval-sla] could not notify {id}: {e}"),
        }
    }
    told
}

/// Why nobody could be told what it says, in words an admin can act on.
///
/// "Nobody can decide it" and "nobody may be told what it says" are not the
/// same sentence, and this function used to know only the first one.
/// `{ by: 'admin', onBoard }` — a repo request an agent raised off a ticket, a
/// capability gap — resolves its CONTENT to `admins ∩ that board's editors`
/// while every admin in the workspace can still ACT on it. In a workspace
/// whose only admin is not a member of that board, the content audience is
/// empty and this function said "It needs an admin, and this workspace has
/// none" to the very admin who could have granted it that morning.
fn why_unreachable(approval: &PendingApproval) -> &'static str {
    use crate::runs::define::Authority;
    match &approval.authority {
        Authority::Board { .. } => {
            "The board it belongs to has no members who can edit it, so nobody can approve or reject it. Add an editor to that board."
        }
        Authority::User { .. } => {
            "Only its owner can decide it, and that account no longer exists. It will never be answered as it stands."
        }
        Authority::Admin { on_board: None } => "It needs an admin, and this workspace has none.",
        Authority::Admin { on_board: Some(_) } => {
            "Any admin can decide it, but it quotes work on a board none of this workspace\u{2019}s admins is a member of, so it cannot be shown to anybody as it stands. Add an admin to that board to read it, or decide it from Admin \u{2192} Agents, where admin-only queues live."
        }
        // `Nobody` is reached from TWO different shapes and one sentence
        // cannot be true of both — the same defect as the admin branch above,
        // one case further down. A workbench plan gets it by having no ticket
        // (the workbench job route 403s a job with no task); a personal Google
        // confirm-send gets it by having no owner left to ask. Telling an
        // admin that somebody's drafted email "is not attached to a ticket" is
        // as false, and as unactionable, as telling them their workspace has
        // no admin.
        Authority::Nobody if approval.kind == ApprovalKind::GoogleAction => {
            "It is one person\u{2019}s own outbound action and there is no owner account left to ask, so nobody can approve or reject it. Being an admin is not a way in: the decision route refuses anyone but the owner."
        }
        Authority::Nobody => {
            "It is not attached to a ticket, so no surface in Talaria can approve or reject it. It has to be abandoned by the agent that raised it."
        }
    }
}

/// The stall report's headline. Same distinction as `why_unreachable`: for an
/// admin-authority approval bounded to a board, somebody CAN decide it and
/// nobody may be shown it, and a title that says otherwise is the false
/// sentence again in bigger type.
fn stall_title(approval: &PendingApproval, waited: &str) -> String {
    use crate::runs::define::Authority;
    match &approval.authority {
        Authority::Admin { on_board: Some(_) } => {
            format!("An approval has been waiting {waited} and nobody here may be shown it")
        }
        _ => format!("An approval has been waiting {waited} and nobody can decide it"),
    }
}

fn waited_for(minutes: f64) -> String {
    let hours = (minutes / 60.0).floor();
    if hours < 1.0 {
        // Sub-hour waits round UP to their nearest minute, floor 1 — "1
        // minutes" is the TS quirk (`${Math.max(1, Math.round(minutes))}
        // minutes`) and it ships rather than being silently grammar-fixed.
        return format!("{} minutes", minutes.round().max(1.0) as i64);
    }
    if hours < 48.0 {
        return format!("{hours} hour{}", if hours == 1.0 { "" } else { "s" });
    }
    format!("{} days", (hours / 24.0).floor() as i64)
}

/// One pass of the SLA. Err is the TS throw on the closing state write: the
/// notifications already went out, and a pass whose marks did not persist is a
/// FAILED run — the stages will re-fire on the next tick, and an operator
/// hearing nothing would read a duplicated nag as the SLA misbehaving.
pub async fn run_approval_escalation(
    deps: &DigestDeps,
    now_ms: i64,
) -> Result<EscalationRunResult, String> {
    let pg = deps.state.pg.clone();
    let config = digest_config(&pg).await;
    let census = approval_census(&pg, &deps.definition_for).await;
    let state_value = get_setting(&pg, ESCALATION_STATE_KEY, json!({})).await;
    let state: EscalationState = serde_json::from_value(state_value).unwrap_or_default();
    let first_pass = state.first_pass_at.is_none();
    // The budget for THIS pass, spent one notification fan-out at a time.
    let budget = if first_pass {
        ESCALATE_MAX_FIRST_PASS
    } else {
        ESCALATE_MAX_PER_PASS
    };
    let mut raised: usize = 0;

    let now_iso = epoch_ms_to_iso(now_ms);
    // The announce stage rides the same deps the ageing stages do; its `now`
    // is pinned to this pass's instant so a sweep that runs long cannot stamp
    // its last mark minutes after its first.
    let approval_deps = ApprovalDeps::new(
        pg.clone(),
        deps.notify.realtime.clone(),
        deps.definition_for.clone(),
        Arc::new(move || now_ms),
    );

    // Stage one, before any ageing: anything raised since the last pass is
    // announced to the people who can decide it. Bounded by the nag threshold,
    // so this can never turn into a second nag or a backlog blast on first
    // deploy.
    let sweep = sweep_unannounced(&approval_deps, &census, now_ms, config.nag_after_minutes).await;

    let mut result = EscalationRunResult {
        pending: census.approvals.len(),
        announced: sweep.announced,
        announced_fact_only: sweep.fact_only,
        unaddressed: sweep.unreachable,
        first_pass,
        incomplete: !census.failed_kinds.is_empty(),
        ..Default::default()
    };

    let mut seen = state.seen.clone();
    for approval in &census.approvals {
        let Some(age) = age_minutes(&approval.waiting_since, now_ms) else {
            continue;
        };
        let prior = seen.get(&approval.key);
        // A record written against a different `waiting_since` describes a
        // wait that has since ended. Discard it rather than let a spent
        // "escalated once, ever" mark silence the new one.
        let record = match prior {
            Some(r)
                if r.waiting_since.is_none()
                    || r.waiting_since.as_deref() == Some(approval.waiting_since.as_str()) =>
            {
                r.clone()
            }
            _ => SeenRecord::default(),
        };
        let mark = |seen: &mut std::collections::BTreeMap<String, SeenRecord>,
                    patch_nagged: bool,
                    nagged: Option<String>,
                    escalated: Option<String>| {
            let mut next = record.clone();
            if patch_nagged {
                next.nagged_at = nagged;
            } else {
                next.escalated_at = escalated;
            }
            next.waiting_since = Some(approval.waiting_since.clone());
            seen.insert(approval.key.clone(), next);
        };

        // Escalated is the LAST stage: there is nothing louder, and this
        // approval has said everything it is ever going to say. Checked before
        // anything else because the two stages are not exclusive — an approval
        // that was already older than the escalation threshold the first time
        // this job saw it escalates without ever having been nagged, and
        // without this line the NEXT pass would then fire the nag at a person
        // who was told it had already gone wider.
        if record.escalated_at.is_some() {
            continue;
        }

        // THE AUDIENCE, both halves. `content` is everyone the decision route
        // would let act AND may be shown what it says; `fact` is everyone who
        // may be told only that it is stuck, which is who a stall report is
        // for.
        let who: &Disclosure = census
            .audience
            .get(&approval.key)
            .unwrap_or(&EMPTY_DISCLOSURE);
        let deciders = &who.content;
        // The named person on the hook — intersected, so an assignee who has
        // since lost edit access on the board is not sent a ticket they cannot
        // open.
        let owners: Vec<&String> = approval
            .owner_user_ids
            .iter()
            .filter(|id| deciders.contains(id))
            .collect();

        if age >= config.escalate_after_minutes {
            // The cap is checked HERE, once this approval is known to be due
            // and before anybody is written to, so a pass spends its budget on
            // the oldest approvals (the census is sorted oldest-first) and
            // leaves the rest untouched and unmarked for the next tick.
            if raised >= budget {
                result.deferred += 1;
                continue;
            }
            raised += 1;
            if deciders.is_empty() {
                // Nobody may be told what this says. The FACT is worth
                // escalating — an agent is stopped indefinitely — but the
                // content is not, and the people hearing it are being asked to
                // fix the access, not to decide the approval. No title, no
                // detail, no deep link they would be 403'd from.
                let told = tell(
                    &deps.notify,
                    &who.fact,
                    &stall_title(approval, &waited_for(age)),
                    &format!(
                        "{} has been waiting {}. {}",
                        kind_label(approval.kind),
                        waited_for(age),
                        why_unreachable(approval)
                    ),
                    None,
                )
                .await;
                if told > 0 {
                    mark(&mut seen, false, None, Some(now_iso.clone()));
                    result.stalled += 1;
                } else {
                    result.unaddressed += 1;
                }
                continue;
            }
            // Wider, within the authority. The owner is told too — a person
            // coming back from leave should find out from Talaria that their
            // approval went wider, not from whoever had to make the call for
            // them.
            let others: Vec<&String> = deciders.iter().filter(|id| !owners.contains(id)).collect();
            let told = tell(
                &deps.notify,
                deciders,
                &format!("Escalated after {}: {}", waited_for(age), approval.title),
                &format!(
                    "{}\n\nThis has been waiting {} and nobody has decided it. {}",
                    approval.detail,
                    waited_for(age),
                    if others.is_empty() {
                        "You are the only person who can decide it. Nothing else will pick it up."
                    } else {
                        "Everyone who can approve or reject it has now been told; whoever gets there first can decide it."
                    }
                ),
                Some(&approval.href),
            )
            .await;
            // Marked only when somebody was actually told. A run where every
            // notification write failed has not escalated anything — recording
            // it as escalated would retire the only mechanism that was ever
            // going to raise it, permanently.
            if told > 0 {
                mark(&mut seen, false, None, Some(now_iso.clone()));
                result.escalated += 1;
            } else {
                result.unaddressed += 1;
            }
            continue;
        }

        if age >= config.nag_after_minutes && record.nagged_at.is_none() {
            if owners.is_empty() {
                // Nobody named to nag. Deliberately NOT marked — the
                // escalation stage above is what widens it to the rest of the
                // deciders, and marking it here would look like it had been
                // handled.
                result.ownerless += 1;
                continue;
            }
            // Same cap, and after the ownerless check on purpose: that branch
            // writes to nobody, so it costs nothing and must not consume a
            // pass's budget.
            if raised >= budget {
                result.deferred += 1;
                continue;
            }
            raised += 1;
            let wider = deciders.len() > owners.len();
            let told = tell(
                &deps.notify,
                &owners.iter().map(|s| (*s).clone()).collect::<Vec<_>>(),
                &format!("Still waiting on you: {}", approval.title),
                &format!(
                    "{}\n\nIt has been waiting {}. {}",
                    approval.detail,
                    waited_for(age),
                    if wider {
                        format!(
                            "If it is not yours to decide, everyone else who can decide it is told in {}.",
                            waited_for((config.escalate_after_minutes - age).max(0.0))
                        )
                    } else {
                        "Nobody else can decide this one; it stays blocked until you do.".to_string()
                    }
                ),
                Some(&approval.href),
            )
            .await;
            // Same rule as above: a nag nobody received is not a nag.
            if told > 0 {
                mark(&mut seen, true, Some(now_iso.clone()), None);
                result.nagged += 1;
            } else {
                result.unaddressed += 1;
            }
        }
    }

    // Forget approvals that are no longer pending, so a decided-and-recreated
    // id cannot inherit a stale "already escalated" mark. Only prune when the
    // census was COMPLETE: a kind that failed to read this pass would
    // otherwise look like a kind with nothing pending, and every one of its
    // approvals would be re-nagged the moment the table came back.
    let live: HashSet<&str> = census.approvals.iter().map(|a| a.key.as_str()).collect();
    // THE DAMPING IS SPENT BY A PASS THAT SAW SOMETHING, and by no other kind.
    //
    // `first_pass_at` used to be written at the end of EVERY completed pass.
    // Two of those passes have no business retiring it:
    //   · an EMPTY census — the ordinary first tick ninety seconds after a
    //     deploy on a workspace with nothing pending yet. It metered nothing,
    //     because there was nothing to meter, and the next pass — the one that
    //     finds the backlog the moment the first approval ages in — ran
    //     unbounded;
    //   · an INCOMPLETE census (`failed_kinds` non-empty) — a transient
    //     database error on one kind. That pass could not SEE the backlog the
    //     first-pass cap exists to meter, so it cannot be the pass that
    //     decides the workspace has been safely metered into the feature.
    // Both burned the one bound standing between a deploy and a notification
    // blast. Only a COMPLETE census with at least one approval in it has
    // actually exercised the budget, so only that spends it; anything else
    // leaves the damping armed for the pass that does. The mark is still
    // written ONCE ever — an instance that re-armed it on every quiet pass
    // would meter itself at two per pass for the rest of its life.
    let spends_first_pass =
        !first_pass || (census.failed_kinds.is_empty() && !census.approvals.is_empty());
    let next = EscalationState {
        first_pass_at: state
            .first_pass_at
            .or_else(|| spends_first_pass.then(|| now_iso.clone())),
        seen: if census.failed_kinds.is_empty() {
            seen.into_iter()
                .filter(|(key, _)| live.contains(key.as_str()))
                .collect()
        } else {
            seen
        },
    };
    if let Err(e) = set_setting(
        &pg,
        ESCALATION_STATE_KEY,
        &serde_json::to_value(&next).expect("escalation state serializes"),
    )
    .await
    {
        // Logged AND failed, not one or the other: the log names the pass, the
        // scheduler's red job is what makes somebody look.
        tracing::error!(
            "[approval-sla] could not record this pass's marks \u{2014} stages may re-fire on the next tick: {e}"
        );
        return Err(format!(
            "approval SLA pass could not record its marks \u{2014} stages may re-fire on the next tick: {e}"
        ));
    }
    Ok(result)
}

/// The empty disclosure an approval with no resolved audience falls back to —
/// a static so the borrow in the loop has somewhere to live.
static EMPTY_DISCLOSURE: Disclosure = Disclosure {
    content: Vec::new(),
    fact: Vec::new(),
};

pub fn approval_escalation_job_spec(deps: DigestDeps) -> crate::scheduler::JobSpec {
    use crate::scheduler::{JobName, JobSpec};
    JobSpec {
        name: JobName::ApprovalEscalation,
        // Five minutes, and the interval now means something it did not
        // before. The SLA thresholds are hours, so the ageing stages barely
        // notice — but this tick also carries the ANNOUNCE stage, and there
        // the interval IS the latency between an agent stopping and a human
        // hearing about it. Five minutes is the worst case; the raiser calling
        // `announce_approval` directly makes it none.
        every_ms: 5 * 60_000,
        first_run_delay_ms: Some(90_000),
        max_run_ms: Some(10 * 60_000),
        per_instance: false,
        run: {
            let run: crate::scheduler::JobFn = Arc::new(move || {
                let deps = deps.clone();
                Box::pin(async move {
                    let r = run_approval_escalation(&deps, wall_ms()).await?;
                    let mut parts: Vec<String> = Vec::new();
                    if r.announced > 0 {
                        parts.push(format!("announced {} newly raised", r.announced));
                    }
                    if r.announced_fact_only > 0 {
                        parts.push(format!(
                            "{} newly raised could not be shown to anybody \u{2014} the people who can unblock them were told they exist",
                            r.announced_fact_only
                        ));
                    }
                    if r.nagged > 0 {
                        parts.push(format!("nagged {}", r.nagged));
                    }
                    if r.escalated > 0 {
                        parts.push(format!("escalated {}", r.escalated));
                    }
                    if r.ownerless > 0 {
                        parts.push(format!(
                            "{} overdue with nobody named to nag (waiting for the escalation threshold)",
                            r.ownerless
                        ));
                    }
                    if r.stalled > 0 {
                        parts.push(format!(
                            "{} STUCK with nobody in this workspace able to decide them \u{2014} reported to the admins",
                            r.stalled
                        ));
                    }
                    if r.unaddressed > 0 {
                        parts.push(format!(
                            "{} could not be raised with anybody at all",
                            r.unaddressed
                        ));
                    }
                    if r.deferred > 0 {
                        let cap = if r.first_pass {
                            ESCALATE_MAX_FIRST_PASS
                        } else {
                            ESCALATE_MAX_PER_PASS
                        };
                        let tail = if r.first_pass {
                            " on this instance\u{2019}s first pass"
                        } else {
                            ""
                        };
                        parts.push(format!(
                            "{} due and held for the next pass (capped at {cap}{tail}/pass)",
                            r.deferred
                        ));
                    }
                    if r.incomplete {
                        parts.push("one approval source could not be read".into());
                    }
                    if parts.is_empty() {
                        return Ok(None);
                    }
                    Ok(Some(format!(
                        "{} \u{2014} {} approval(s) pending",
                        parts.join(", "),
                        r.pending
                    )))
                })
            });
            run
        },
    }
}

pub fn register_approval_escalation_job(deps: DigestDeps) {
    crate::scheduler::register_job(approval_escalation_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(timezone: Option<&str>) -> Recipient {
        Recipient {
            id: "u1".into(),
            email: "a@b.c".into(),
            prefs: Some(json!({})),
            timezone: timezone.map(String::from),
        }
    }

    fn config() -> DigestConfig {
        DigestConfig {
            hour: 8,
            time_zone: "UTC".into(),
            nag_after_minutes: 240.0,
            escalate_after_minutes: 1440.0,
            list_limit: 5,
        }
    }

    #[test]
    fn recipient_zone_reads_the_persons_clock() {
        // The person's stored zone wins; the workspace's is the fallback; a
        // hand-emptied row counts as unset rather than handing the formatter a
        // blank — same survivability rule as a typo in the org config.
        assert_eq!(
            recipient_zone(&person(Some("Asia/Tokyo")), &config()),
            "Asia/Tokyo"
        );
        let denver = DigestConfig {
            time_zone: "America/Denver".into(),
            ..config()
        };
        assert_eq!(recipient_zone(&person(None), &denver), "America/Denver");
        assert_eq!(
            recipient_zone(&person(Some("   ")), &denver),
            "America/Denver"
        );
    }

    #[test]
    fn the_headline_counts_or_stays_silent() {
        assert_eq!(headline(0, 0, 0, 0), None);
        assert_eq!(
            headline(1, 0, 0, 0),
            Some("1 item needs your approval".into())
        );
        assert_eq!(
            headline(2, 0, 0, 0),
            Some("2 items need your approval".into())
        );
        assert_eq!(
            headline(0, 3, 1, 2),
            Some("3 tickets in review, 1 ticket blocked, 2 tickets to triage".into())
        );
        // The whole bar in one line — this is the subject a phone shows.
        assert_eq!(
            headline(2, 3, 1, 0),
            Some("2 items need your approval, 3 tickets in review, 1 ticket blocked".into())
        );
    }

    #[test]
    fn waited_for_rounds_up_under_an_hour_and_goes_days_past_two() {
        assert_eq!(waited_for(0.4), "1 minutes"); // the TS quirk, shipped
        assert_eq!(waited_for(30.0), "30 minutes");
        assert_eq!(waited_for(60.0), "1 hour");
        assert_eq!(waited_for(120.0), "2 hours");
        assert_eq!(waited_for(47.9 * 60.0), "47 hours");
        assert_eq!(waited_for(72.0 * 60.0), "3 days");
    }

    #[test]
    fn the_send_window_starts_at_the_hour_and_never_wraps() {
        let in_window = |hour: i64, config_hour: i64| {
            hour >= config_hour && hour < config_hour + SEND_WINDOW_HOURS
        };
        // An 08:00 workspace: 08:00 and 09:59 local are in, 07:59 and 10:00
        // are not, and an instance that boots at 22:00 stays quiet.
        assert!(in_window(8, 8));
        assert!(in_window(9, 8));
        assert!(!in_window(7, 8));
        assert!(!in_window(10, 8));
        assert!(!in_window(22, 8));
        // A 23:00 workspace gets ONE hour, not a window that spills into
        // tomorrow's calendar date — 00:10 would be date D+1's mark and mail
        // the same person twice (see SEND_WINDOW_HOURS).
        assert!(in_window(23, 23));
        assert!(!in_window(0, 23));
        assert!(!in_window(1, 23));
    }

    #[test]
    fn a_rendered_digest_carries_its_links_and_its_exit() {
        let content = DigestContent {
            user_id: "u1".into(),
            email: "a@b.c".into(),
            subject: "Talaria: 2 items need your approval, 1 ticket blocked".into(),
            approvals: vec![],
            review: vec![],
            blocked: vec![WorkItem {
                id: "t1".into(),
                board_id: "b1".into(),
                board: "Ops".into(),
                ticket_ref: Some("TASK-9".into()),
                title: "Fix the login loop".into(),
                status: "blocked".into(),
                updated_at: "2026-08-29T00:00:00.000Z".into(),
                queue: "blocked".into(),
            }],
            triage: vec![],
            incomplete: false,
        };
        let mail = render_digest(&content, &(Some("https://talaria.example.com".into()), 5));
        assert_eq!(mail.subject, content.subject);
        // The body opens with the subject MINUS the "Talaria: " prefix — the
        // shell already says who sent it.
        assert!(
            mail.html
                .contains("2 items need your approval, 1 ticket blocked.</p>")
        );
        assert!(mail.html.contains("TASK-9 \u{b7} Fix the login loop"));
        assert!(
            mail.html
                .contains("href=\"https://talaria.example.com/boards/b1/t1\"")
        );
        assert!(
            mail.html
                .contains("https://talaria.example.com/settings/notifications")
        );
        assert_eq!(
            mail.unsubscribe_url.as_deref(),
            Some("https://talaria.example.com/settings/notifications")
        );
        // The text part carries the same counts and its own opt-out line.
        assert!(
            mail.text
                .contains("BLOCKED\n- TASK-9 \u{b7} Fix the login loop (Ops)")
        );
        assert!(mail.text.contains(
            "Turn the daily digest off: https://talaria.example.com/settings/notifications"
        ));

        // No verified domain: no links, no List-Unsubscribe, and the spelled
        // instruction instead of a URL that would not resolve.
        let bare = render_digest(&content, &(None, 5));
        assert_eq!(bare.unsubscribe_url, None);
        // No base means no button in the HTML (TS renders it only on the
        // `links.base ?` arm) and no URL anywhere in the mail — the exit is
        // the spelled instruction, and the text part points at the app itself.
        assert!(!bare.html.contains("https://"));
        assert!(bare.text.contains("Open Talaria to act on these."));
        assert!(bare.text.contains(DIGEST_OPT_OUT_STEPS));
    }

    #[test]
    fn list_limit_caps_the_lists_and_says_how_much_is_left() {
        let many: Vec<WorkItem> = (0..7)
            .map(|i| WorkItem {
                id: format!("t{i}"),
                board_id: "b1".into(),
                board: "Ops".into(),
                ticket_ref: None,
                title: format!("ticket {i}"),
                status: "inbox".into(),
                updated_at: String::new(),
                queue: "triage".into(),
            })
            .collect();
        let content = DigestContent {
            user_id: "u1".into(),
            email: "a@b.c".into(),
            subject: "Talaria: 7 tickets to triage".into(),
            approvals: vec![],
            review: vec![],
            blocked: vec![],
            triage: many,
            incomplete: false,
        };
        let mail = render_digest(&content, &(None, 5));
        assert!(mail.html.contains("ticket 4"));
        assert!(!mail.html.contains("ticket 5"));
        assert!(mail.html.contains("and 2 more"));
        // The TEXT part caps too, with no "and N more" — a plain-text reader
        // counts what is there.
        assert!(mail.text.contains("ticket 4"));
        assert!(!mail.text.contains("ticket 5"));
    }

    #[test]
    fn an_incomplete_digest_says_so_in_both_parts() {
        let content = DigestContent {
            user_id: "u1".into(),
            email: "a@b.c".into(),
            subject: "Talaria: 1 item needs your approval".into(),
            approvals: vec![],
            review: vec![],
            blocked: vec![],
            triage: vec![],
            incomplete: true,
        };
        let mail = render_digest(&content, &(None, 5));
        assert!(
            mail.html
                .contains("One source could not be read when this was built")
        );
    }

    #[test]
    fn the_state_blobs_round_trip_in_the_ts_spelling() {
        // The TS scheduler still owns this job until the flip; the blob this
        // pass writes is the one the NEXT pass — either runtime — reads.
        let state = DigestState {
            sent_on: [("u1".to_string(), "2026-08-29".to_string())]
                .into_iter()
                .collect(),
        };
        let v = serde_json::to_value(&state).expect("serializes");
        assert_eq!(v, json!({"sentOn": {"u1": "2026-08-29"}}));

        let esc: EscalationState = serde_json::from_value(json!({
            "firstPassAt": "2026-08-29T00:00:00.000Z",
            "seen": {"run_decision:r1": {"escalatedAt": "2026-08-29T00:00:00.000Z", "waitingSince": "2026-08-28T00:00:00.000Z"}}
        }))
        .expect("deserializes");
        assert_eq!(
            esc.first_pass_at.as_deref(),
            Some("2026-08-29T00:00:00.000Z")
        );
        let seen = esc.seen.get("run_decision:r1").expect("seen row");
        assert_eq!(seen.nagged_at, None);
        assert_eq!(
            seen.escalated_at.as_deref(),
            Some("2026-08-29T00:00:00.000Z")
        );

        // A written-back record keeps the TS spellings and drops nothing the
        // other runtime wrote.
        let back = serde_json::to_value(&esc).expect("serializes");
        assert_eq!(
            back,
            json!({
                "firstPassAt": "2026-08-29T00:00:00.000Z",
                "seen": {"run_decision:r1": {"escalatedAt": "2026-08-29T00:00:00.000Z", "waitingSince": "2026-08-28T00:00:00.000Z"}}
            })
        );
    }
}
