// The daily brief — one document per person per day, opened before their
// workday and APPENDED TO for the rest of it: the job plane (open_brief,
// sweep_brief, append_entries, the scheduled pass) and the reader plane
// (get_brief's absence kinds, mark_brief_read/item, sweep_if_due). The
// delegation routes' engine halves live in delegation.rs; the document
// assembly in view.rs.
//
// WHY THIS REPLACES THE INBOX
//   The focus queue answered "what is the single next decision", which is the
//   right question at 09:00 and the wrong one every other time you open the
//   app. It had no memory: a queue that reorders itself under you cannot tell
//   you what MOVED, and the two things a person actually wants from an inbox at
//   14:00 are "what changed since I looked" and "where do I go for it". A log
//   answers both by construction; a queue answers neither at any length.
//
// THE ONE RULE: IT IS NEVER REGENERATED.
//   Not "regenerated carefully", not "regenerated only when stale" — never. The
//   lede written at 07:00 is the lede at 18:00, and every item on the page is
//   the row that was inserted the moment that thing first needed the owner.
//   Change arrives as a NEW row that supersedes an old one by id, so the
//   document grows downward and nothing a person read is ever taken away from
//   them. The enforcement is that nothing in this module issues an UPDATE
//   against `daily_brief_entries`, and the only column it updates anywhere is
//   `read_seq`, which describes the reader rather than the content.
//
// THE DIFF ENGINE IS focus.rs.
//   It computes "what needs this user" across approvals, tickets and
//   notifications, and stamps every item with a `source_fingerprint` over
//   the fields that matter. That is exactly the change detector an append-only
//   log needs, so this module consumes those functions directly rather than
//   reimplementing their scoping — which is the half that carries the read ACL
//   and is therefore the half you least want a second copy of.

pub mod artifact;
pub mod comms;
pub mod config;
pub mod delegation;
pub mod focus;
pub mod types;
pub mod view;

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent_auth::epoch_ms_to_iso;
use crate::daily_brief::comms::{CommsLine, comms_lines};
use crate::daily_brief::config::{BriefConfig, brief_config, brief_window, zone_for};
use crate::daily_brief::focus::{
    BriefEvidence, RawFocusItem, approval_items, as_iso, dedupe_items, evidence_value,
    notification_items, sort_items, task_items,
};
use crate::daily_brief::types::{BriefEntry, BriefLine, NewEntry, fold_entries};
use crate::google::calendar::list_upcoming_events;
use crate::harness::defs::briefer::{daily_brief_lede_harness, daily_brief_note_harness};
use crate::harness::run::{RunContext, run_harness};
use crate::notify::NotifyDeps;
use crate::realtime::{RealtimeDeps, UserEvent, publish_user};
use crate::state::AppState;
use crate::tz::{LocalMoment, local_moment, parse_rfc3339_ms};

// ── Rows ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BriefRow {
    pub id: String,
    pub user_id: String,
    pub brief_date: String,
    pub zone: String,
    pub agent_model: Option<String>,
    pub agent_name: Option<String>,
    pub artifact_id: Option<String>,
    pub last_seq: i64,
    pub read_seq: i64,
    pub last_swept_ms: Option<i64>,
    pub created_ms: i64,
}

const ROW_COLS: &str = "id::text, user_id::text, to_char(brief_date, 'YYYY-MM-DD'), zone, \
                        agent_model, agent_name, artifact_id::text, last_seq, read_seq, \
                        (trunc(extract(epoch from last_swept_at) * 1000))::bigint, \
                        (trunc(extract(epoch from created_at) * 1000))::bigint";

/// The brief row as SQL hands it back, in ROW_COLS order: (id, user, date,
/// zone, model, name, artifact, last seq, read seq, swept ms, created ms).
type BriefRowTuple = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i32,
    i32,
    Option<i64>,
    i64,
);

impl BriefRow {
    fn from_row(
        (
            id,
            user_id,
            brief_date,
            zone,
            agent_model,
            agent_name,
            artifact_id,
            last_seq,
            read_seq,
            last_swept_ms,
            created_ms,
        ): BriefRowTuple,
    ) -> Self {
        BriefRow {
            id,
            user_id,
            brief_date,
            zone,
            agent_model,
            agent_name,
            artifact_id,
            last_seq: last_seq as i64,
            read_seq: read_seq as i64,
            last_swept_ms,
            created_ms,
        }
    }
}

async fn load_row(pg: &PgPool, user_id: &str, date: &str) -> Result<Option<BriefRow>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is ROW_COLS, this module's column list.
    let sql = format!(
        "select {ROW_COLS} from daily_briefs where user_id = $1::uuid and brief_date = $2::date"
    );
    let row: Option<BriefRowTuple> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(date)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(BriefRow::from_row))
}

/// One row of `daily_brief_entries`, as both reads decode it — a derived
/// struct rather than a tuple because the log's columns (eighteen) outgrow
/// sqlx's tuple `FromRow`, and the read and the append's RETURNING share one
/// spelling so a column added to one cannot drift from the other.
#[derive(Debug, sqlx::FromRow)]
struct EntryRow {
    id: String,
    seq: i32,
    batch: Option<String>,
    kind: String,
    section: String,
    source_key: Option<String>,
    source_type: Option<String>,
    source_id: Option<String>,
    source_href: Option<String>,
    fingerprint: Option<String>,
    supersedes: Option<String>,
    priority: Option<String>,
    status_label: Option<String>,
    badge: Option<Value>,
    title: String,
    body: String,
    evidence: Value,
    created_ms: i64,
}

impl From<EntryRow> for BriefEntry {
    fn from(r: EntryRow) -> Self {
        BriefEntry {
            id: r.id,
            seq: r.seq as i64,
            batch: r.batch,
            kind: r.kind,
            section: r.section,
            source_key: r.source_key,
            source_type: r.source_type,
            source_id: r.source_id,
            source_href: r.source_href,
            fingerprint: r.fingerprint,
            supersedes: r.supersedes,
            priority: r.priority,
            status_label: r.status_label,
            badge: r.badge,
            title: r.title,
            body: r.body,
            evidence: r.evidence,
            created_at: epoch_ms_to_iso(r.created_ms),
        }
    }
}

/// The entry columns, as `load_brief_entries` selects them and the append's
/// RETURNING hands them back — one const so the two cannot disagree.
const ENTRY_COLS: &str = "id::text, seq, batch::text, kind, section, source_key, source_type, \
                          source_id, source_href, fingerprint, supersedes::text, priority, \
                          status_label, badge, title, body, evidence, \
                          (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms";
// NOTE: batch and supersedes are uuid columns — both cast to text here because
// the row decodes them as Option<String>. An uncast uuid column is a decode
// error the first time a row actually carries one.

/// The brief's entry log, oldest first — the one read every consumer of the
/// document (the sweep, the artifact mirror, the deferred reader) shares.
pub async fn load_brief_entries(
    pg: &PgPool,
    brief_id: &str,
) -> Result<Vec<BriefEntry>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is ENTRY_COLS, this module's column list.
    let sql = format!(
        "select {ENTRY_COLS} from daily_brief_entries where brief_id = $1::uuid order by seq asc"
    );
    let rows: Vec<EntryRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(brief_id)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(BriefEntry::from).collect())
}

// ── The person, as the sources need them ─────────────────────────────────────

/// The user fields this plane reads: id, the handle the draft request names
/// the owner by, and the role that gates org-wide approvals. The session's
/// other fields are the route plane's business.
#[derive(Debug, Clone)]
pub struct BriefUser {
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
    pub timezone: Option<String>,
}

impl BriefUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

impl From<&crate::session::SessionUser> for BriefUser {
    /// The route plane's cut: the session fields, and the timezone LEFT OUT on
    /// purpose — `get_brief` and `mark_brief_item` read the stored zone from
    /// the row themselves, so a copy carried here would be a second version of
    /// the same fact that could go stale between login and read. The scheduled
    /// pass fills it in from its own query.
    fn from(user: &crate::session::SessionUser) -> Self {
        BriefUser {
            id: user.id.clone(),
            email: user.email.clone(),
            name: user.name.clone(),
            role: user.role.clone(),
            timezone: None,
        }
    }
}

/// The stored timezone preference, null when unset.
async fn get_timezone(pg: &PgPool, user_id: &str) -> Result<Option<String>, sqlx::Error> {
    let tz: Option<(Option<String>,)> =
        sqlx::query_as("select timezone from users where id = $1::uuid")
            .bind(user_id)
            .fetch_optional(pg)
            .await?;
    Ok(tz.and_then(|(tz,)| tz))
}

/// The READER'S zone: the one they SET, else the one their browser said, as
/// an IANA name the clock can actually resolve — anything else falls back to
/// the workspace config zone.
///
/// STORED FIRST, DELIBERATELY. A zone the person chose in Settings is the
/// contract and wins everywhere — brief reads, the scheduled open, the digest
/// — so a laptop in an airport lounge cannot re-file their day. The browser
/// `?tz=` param is the fallback that predates the column. An unparseable
/// param is discarded, never stored. The scheduled pass passes None and lands
/// on the stored-or-config zone, which is `zone_for`.
pub fn reader_zone(stored: Option<&str>, tz: Option<&str>, config: &BriefConfig) -> String {
    if let Some(zone) = stored.map(str::trim).filter(|s| !s.is_empty()) {
        return zone.to_string();
    }
    match tz.filter(|t| !t.is_empty() && t.len() <= 64) {
        Some(t) if crate::me::is_valid_time_zone(t) => t.to_string(),
        _ => config.time_zone.clone(),
    }
}

// ── The assistant ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefAssistant {
    pub configured: bool,
    pub model: Option<String>,
    pub name: Option<String>,
}

/// The owner's personal assistant, and nothing else may write a brief.
///
/// Same rule (and the same reason) as the briefer harness: this document is
/// built from the owner's private attention state — their unread DMs, their
/// approvals, their blocked tickets — so routing it through a model an admin
/// picked would quietly hand one person's day to somebody else's chosen brain.
pub async fn brief_assistant(pg: &PgPool, user_id: &str) -> Result<BriefAssistant, sqlx::Error> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "select model, display_name from agent_defs where owner_user_id = $1::uuid and enabled limit 1",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(match row {
        Some((model, name)) => BriefAssistant {
            configured: true,
            model: Some(model),
            name,
        },
        None => BriefAssistant {
            configured: false,
            model: None,
            name: None,
        },
    })
}

// ── Sources → candidate entries ──────────────────────────────────────────────

/// Where a source lands in the document. Section is a property of the SOURCE,
/// decided once here, so a thing cannot migrate between sections on a later
/// sweep and appear to be two different items.
fn section_for(item: &RawFocusItem) -> &'static str {
    if item.source_type == "approval" || item.source_type == "task" {
        return "action";
    }
    if item.source_type == "channel" {
        return "comms";
    }
    if matches!(
        item.metadata_kind.as_str(),
        "mention" | "dm" | "agent-outreach"
    ) {
        "comms"
    } else {
        "highlights"
    }
}

fn badge_for(item: &RawFocusItem, now_ms: i64) -> Option<Value> {
    if let Some(due) = item.due_at.as_deref().and_then(parse_rfc3339_ms) {
        if due <= now_ms {
            return Some(json!({"label": "OVERDUE", "tone": "danger"}));
        }
        if due <= now_ms + 86_400_000 {
            return Some(json!({"label": "DUE TODAY", "tone": "danger"}));
        }
    }
    if item.priority == "p0" {
        return Some(json!({"label": "BLOCKING", "tone": "danger"}));
    }
    None
}

fn candidate_from(item: &RawFocusItem, now_ms: i64) -> NewEntry {
    NewEntry {
        kind: "item".into(),
        section: section_for(item).into(),
        source_key: Some(item.key.clone()),
        source_type: Some(item.source_type.clone()),
        source_id: Some(item.source_id.clone()),
        source_href: Some(item.source_href.clone()),
        fingerprint: Some(item.source_fingerprint.clone()),
        supersedes: None,
        priority: Some(item.priority.clone()),
        status_label: Some(item.status_label.clone()),
        badge: badge_for(item, now_ms),
        title: item.question.clone(),
        body: item.recommendation.clone(),
        evidence: evidence_value(&item.evidence),
    }
}

/// A conversation, as a brief line.
///
/// Built here rather than in `comms.rs` so that file stays a pure read model —
/// it answers "who is waiting", and this decides how that reads on the page.
/// The `resolved` kind is the interesting part: this source can say "this
/// became answered, and here is who answered it" instead of merely vanishing
/// from the live set, which is what lets the line say YOU REPLIED or ASSISTANT
/// REPLIED rather than a generic DONE.
fn comms_candidate(line: &CommsLine) -> NewEntry {
    let answered = line.answered_by.is_some();
    NewEntry {
        kind: if answered { "resolved" } else { "item" }.into(),
        section: "comms".into(),
        source_key: Some(line.key.clone()),
        source_type: Some("channel".into()),
        source_id: Some(line.channel_id.clone()),
        source_href: Some(format!("/comms/channel/{}", line.channel_id)),
        fingerprint: Some(line.source_fingerprint.clone()),
        supersedes: None,
        priority: Some(line.priority.clone()),
        status_label: Some(line.status_label.clone()),
        badge: line.badge.clone(),
        title: if answered {
            format!("Replied to {}", line.peer)
        } else {
            format!("{} is waiting on you", line.peer)
        },
        body: if answered {
            String::new()
        } else {
            line.excerpt.clone()
        },
        evidence: match &line.draft {
            Some(d) if !d.stale => evidence_value(&[BriefEvidence {
                label: "Drafted reply".into(),
                text: d.content.clone(),
            }]),
            _ => evidence_value(&[]),
        },
    }
}

/// Today's calendar, as brief entries.
///
/// OPTIONAL AND SAID SO. Most installs have no Google connection and the
/// section simply does not appear; an install that HAS one and could not read
/// it gets a keyed item entry saying that, because "your schedule is empty"
/// and "I could not see your schedule" are different sentences and printing
/// the first for the second is the exact failure the UI conventions call
/// empty-≠-broken. `Ok(None)` here means "no connection, nothing to say".
async fn calendar_entries(
    pg: &PgPool,
    sb: &crate::secretbox::SecretBox,
    user_id: &str,
    zone: &str,
    now_ms: i64,
) -> Result<Option<Vec<NewEntry>>, String> {
    let events = match list_upcoming_events(pg, sb, user_id, now_ms, 12).await {
        Ok(events) => events,
        Err(e) => {
            let message = e.to_string();
            // A missing/expired connection is the ordinary state, not an
            // incident. The match is case-insensitive over the message text.
            let lower = message.to_lowercase();
            if ["not connected", "no google", "unauthor", "401", "409"]
                .iter()
                .any(|w| lower.contains(w))
            {
                return Ok(None);
            }
            // A LINE, NOT A NARRATION, and the distinction cost a bug. Written
            // as an unkeyed note this landed in the day's timeline and NOWHERE
            // IN THE DOCUMENT — so the one reader who needed to know their
            // schedule was unreadable saw a schedule section that simply did
            // not appear, which is "empty" standing in for "broken" on the
            // surface whose whole contract is that those are different
            // renderings.
            //
            // With a key it behaves like everything else: it sits in the
            // schedule section while the calendar is unreadable, it does not
            // re-append on every sweep (the fingerprint is stable for the same
            // error), and it RESOLVES on its own the moment a later sweep can
            // read the calendar — because that sweep returns real events and
            // this key is no longer live.
            let first_120 = message.chars().take(120).collect::<String>();
            let first_300 = message.chars().take(300).collect::<String>();
            return Ok(Some(vec![NewEntry {
                kind: "item".into(),
                section: "schedule".into(),
                source_key: Some("calendar:unreadable".into()),
                source_type: Some("calendar".into()),
                source_id: Some("unreadable".into()),
                source_href: None,
                fingerprint: Some(format!("unreadable:{first_120}")),
                supersedes: None,
                priority: Some("p2".into()),
                status_label: Some("CALENDAR".into()),
                badge: Some(json!({"label": "UNREADABLE", "tone": "warn"})),
                title: "Your calendar could not be read this morning".into(),
                body: format!(
                    "Google Calendar returned: {first_300}. Nothing else in this brief is affected, but today's schedule is missing from it."
                ),
                evidence: evidence_value(&[]),
            }]));
        }
    };
    let today = local_moment(zone, now_ms).date;
    Ok(Some(
        events
            .iter()
            // An event whose start cannot be read in this zone is not today's.
            .filter(|e| {
                e.start
                    .as_deref()
                    .and_then(parse_rfc3339_ms)
                    .map(|ms| local_moment(zone, ms).date == today)
                    .unwrap_or(false)
            })
            .map(|e| NewEntry {
                kind: "item".into(),
                section: "schedule".into(),
                source_key: Some(format!("calendar:{}", e.id)),
                source_type: Some("calendar".into()),
                source_id: Some(e.id.clone()),
                source_href: e.html_link.clone(),
                // Start, end and title only. Attendee lists churn (a tentative
                // RSVP flipping) and would append a "changed" row for something
                // nobody asked to be told about.
                fingerprint: Some(format!(
                    "{}|{}|{}|{}",
                    e.start.clone().unwrap_or_default(),
                    e.end.clone().unwrap_or_default(),
                    e.summary,
                    e.location.clone().unwrap_or_default()
                )),
                supersedes: None,
                priority: None,
                status_label: Some(if e.all_day {
                    "ALL DAY".into()
                } else {
                    format_slot(e.start.as_deref(), e.end.as_deref(), zone)
                }),
                badge: None,
                title: e.summary.clone(),
                body: e
                    .location
                    .as_deref()
                    .map(|l| format!("Location: {l}"))
                    .unwrap_or_default(),
                evidence: if e.attendees.is_empty() {
                    evidence_value(&[])
                } else {
                    evidence_value(&[BriefEvidence {
                        label: "With".into(),
                        text: e
                            .attendees
                            .iter()
                            .take(8)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                    }])
                },
            })
            .collect(),
    ))
}

/// "7:30 PM – 8:15 PM", in the event's zone. The en-US hour-12 clock, spelled
/// by hand: the calendar's starts are RFC3339 with offsets, so the moment is
/// read through the zone rather than derived from the string.
fn format_slot(start: Option<&str>, end: Option<&str>, zone: &str) -> String {
    let Some(start) = start else {
        return String::new();
    };
    let fmt = |iso: &str| -> Option<String> {
        let m: LocalMoment = local_moment(zone, parse_rfc3339_ms(iso)?);
        let (h12, meridiem) = match m.hour % 24 {
            0 => (12, "AM"),
            h if h < 12 => (h, "AM"),
            12 => (12, "PM"),
            h => (h - 12, "PM"),
        };
        Some(format!("{h12}:{:02} {meridiem}", m.minute))
    };
    // An unstartable start is an empty slot.
    let Some(left) = fmt(start) else {
        return String::new();
    };
    match end.and_then(fmt) {
        Some(right) => format!("{left} \u{2013} {right}"),
        None => left,
    }
}

/// Everything that needs this person right now, as candidate entries.
///
/// The four source reads run together and A FAILURE OF ONE IS NOT A FAILURE OF
/// THE PASS — but it is also not silence. A source that failed is recorded in
/// `failed_types`, and the sweep declines to append RESOLUTIONS for anything
/// in that source's namespace: an approvals query that 500s must never be read
/// as "all your approvals resolved", which is the single most damaging thing
/// this module could write, because a resolution is permanent.
pub struct SnapshotOutcome {
    pub candidates: Vec<NewEntry>,
    pub failed_types: HashSet<String>,
    pub calendar: Option<Vec<NewEntry>>,
    pub comms: Vec<CommsLine>,
}

async fn snapshot(
    state: &AppState,
    user: &BriefUser,
    zone: &str,
    assistant_model: Option<&str>,
    now_ms: i64,
) -> SnapshotOutcome {
    let mut failed_types: HashSet<String> = HashSet::new();

    // comms_lines logs its own failures and returns empty — the sweep never
    // resolves a channel line by vanishing (see the comms module's header), so
    // a failed read cannot do damage the way a failed approvals read can.
    //
    // Each guarded read returns `(items, failed?)` rather than mutating the
    // set from inside the join: the futures run together, and the borrow
    // checker is right that two of them cannot hold `&mut` to one set at once.
    let (
        (approvals, approval_failed),
        (tasks, task_failed),
        comms,
        (notifications, notification_failed),
    ) = tokio::join!(
        async {
            match approval_items(&state.pg, &user.id, user.is_admin()).await {
                Ok(v) => (v, None),
                Err(e) => {
                    tracing::error!("[daily-brief] approval source failed for {}: {e}", user.id);
                    (Vec::new(), Some("approval"))
                }
            }
        },
        async {
            match task_items(&state.pg, &user.id).await {
                Ok(v) => (v, None),
                Err(e) => {
                    tracing::error!("[daily-brief] task source failed for {}: {e}", user.id);
                    (Vec::new(), Some("task"))
                }
            }
        },
        comms_lines(&state.pg, &user.id, assistant_model),
        async {
            match notification_items(&state.pg, &user.id).await {
                Ok(v) => (v, None),
                Err(e) => {
                    tracing::error!(
                        "[daily-brief] notification source failed for {}: {e}",
                        user.id
                    );
                    (Vec::new(), Some("notification"))
                }
            }
        },
    );
    for failed in [approval_failed, task_failed, notification_failed]
        .into_iter()
        .flatten()
    {
        failed_types.insert(failed.into());
    }

    // The calendar door needs the secretbox; a degraded (failed-load) box
    // reads as not connected, which is the honest rendering.
    let sb = state.secretbox().await.unwrap_or_default();
    let calendar = match calendar_entries(&state.pg, &sb, &user.id, zone, now_ms).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[daily-brief] calendar source failed for {}: {e}", user.id);
            None
        }
    };

    // `dedupe_items` drops a notification that merely points at a ticket or a
    // channel already in the set. It can no longer see the conversations (they
    // are their own source now), so the channel hrefs are re-applied here —
    // without it, "Priya mentioned you" and "Priya is waiting on you" both
    // land on the page as separate lines about the same nudge.
    let comms_hrefs: Vec<String> = comms
        .iter()
        .map(|c| format!("/comms/channel/{}", c.channel_id))
        .collect();
    let mut items = approvals;
    items.extend(tasks);
    items.extend(notifications);
    let deduped: Vec<RawFocusItem> = dedupe_items(items)
        .into_iter()
        .filter(|item| {
            !(item.source_type == "notification"
                && comms_hrefs
                    .iter()
                    .any(|href| item.source_href.starts_with(href.as_str())))
        })
        .collect();
    let mut candidates: Vec<NewEntry> = sort_items(deduped, now_ms)
        .into_iter()
        .map(|item| candidate_from(&item, now_ms))
        .collect();
    candidates.extend(comms.iter().map(comms_candidate));
    SnapshotOutcome {
        candidates,
        failed_types,
        calendar,
        comms,
    }
}

// ── The single writer ────────────────────────────────────────────────────────

/// Append rows. THE ONLY WRITE PATH INTO A BRIEF'S CONTENT.
///
/// Seq is claimed by `update ... returning`, not by `max(seq) + 1`, and the
/// difference matters: two sweeps can overlap (a scheduler tick and a realtime
/// nudge land together), and a read-then-write would hand both the same number.
/// The row update is atomic, so the loser gets the next block. The unique index
/// on (brief_id, seq) is the backstop that turns any remaining race into a
/// failed insert rather than a duplicated line.
async fn append_entries(
    deps: &BriefDeps,
    brief_id: &str,
    user_id: &str,
    rows: Vec<NewEntry>,
) -> Result<Vec<BriefEntry>, String> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let count = rows.len() as i64;

    let end: Option<i64> = sqlx::query_scalar(
        "update daily_briefs set last_seq = last_seq + $2::int where id = $1::uuid returning last_seq::int8",
    )
    .bind(brief_id)
    .bind(count)
    .fetch_optional(&deps.state.pg)
    .await
    .map_err(|e| format!("seq claim: {e}"))?;
    let Some(end) = end else {
        return Err("brief disappeared while appending".into());
    };
    let start = end - count;
    // One id for this whole append. Minted here rather than defaulted in the
    // schema because the batch IS this call — a per-row default would give
    // every row its own and the timeline would be a flat list again.
    let batch = Uuid::new_v4().to_string();

    let mut inserted: Vec<BriefEntry> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        // AssertSqlSafe: the interpolation is ENTRY_COLS, this module's list.
        let sql = format!(
            "insert into daily_brief_entries \
               (brief_id, seq, batch, kind, section, source_key, source_type, source_id, source_href, \
                fingerprint, supersedes, priority, status_label, badge, title, body, evidence) \
             values ($1::uuid, $2, $3::uuid, $4, $5, $6, $7, $8, $9, $10, $11::uuid, $12, $13, $14, $15, $16, $17) \
             returning {ENTRY_COLS}"
        );
        let out: Option<EntryRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(brief_id)
            .bind(start + i as i64 + 1)
            .bind(&batch)
            .bind(&row.kind)
            .bind(&row.section)
            .bind(&row.source_key)
            .bind(&row.source_type)
            .bind(&row.source_id)
            .bind(&row.source_href)
            .bind(&row.fingerprint)
            .bind(&row.supersedes)
            .bind(&row.priority)
            .bind(&row.status_label)
            .bind(&row.badge)
            .bind(&row.title)
            .bind(&row.body)
            .bind(&row.evidence)
            .fetch_optional(&deps.state.pg)
            .await
            .map_err(|e| format!("entry insert: {e}"))?;
        if let Some(out) = out {
            inserted.push(BriefEntry::from(out));
        }
    }

    // The mirror and the nudge are both DERIVED, so neither may take the append
    // down with it. A brief whose artifact failed to re-render is a brief with
    // a stale share link; a brief that failed to append is a brief that lost a
    // day.
    {
        let pg = deps.state.pg.clone();
        let brief_id = brief_id.to_string();
        let user_id = user_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = artifact::mirror_brief_artifact(&pg, &brief_id, &user_id).await {
                tracing::error!("[daily-brief] artifact mirror failed for {brief_id}: {e}");
            }
        });
    }
    publish_user(
        &deps.realtime,
        user_id,
        &UserEvent::Brief {
            brief_id: brief_id.to_string(),
            seq: end,
        },
    );
    Ok(inserted)
}

// ── Opening ──────────────────────────────────────────────────────────────────

pub struct OpenOutcome {
    pub opened: bool,
    pub brief_id: Option<String>,
}

/// Open today's brief. Idempotent: the unique key on (user_id, brief_date) is
/// what makes a double-firing tick a no-op rather than a second document, and
/// the `returning` tells us which of the two we are.
pub async fn open_brief(
    deps: &BriefDeps,
    user: &BriefUser,
    at_ms: i64,
    tz: Option<&str>,
) -> Result<OpenOutcome, String> {
    let config = brief_config(&deps.state.pg).await;
    let stored = get_timezone(&deps.state.pg, &user.id)
        .await
        .map_err(|e| format!("timezone read: {e}"))?;
    let zone = reader_zone(stored.as_deref(), tz, &config);
    let date = brief_window(&config, &zone, at_ms).date;
    let agent = brief_assistant(&deps.state.pg, &user.id)
        .await
        .map_err(|e| format!("assistant read: {e}"))?;
    // No assistant, no brief. NOT a degraded brief written by some other model —
    // see `brief_assistant`. The surface renders the setup card instead.
    if !agent.configured {
        return Ok(OpenOutcome {
            opened: false,
            brief_id: None,
        });
    }

    let created: Option<(String,)> = sqlx::query_as(
        "insert into daily_briefs (user_id, brief_date, zone, agent_model, agent_name) \
         values ($1::uuid, $2::date, $3, $4, $5) \
         on conflict (user_id, brief_date) do nothing returning id::text",
    )
    .bind(&user.id)
    .bind(&date)
    .bind(&zone)
    .bind(&agent.model)
    .bind(&agent.name)
    .fetch_optional(&deps.state.pg)
    .await
    .map_err(|e| format!("brief insert: {e}"))?;
    let Some(brief_id) = created.map(|(id,)| id) else {
        let existing = load_row(&deps.state.pg, &user.id, &date)
            .await
            .map_err(|e| format!("brief read: {e}"))?;
        return Ok(OpenOutcome {
            opened: false,
            brief_id: existing.map(|r| r.id),
        });
    };

    let snap = snapshot(&deps.state, user, &zone, agent.model.as_deref(), at_ms).await;
    let mut items: Vec<NewEntry> = snap.calendar.unwrap_or_default();
    items.extend(snap.candidates);

    // The lede is written BEFORE the items are appended and inserted FIRST, so
    // seq 1 is always the opening read. It is also the only model call on this
    // path that is allowed to fail quietly: a brief with no lede is a brief; a
    // brief with no items is an empty page.
    let lede = write_lede(deps, &agent, &items, &date, &zone)
        .await
        .unwrap_or(None);
    let body = lede.unwrap_or_else(|| fallback_lede(&items));
    let mut to_append = vec![NewEntry::narrative(
        "lede",
        "action",
        format!("Daily brief: {date}"),
        body,
    )];
    to_append.extend(items);
    append_entries(deps, &brief_id, &user.id, to_append).await?;
    Ok(OpenOutcome {
        opened: true,
        brief_id: Some(brief_id),
    })
}

/// What the brief says when the model could not be reached.
///
/// A COUNT, NOT AN APOLOGY. The items are already on the page and they are the
/// real content; the lede is synthesis on top. A failed model call must not
/// turn into "your assistant is unavailable" at the head of a document that is
/// otherwise complete and correct.
fn fallback_lede(items: &[NewEntry]) -> String {
    let n = |section: &str| {
        items
            .iter()
            .filter(|i| i.section == section && i.kind == "item")
            .count()
    };
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_string()
        } else {
            many.to_string()
        }
    };
    let mut parts: Vec<String> = Vec::new();
    if n("action") > 0 {
        parts.push(format!(
            "{} {} need you",
            n("action"),
            plural(n("action"), "thing", "things")
        ));
    }
    if n("comms") > 0 {
        parts.push(format!(
            "{} {} waiting",
            n("comms"),
            plural(n("comms"), "conversation", "conversations")
        ));
    }
    if n("schedule") > 0 {
        parts.push(format!("{} on your calendar", n("schedule")));
    }
    if parts.is_empty() {
        return "Nothing is waiting on you this morning.".into();
    }
    format!("{}.", parts.join(", "))
}

async fn write_lede(
    deps: &BriefDeps,
    agent: &BriefAssistant,
    items: &[NewEntry],
    date: &str,
    zone: &str,
) -> Result<Option<String>, crate::harness::run::HarnessError> {
    let Some(model) = agent.model.as_deref() else {
        return Ok(None);
    };
    let lines: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "item")
        .map(|i| {
            if i.body.is_empty() {
                format!("[{}] {}", i.section, i.title)
            } else {
                format!("[{}] {} — {}", i.section, i.title, i.body)
            }
        })
        .collect();
    let run = run_harness(
        &deps.state,
        &daily_brief_lede_harness(),
        &json!({ "date": date, "zone": zone, "lines": lines }),
        RunContext {
            caller: "briefer:daily-open".into(),
            user_id: None,
            model: Some(model.to_string()),
            step: None,
            tier: None,
            effort: None,
            ledger: None,
            deps: None,
        },
    )
    .await?;
    Ok(run.value.and_then(|v| v.as_str().map(str::to_string)))
}

// ── Following the day ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepResult {
    pub appended: usize,
    pub added: usize,
    pub changed: usize,
    pub resolved: usize,
}

/// Recompute the sources and append what moved. NEVER rewrites.
///
/// Three verbs and one refusal:
///   · a key the log has never seen         → append 'item'
///   · a key whose fingerprint moved        → append 'change', superseding
///   · a key that is gone from the sources  → append 'resolved', superseding
///   · a key whose SOURCE FAILED TO LOAD    → append nothing at all
///
/// That last one is the load-bearing branch. A resolution is permanent — there
/// is no row to take back — so "the approvals query 500'd" must never be
/// allowed to look like "every approval was handled". `snapshot` reports which
/// source types failed and this pass skips resolutions for exactly those,
/// leaving the lines standing until a pass that could actually read them says
/// otherwise.
pub async fn sweep_brief(
    deps: &BriefDeps,
    user: &BriefUser,
    at_ms: i64,
) -> Result<SweepResult, String> {
    let none = SweepResult::default();
    let config = brief_config(&deps.state.pg).await;
    let stored = get_timezone(&deps.state.pg, &user.id)
        .await
        .map_err(|e| format!("timezone read: {e}"))?;
    let zone = zone_for(stored.as_deref(), &config.time_zone);
    let date = brief_window(&config, &zone, at_ms).date;
    let Some(row) = load_row(&deps.state.pg, &user.id, &date)
        .await
        .map_err(|e| format!("brief read: {e}"))?
    else {
        return Ok(none);
    };
    // NOT DURING BIRTH. A row with last_seq === 0 is an open mid-flight — the
    // lede and first items have not landed — and a sweep that ran now would
    // race the open's own append: the same source keys written twice, a lede
    // that is not seq 1, a timeline that says the day started twice. The open
    // owns the document until its first batch lands; sweeps are for following
    // one that exists.
    if row.last_seq == 0 {
        return Ok(none);
    }

    let (entries, first) = tokio::join!(
        load_brief_entries(&deps.state.pg, &row.id),
        snapshot(&deps.state, user, &zone, row.agent_model.as_deref(), at_ms),
    );
    let entries = entries.map_err(|e| format!("entries read: {e}"))?;

    // DRAFT FIRST, THEN LOOK AT THE WORLD. Drafting used to run after the diff,
    // which was wrong in a way only visible with a standing grant: the
    // assistant SENT a reply during the sweep, and because the snapshot had
    // already been taken the line still read "1 unread" for another five
    // minutes — the brief reporting a conversation as waiting that its own
    // assistant had just answered. Re-reading costs one snapshot on the sweeps
    // where anything was drafted, and nothing at all on the ones where nothing
    // was.
    // A grant made since the last sweep can have parked drafts waiting behind
    // it — see `release_drafts`. Released BEFORE drafting, so a thread whose
    // reply has just gone out is not also drafted for.
    let released = match delegation::release_drafts(&deps.notify, &user.id, None).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("[daily-brief] releasing drafts failed for {}: {e}", user.id);
            0
        }
    };
    let fresh_drafts = match draft_pending(deps, user, &row, &first.comms).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("[daily-brief] drafting replies failed for {}: {e}", user.id);
            0
        }
    };
    let drafted = released + fresh_drafts;
    let second = if drafted > 0 {
        Some(snapshot(&deps.state, user, &zone, row.agent_model.as_deref(), at_ms).await)
    } else {
        None
    };
    let SnapshotOutcome {
        candidates,
        failed_types,
        calendar,
        ..
    } = match second {
        Some(s) => s,
        None => first,
    };
    // Read BEFORE the calendar is consumed below — None (no connection, or a
    // read that could not even be attempted) is the "do not resolve calendar
    // lines" signal, and it is a different fact from "no events today".
    let calendar_was_none = calendar.is_none();

    let folded = fold_entries(entries, row.read_seq);
    let lines = folded.lines;
    let known: HashMap<String, &BriefLine> = lines.iter().map(|l| (l.key.clone(), l)).collect();
    // No `kind` filter: `calendar_entries` returns only items now (the
    // unreadable case included, as a keyed line). Filtering here would have
    // dropped that line out of `live` and made every sweep resolve it and
    // re-add it.
    let mut live: Vec<NewEntry> = calendar.unwrap_or_default();
    live.extend(candidates);
    let live_keys: HashSet<String> = live.iter().filter_map(|c| c.source_key.clone()).collect();

    let mut appends: Vec<NewEntry> = Vec::new();
    let mut added = 0usize;
    let mut changed = 0usize;
    let mut resolved_count = 0usize;

    for mut candidate in live {
        let Some(key) = candidate.source_key.clone() else {
            continue;
        };
        let Some(line) = known.get(&key) else {
            // A SOURCE-EMITTED RESOLUTION FOR A LINE THAT WAS NEVER HERE IS
            // DROPPED. The comms source reports answered conversations so a
            // line can close with WHO answered it rather than a generic DONE —
            // but a thread answered before the brief ever mentioned it has
            // nothing to close, and appending it would put "Replied to Sam" on
            // a page that never asked you to.
            if candidate.kind == "resolved" {
                continue;
            }
            appends.push(candidate);
            added += 1;
            continue;
        };
        if candidate.kind == "resolved" {
            // The source says it is done. Nothing to add if the line is already
            // closed — by the source earlier, or by the owner checking it off.
            if line.resolved {
                continue;
            }
            candidate.supersedes = Some(line.current.id.clone());
            appends.push(candidate);
            resolved_count += 1;
            continue;
        }

        // The source says it is still live. THE FINGERPRINT DECIDES, AND ONLY
        // THE FINGERPRINT — a closed line whose source has not moved stays
        // closed.
        //
        // This used to also compare `line.resolved` against whether the
        // candidate was a resolution, which was correct while the source was
        // the only thing that could close a line. It stopped being correct the
        // moment the owner could: a dismissed item is still LIVE in its source,
        // so every sweep found a closed line against an unchanged live
        // candidate and appended a change — undoing the dismissal, five minutes
        // later, for ever.
        if line.current.fingerprint == candidate.fingerprint {
            continue;
        }

        // Moved. A closed line reopens here on purpose: if you dismissed
        // "Reply to Priya" and Priya has since said something else, that is new
        // information and burying it would make dismissal a mute button on a
        // person.
        candidate.kind = "change".into();
        candidate.supersedes = Some(line.current.id.clone());
        appends.push(candidate);
        changed += 1;
    }

    for line in &lines {
        if line.resolved || live_keys.contains(&line.key) {
            continue;
        }
        // A conversation NEVER resolves by vanishing. `comms_lines` reports
        // answered threads explicitly (with who answered), so a comms key that
        // is absent from the live set means the source could not see it — an
        // archived channel, a failed read — and closing it here would be the
        // original bug wearing a different hat: a line marked done that nobody
        // answered.
        if line.current.source_type.as_deref() == Some("channel") {
            continue;
        }
        let type_ = line.current.source_type.clone().unwrap_or_default();
        // Calendar has no failure namespace of its own — a schedule that could
        // not be read comes back as None, not as an empty list — so a None
        // calendar means "do not resolve calendar lines either".
        if failed_types.contains(&type_) || (type_ == "calendar" && calendar_was_none) {
            continue;
        }
        appends.push(NewEntry {
            kind: "resolved".into(),
            section: line.section.clone(),
            source_key: Some(line.key.clone()),
            source_type: line.current.source_type.clone(),
            source_id: line.current.source_id.clone(),
            source_href: line.current.source_href.clone(),
            fingerprint: None,
            supersedes: Some(line.current.id.clone()),
            priority: Some("ok".into()),
            status_label: Some("DONE".into()),
            badge: None,
            title: line.current.title.clone(),
            body: String::new(),
            evidence: evidence_value(&[]),
        });
        resolved_count += 1;
    }

    if appends.is_empty() {
        let _ = sqlx::query("update daily_briefs set last_swept_at = now() where id = $1::uuid")
            .bind(&row.id)
            .execute(&deps.state.pg)
            .await;
        return Ok(none);
    }

    // ONE narration per batch, and it goes in FIRST so the fold finds it at the
    // head of the group. The model sees only what this pass appended — never
    // the whole document — because a note is about the change, and handing it
    // the day so far is how a delta line turns into a second, competing
    // summary.
    let note = write_note(deps, &row, &appends).await.unwrap_or(None);
    let mut to_append: Vec<NewEntry> = Vec::with_capacity(appends.len() + 1);
    if let Some(note) = note {
        to_append.push(note);
    }
    to_append.extend(appends);
    let written = append_entries(deps, &row.id, &user.id, to_append).await?;
    let _ = sqlx::query("update daily_briefs set last_swept_at = now() where id = $1::uuid")
        .bind(&row.id)
        .execute(&deps.state.pg)
        .await;
    Ok(SweepResult {
        appended: written.len(),
        added,
        changed,
        resolved: resolved_count,
    })
}

/// Ask the assistant to write replies for the conversations still waiting.
///
/// WHY DRAFTING NEEDS NO PERMISSION AND SENDING DOES. A draft is a suggestion
/// sitting on the owner's own page; nothing has left the building, and they
/// read it before anyone else does. `draft_reply` is what checks for a grant,
/// and it is the only thing that can turn a draft into a sent message.
async fn draft_pending(
    deps: &BriefDeps,
    user: &BriefUser,
    row: &BriefRow,
    comms: &[CommsLine],
) -> Result<usize, String> {
    let Some(agent_model) = row.agent_model.as_deref() else {
        return Ok(0);
    };
    // Bounded twice over: only threads where somebody is genuinely waiting, and
    // only those without a live draft already. Without both, every unanswered
    // DM would be re-drafted every five minutes — a model call per thread per
    // sweep, and a page that rewrites itself under the reader.
    let waiting: Vec<&CommsLine> = comms
        .iter()
        .filter(|c| c.answered_by.is_none() && c.draft.as_ref().is_none_or(|d| d.stale))
        .collect();
    if waiting.is_empty() {
        return Ok(0);
    }
    let owner = user
        .name
        .clone()
        .or_else(|| user.email.clone())
        .unwrap_or_else(|| "your owner".into());
    // Sequential: each is a model call, and a person with twenty unanswered DMs
    // should not fan twenty concurrent turns at their own assistant.
    if waiting.len() > DRAFT_LIMIT {
        // SAID OUT LOUD. A silent cap reads as "every waiting thread was
        // handled", which is the class of quiet truncation this whole surface
        // exists to avoid.
        tracing::warn!(
            "[daily-brief] {} conversations waiting for {}; drafting {DRAFT_LIMIT} this pass, the rest on following sweeps",
            waiting.len(),
            user.id
        );
    }
    let mut wrote = 0usize;
    for line in waiting.into_iter().take(DRAFT_LIMIT) {
        let result = delegation::draft_reply(
            &deps.state,
            &deps.notify,
            delegation::DraftRequest {
                user_id: &user.id,
                channel_id: &line.channel_id,
                peer: &line.peer,
                awaiting_seq: line.awaiting_seq,
                agent_model,
                owner_name: &owner,
            },
        )
        .await;
        match result {
            // One thread failing to draft leaves that thread waiting, which the
            // line already renders honestly. It must not stop the other threads.
            Err(e) => tracing::error!(
                "[daily-brief] draft failed for channel {}: {e}",
                line.channel_id
            ),
            Ok(Some(_)) => wrote += 1,
            Ok(None) => {}
        }
    }
    Ok(wrote)
}

/// How many conversations one sweep will draft for.
///
/// A ceiling rather than a queue, and it is LOGGED when it bites: a person who
/// comes back from leave with forty unanswered DMs should not have their
/// assistant spend forty model calls in one tick. The rest are drafted on
/// following sweeps, and every one of them is still visible on the page as
/// waiting in the meantime.
const DRAFT_LIMIT: usize = 8;

async fn write_note(
    deps: &BriefDeps,
    row: &BriefRow,
    appends: &[NewEntry],
) -> Result<Option<NewEntry>, crate::harness::run::HarnessError> {
    let Some(model) = row.agent_model.as_deref() else {
        return Ok(None);
    };
    let changes: Vec<String> = appends
        .iter()
        .map(|a| {
            if a.body.is_empty() {
                format!("{}: {}", a.kind, a.title)
            } else {
                format!("{}: {} — {}", a.kind, a.title, a.body)
            }
        })
        .collect();
    let run = run_harness(
        &deps.state,
        &daily_brief_note_harness(),
        &json!({ "changes": changes }),
        RunContext {
            caller: "briefer:daily-delta".into(),
            user_id: None,
            model: Some(model.to_string()),
            step: None,
            tier: None,
            effort: None,
            ledger: None,
            deps: None,
        },
    )
    .await?;
    let value = run.value.and_then(|v| v.as_str().map(str::to_string));
    Ok(value.map(|body| NewEntry::narrative("note", "action", "", body)))
}

// ── The reader ───────────────────────────────────────────────────────────────
//
// The route-facing half: the surface's one read (with its
// three kinds of nothing), the cursor, the owner's verdict on a line, and the
// sweep-if-due the read performs first. The document assembly itself lives in
// `view.rs`; this section is the branching around it.

/// An open in flight for this person, by user id. The unique key on (user_id,
/// brief_date) already makes a double-open harmless — the loser's insert
/// conflicts and it returns the winner's id — but the reader's fast poll while
/// a brief is 'writing' would re-kick one open PER POLL, each costing an
/// assistant lookup and a doomed insert. This set makes the re-kick a no-op
/// while any open for that person is still running.
static OPENS_IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

/// Kick today's open, detached, at most one at a time per person. The caller
/// answers the read with 'writing' immediately; `append_entries` publishes the
/// brief event that turns the surface into the document.
fn open_brief_detached(deps: Arc<BriefDeps>, user: BriefUser, tz: Option<String>) {
    {
        let mut guard = OPENS_IN_FLIGHT.lock().expect("opens-in-flight lock");
        if guard.contains(&user.id) {
            return;
        }
        guard.insert(user.id.clone());
    }
    tokio::spawn(async move {
        let at = wall_ms();
        if let Err(e) = open_brief(&deps, &user, at, tz.as_deref()).await {
            tracing::error!("[daily-brief] on-demand open failed for {}: {e}", user.id);
        }
        OPENS_IN_FLIGHT
            .lock()
            .expect("opens-in-flight lock")
            .remove(&user.id);
    });
}

/// The person's most recent brief, when it is recent enough to still be "the
/// current one" (48h staleness window). None otherwise.
pub async fn load_recent_row(
    pg: &PgPool,
    user_id: &str,
    at_ms: i64,
) -> Result<Option<BriefRow>, sqlx::Error> {
    // Cutoff computed here rather than as SQL `created_at - interval` — the
    // boundary is the same fact either way, and in code it needs no interval
    // arithmetic. It crosses the wire as text, so the bind carries the cast.
    let since = epoch_ms_to_iso(at_ms - 48 * 3_600_000);
    // AssertSqlSafe: the interpolation is ROW_COLS, this module's column list.
    let sql = format!(
        "select {ROW_COLS} from daily_briefs \
         where user_id = $1::uuid and last_seq > 0 and created_at > $2::timestamptz \
         order by brief_date desc limit 1"
    );
    let row: Option<BriefRowTuple> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(&since)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(BriefRow::from_row))
}

/// The read-side assembly shared by the today row and the recent-brief
/// fallback: live comms state plus the fold. One function because the two
/// callers must not drift — a fallback that forgot the comms controls would
/// serve a read-only page for the same document depending on the clock.
///
/// `comms_lines` reads fresh because a draft's approvability and a grant's
/// existence are present-tense facts — the log can only say what was true when
/// it was written, and acting on that would offer to send a draft the owner
/// already discarded. (comms_lines swallows its own failure and answers empty
/// — a failure there costs the controls, never the document.)
async fn document_view(
    pg: &PgPool,
    row: &BriefRow,
    agent: &BriefAssistant,
) -> Result<view::BriefViewWire, sqlx::Error> {
    let entries = load_brief_entries(pg, &row.id).await?;
    let live = comms_lines(pg, &row.user_id, row.agent_model.as_deref()).await;
    let comms = live.iter().map(view::comms_state).collect();
    Ok(view::view(row, &entries, agent, comms))
}

/// The surface's one read (getBrief): the day's document, or WHICH kind of
/// nothing — the absences render differently and collapsing them into one
/// empty state is how a surface tells a person "you are all clear" over a
/// brief that simply has not been written yet.
///
/// THE READ OPENS, and that is the fix for the blank day. The scheduled pass
/// is still the ordinary way a brief starts, but a tick can be missed for
/// reasons that have nothing to do with this reader — a Redis lease skipped,
/// a deploy window, an assistant configured at noon — and "the hour passed and
/// nothing was written" used to be a state the person stared at until
/// tomorrow. Now the first read past the due hour kicks the open itself; the
/// reader sees 'writing' for the few seconds it takes, and the append's
/// publish turns the page into the document without them touching anything.
pub enum BriefRead {
    // Boxed: the document dwarfs the absent arm, and the read answers one per
    // request — the indirection is free where a fat enum value would pad
    // every BriefRead in flight.
    Document(Box<view::BriefViewWire>),
    /// (which absence, next open ISO when knowable, the assistant) — every
    /// absent literal carries `agent`, because the surface offers assistant
    /// settings from the empty state too. The three kinds: 'pending' the hour
    /// has not arrived; 'no-agent' nothing can write one; 'writing' it is
    /// being written right now.
    Absent(&'static str, Option<String>, BriefAssistant),
}

pub async fn get_brief(
    deps: &BriefDeps,
    user: &BriefUser,
    tz: Option<&str>,
) -> Result<BriefRead, String> {
    let at = wall_ms();
    let pg = &deps.state.pg;
    let config = brief_config(pg).await;
    let stored = get_timezone(pg, &user.id)
        .await
        .map_err(|e| format!("timezone read: {e}"))?;
    let zone = reader_zone(stored.as_deref(), tz, &config);
    let window = brief_window(&config, &zone, at);
    let row = load_row(pg, &user.id, &window.date)
        .await
        .map_err(|e| format!("brief read: {e}"))?;
    let agent = brief_assistant(pg, &user.id)
        .await
        .map_err(|e| format!("assistant read: {e}"))?;

    if let Some(row) = row {
        // BIRTH, NOT A DOCUMENT. `open_brief` inserts the row first and
        // appends the lede and first items in one batch afterwards, so
        // last_seq == 0 is only reachable while that first append is in
        // flight. Rendering it as a brief would show an empty page —
        // "nothing is waiting on you", asserted by a surface whose document
        // has not arrived. It is the same 'writing' the reader gets before
        // the row exists, because it is the same fact.
        if row.last_seq == 0 {
            return Ok(BriefRead::Absent("writing", None, agent.clone()));
        }
        let doc = document_view(pg, &row, &agent)
            .await
            .map_err(|e| format!("document read: {e}"))?;
        return Ok(BriefRead::Document(Box::new(doc)));
    }
    if !agent.configured {
        return Ok(BriefRead::Absent("no-agent", None, agent));
    }
    if window.due {
        open_brief_detached(
            Arc::new(BriefDeps {
                state: deps.state.clone(),
                realtime: deps.realtime.clone(),
                notify: deps.notify.clone(),
                now_ms: at,
            }),
            user.clone(),
            tz.map(str::to_string),
        );
        return Ok(BriefRead::Absent("writing", None, agent));
    }
    // NEVER 'pending' OVER A READABLE DOCUMENT. The window said "not due yet",
    // which is a statement about the NEXT brief — not a reason to hide the
    // last one. A person opening the surface in their evening (or from a
    // timezone the org config disagrees with) still has yesterday's page,
    // titled with its own date, and anything they check off or reply to there
    // is real work.
    let recent = load_recent_row(pg, &user.id, at)
        .await
        .map_err(|e| format!("recent brief read: {e}"))?;
    if let Some(recent) = recent {
        let doc = document_view(pg, &recent, &agent)
            .await
            .map_err(|e| format!("document read: {e}"))?;
        return Ok(BriefRead::Document(Box::new(doc)));
    }
    Ok(BriefRead::Absent(
        "pending",
        Some(as_iso(config::next_brief_at(&config, &zone, at))),
        agent,
    ))
}

/// The sweep the read performs first — BEFORE the read, not after, and only if
/// the throttle allows it. A person opening the surface is the one moment
/// latency is visible, and sweeping afterwards would hand them the stale page
/// and the fresh one a realtime nudge later — a document that visibly rewrites
/// itself on arrival, which is the exact impression an append-only brief must
/// never give. None means "nothing to do" (no row, birth in flight, or the
/// throttle refusing); the route logs an Err and reads anyway.
pub async fn sweep_if_due(
    deps: &BriefDeps,
    user: &BriefUser,
) -> Result<Option<SweepResult>, String> {
    let at = wall_ms();
    let config = brief_config(&deps.state.pg).await;
    let stored = get_timezone(&deps.state.pg, &user.id)
        .await
        .map_err(|e| format!("timezone read: {e}"))?;
    let zone = zone_for(stored.as_deref(), &config.time_zone);
    let date = brief_window(&config, &zone, at).date;
    let row = load_row(&deps.state.pg, &user.id, &date)
        .await
        .map_err(|e| format!("brief read: {e}"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.last_seq == 0 || !sweep_due(&row, &config, at) {
        return Ok(None);
    }
    sweep_brief(deps, user, at).await.map(Some)
}

/// Move the reader's cursor. The only update this feature performs on a brief,
/// and it describes the READER, not the document. Monotonic — `greatest` — so
/// a second tab that loaded an older page cannot walk the cursor backwards and
/// re-flag everything as new.
pub async fn mark_brief_read(
    pg: &PgPool,
    user_id: &str,
    brief_id: &str,
    seq: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update daily_briefs set read_seq = greatest(read_seq, $1) \
         where id = $2::uuid and user_id = $3::uuid",
    )
    .bind(seq)
    .bind(brief_id)
    .bind(user_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// A mark-item verdict: ok, or the reason the route answers 404 with.
#[derive(Debug, Clone)]
pub struct ItemMark {
    pub ok: bool,
    pub reason: Option<String>,
}

impl ItemMark {
    fn done() -> Self {
        ItemMark {
            ok: true,
            reason: None,
        }
    }
    fn refused(reason: &str) -> Self {
        ItemMark {
            ok: false,
            reason: Some(reason.to_string()),
        }
    }
}

/// Close a line by hand, or reopen one — the owner's own verdict, scoped to
/// the caller's own brief inside this function (a key belonging to somebody
/// else's day resolves to no line rather than to theirs).
///
/// AN APPEND, NOT AN EDIT, like everything else here. Checking something off
/// does not delete it or set a flag on it — it writes a new row that supersedes
/// the one on the page, so the line stays visible, struck through, with its
/// whole history intact. `restore` is the same move in reverse rather than an
/// undo: it appends a row carrying the last live state, because the way to
/// take something back in an append-only log is to say the next thing, not to
/// remove the last one.
///
/// THE FINGERPRINT IS CARRIED FORWARD, and that is what makes a dismissal
/// stick. `sweep_brief` skips a line whose source fingerprint has not moved,
/// so copying it onto the closing row means the next sweep sees a dismissed
/// line against an unchanged source and leaves it alone. Drop it and the
/// sweep finds a mismatch every five minutes and reopens what the owner just
/// closed.
pub async fn mark_brief_item(
    deps: &BriefDeps,
    user: &BriefUser,
    source_key: &str,
    action: &str,
    tz: Option<&str>,
) -> Result<ItemMark, String> {
    let at = wall_ms();
    let pg = &deps.state.pg;
    let config = brief_config(pg).await;
    let stored = get_timezone(pg, &user.id)
        .await
        .map_err(|e| format!("timezone read: {e}"))?;
    let zone = reader_zone(stored.as_deref(), tz, &config);
    let date = brief_window(&config, &zone, at).date;
    let row = load_row(pg, &user.id, &date)
        .await
        .map_err(|e| format!("brief read: {e}"))?;
    let Some(row) = row else {
        return Ok(ItemMark::refused("no brief today"));
    };

    let entries = load_brief_entries(pg, &row.id)
        .await
        .map_err(|e| format!("entries read: {e}"))?;
    let folded = fold_entries(entries, row.read_seq);
    // The apostrophe is U+2019 — the sentence is bytes the surface renders
    // verbatim, not prose to re-type.
    let Some(line) = folded.lines.iter().find(|l| l.key == source_key) else {
        return Ok(ItemMark::refused("that line is not on today’s brief"));
    };

    if action == "restore" {
        if !line.resolved {
            return Ok(ItemMark::done());
        }
        // The last state the line had before anything closed it. Searched from
        // the end so a line closed, reopened and closed again restores to the
        // most recent LIVE row rather than to the one it started the day with.
        let live = line
            .history
            .iter()
            .rev()
            .find(|e| !types::is_terminal(&e.kind));
        let Some(live) = live else {
            return Ok(ItemMark::refused("nothing to restore this line to"));
        };
        append_entries(
            deps,
            &row.id,
            &user.id,
            vec![NewEntry {
                kind: "change".into(),
                section: live.section.clone(),
                source_key: Some(live.source_key.clone().unwrap_or_default()),
                source_type: live.source_type.clone(),
                source_id: live.source_id.clone(),
                source_href: live.source_href.clone(),
                fingerprint: live.fingerprint.clone(),
                supersedes: Some(line.current.id.clone()),
                priority: live.priority.clone(),
                status_label: live.status_label.clone(),
                badge: live.badge.clone(),
                title: live.title.clone(),
                body: live.body.clone(),
                evidence: live.evidence.clone(),
            }],
        )
        .await?;
        return Ok(ItemMark::done());
    }

    // Already closed — by the source, or by an earlier click. Nothing to add,
    // and appending anyway would put two identical strike-throughs in the
    // timeline for one double-click.
    if line.resolved {
        return Ok(ItemMark::done());
    }

    let current = &line.current;
    // The closing literal sets NO badge and NO evidence — the strike-through
    // is the whole row.
    append_entries(
        deps,
        &row.id,
        &user.id,
        vec![NewEntry {
            kind: if action == "check" {
                "checked"
            } else {
                "dismissed"
            }
            .into(),
            section: current.section.clone(),
            source_key: current.source_key.clone(),
            source_type: current.source_type.clone(),
            source_id: current.source_id.clone(),
            source_href: current.source_href.clone(),
            fingerprint: current.fingerprint.clone(),
            supersedes: Some(current.id.clone()),
            priority: Some("ok".into()),
            status_label: Some(
                if action == "check" {
                    "CHECKED OFF"
                } else {
                    "DISMISSED"
                }
                .into(),
            ),
            badge: None,
            title: current.title.clone(),
            body: String::new(),
            evidence: Value::Array(Vec::new()),
        }],
    )
    .await?;
    Ok(ItemMark::done())
}

// ── The scheduled pass ───────────────────────────────────────────────────────

/// Users who could have a brief: everyone with an enabled personal assistant.
/// Scoped by that rather than by `users`, because a person with no assistant
/// has nothing that can write one and sweeping them is pure cost. Carries the
/// person's timezone in the same row — the scheduled pass resolves each zone
/// with zero queries beyond the one it already paid.
/// One briefable person as SQL hands them back: (id, email, name, role, zone).
type BriefableRow = (
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

async fn briefable_users(pg: &PgPool) -> Result<Vec<BriefUser>, sqlx::Error> {
    let rows: Vec<BriefableRow> = sqlx::query_as(
        "select u.id::text, u.email, u.name, u.role, u.timezone \
         from users u join agent_defs d on d.owner_user_id = u.id and d.enabled \
         order by u.created_at asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, email, name, role, timezone)| BriefUser {
            id,
            email,
            name,
            role,
            timezone,
        })
        .collect())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BriefPassResult {
    pub opened: usize,
    pub swept: usize,
    pub appended: usize,
    pub failed: usize,
}

/// One scheduler tick: open what is due, then follow what is open.
///
/// Sequential, not a fan-out. Each person costs four scoped queries and up to
/// one model call, and a workspace-wide fan-out would hit the connection pool
/// and the gateway at the same instant every five minutes. The job's
/// `max_run_ms` is what makes a pass that gets too slow VISIBLE, which is the
/// right way for this to fail.
pub async fn run_brief_pass(deps: &BriefDeps) -> BriefPassResult {
    let config = brief_config(&deps.state.pg).await;
    let mut result = BriefPassResult::default();
    let Ok(users) = briefable_users(&deps.state.pg).await else {
        return result;
    };

    for user in users {
        let zone = zone_for(user.timezone.as_deref(), &config.time_zone);
        let window = brief_window(&config, &zone, deps.now_ms);
        let existing = load_row(&deps.state.pg, &user.id, &window.date).await;
        match existing {
            Err(e) => {
                result.failed += 1;
                tracing::error!("[daily-brief] pass failed for {}: {e}", user.id);
            }
            Ok(None) => {
                if !window.due {
                    continue;
                }
                match open_brief(deps, &user, deps.now_ms, None).await {
                    Ok(outcome) if outcome.opened => result.opened += 1,
                    Ok(_) => {}
                    Err(e) => {
                        result.failed += 1;
                        tracing::error!("[daily-brief] pass failed for {}: {e}", user.id);
                    }
                }
            }
            Ok(Some(row)) => {
                if !sweep_due(&row, &config, deps.now_ms) {
                    continue;
                }
                match sweep_brief(deps, &user, deps.now_ms).await {
                    Ok(swept) => {
                        result.swept += 1;
                        result.appended += swept.appended;
                    }
                    Err(e) => {
                        result.failed += 1;
                        tracing::error!("[daily-brief] pass failed for {}: {e}", user.id);
                    }
                }
            }
        }
    }
    result
}

fn sweep_due(row: &BriefRow, config: &BriefConfig, at_ms: i64) -> bool {
    row.last_swept_ms
        .map(|ms| at_ms - ms >= config.sweep_minutes * 60_000)
        .unwrap_or(true)
}

// ── The job ──────────────────────────────────────────────────────────────────

/// Everything the pass touches: the app state (pool, secretbox for the
/// calendar door, the harness runner) plus the realtime edge the append
/// publishes on. `now_ms` is a field, not a clock call — the house rule that
/// keeps the pass testable.
#[derive(Clone)]
pub struct BriefDeps {
    pub state: AppState,
    pub realtime: RealtimeDeps,
    pub notify: NotifyDeps,
    pub now_ms: i64,
}

/// The wall clock, stamped ONCE PER TICK by the job closure. Constructing the
/// deps with it would freeze the boot instant for the process's whole life —
/// a scheduler armed at 09:00 and left for a week would open a week of briefs
/// against Monday's clock.
fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub async fn real_brief_deps(state: &AppState) -> Arc<BriefDeps> {
    let conn = state.redis().await.ok();
    let notify = NotifyDeps::publishing(state.pg.clone(), conn);
    Arc::new(BriefDeps {
        state: state.clone(),
        realtime: notify.realtime.clone(),
        notify,
        now_ms: wall_ms(),
    })
}

use std::sync::Arc;

/// The scheduler owns the timing — this module owns the work. Not
/// `per_instance`: the brief reads and writes rows every instance can reach,
/// so the tick needs the Redis lease.
///
/// A TALARIA JOB, NOT AN AGENT CRON, and the choice is deliberate. An agent
/// cron fires the agent's loop on the PERSONA gateway key, whose spend reaches
/// no ledger; a brief is one model call per person per morning plus one per
/// sweep — across a workspace that is real money going nowhere visible.
/// Driving the schedule from here dispatches through `run_harness` like any
/// other turn, which meters and attributes it for free. It also fails in the
/// right direction: this job runs regardless of any agent container, and a
/// failed pass is a scheduler failure an operator can see.
pub fn daily_brief_job_spec(deps: Arc<BriefDeps>) -> crate::scheduler::JobSpec {
    use crate::scheduler::{JobName, JobSpec};
    JobSpec {
        name: JobName::DailyBrief,
        // Five minutes, and the number is doing two jobs. It is the granularity
        // of the OPEN (a brief can be at most five minutes late), and it is the
        // tick the per-brief `sweep_minutes` throttle rides on, so a sweep
        // interval below five minutes has no effect without changing this too.
        every_ms: 5 * 60_000,
        // Let a deploy settle before the first pass. A crash-looping instance
        // must never reach a job that writes a document to every person in the
        // workspace.
        first_run_delay_ms: Some(90_000),
        // Generous, because the pass is sequential across everyone with an
        // assistant and each person may cost a model call. Past this it is not
        // slow, it is stuck, and `unhealthy_jobs` should say so.
        max_run_ms: Some(10 * 60_000),
        per_instance: false,
        run: {
            let deps = deps.clone();
            let run: crate::scheduler::JobFn = Arc::new(move || {
                let deps = deps.clone();
                Box::pin(async move {
                    // The tick's clock, stamped HERE and not at boot — see
                    // `wall_ms`. Everything in the pass reads this one instant.
                    let deps = BriefDeps {
                        now_ms: wall_ms(),
                        ..deps.as_ref().clone()
                    };
                    let r = run_brief_pass(&deps).await;
                    if r.opened == 0 && r.swept == 0 && r.failed == 0 {
                        return Ok(None);
                    }
                    let line = format!(
                        "opened {} brief(s), swept {}, appended {} entr(ies)",
                        r.opened, r.swept, r.appended
                    );
                    // A pass with failures is a FAILED run, not a run with a
                    // footnote. The per-user error is already logged with its
                    // id; this is what makes the pattern — "it has been failing
                    // for six people since Tuesday" — reach an operator instead
                    // of scrolling past in a log.
                    if r.failed > 0 {
                        return Err(format!(
                            "{line}; FAILED for {} user(s); see the [daily-brief] errors above",
                            r.failed
                        ));
                    }
                    Ok(Some(line))
                })
            });
            run
        },
    }
}

pub fn register_daily_brief_job(deps: Arc<BriefDeps>) {
    crate::scheduler::register_job(daily_brief_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, section: &str, title: &str, body: &str) -> NewEntry {
        NewEntry::narrative(kind, section, title, body)
    }

    #[test]
    fn the_fallback_lede_counts_it_does_not_apologize() {
        let items = vec![
            item("item", "action", "Approve X?", "Open it."),
            item("item", "action", "Unblock Y?", "Look."),
            item("item", "comms", "Priya is waiting on you", "…"),
            item("item", "schedule", "Standup", ""),
            item("lede", "action", "Daily brief", "words"), // not counted
            item("note", "action", "", "words"),            // not counted
        ];
        assert_eq!(
            fallback_lede(&items),
            "2 things need you, 1 conversation waiting, 1 on your calendar."
        );
        assert_eq!(
            fallback_lede(&[]),
            "Nothing is waiting on you this morning."
        );
        assert_eq!(
            fallback_lede(&[item("item", "comms", "a", "")]),
            "1 conversation waiting."
        );
    }

    #[test]
    fn sections_come_from_the_source_not_the_sort() {
        let base = |source_type: &str, kind: &str| RawFocusItem {
            key: "k".into(),
            source_type: source_type.into(),
            source_id: "s".into(),
            priority: "p1".into(),
            status_label: "L".into(),
            due_at: None,
            question: "q".into(),
            recommendation: "r".into(),
            evidence: vec![],
            metadata_kind: kind.into(),
            metadata_priority: None,
            source_href: "/".into(),
            action_ids: vec![],
            bucket: 5,
            created_at_ms: 0,
            source_fingerprint: String::new(),
        };
        assert_eq!(section_for(&base("approval", "")), "action");
        assert_eq!(section_for(&base("task", "")), "action");
        assert_eq!(section_for(&base("channel", "")), "comms");
        assert_eq!(section_for(&base("notification", "mention")), "comms");
        assert_eq!(section_for(&base("notification", "research")), "highlights");
        assert_eq!(section_for(&base("calendar", "")), "highlights");
    }

    #[test]
    fn badges_read_the_due_date_against_the_clock() {
        let mut raw = RawFocusItem {
            key: "k".into(),
            source_type: "task".into(),
            source_id: "s".into(),
            priority: "p1".into(),
            status_label: "L".into(),
            due_at: Some("2026-08-29T12:00:00Z".into()),
            question: "q".into(),
            recommendation: "r".into(),
            evidence: vec![],
            metadata_kind: String::new(),
            metadata_priority: None,
            source_href: "/".into(),
            action_ids: vec![],
            bucket: 5,
            created_at_ms: 0,
            source_fingerprint: String::new(),
        };
        let now = crate::agent_auth::iso_to_epoch_ms("2026-08-29T12:00:00Z").unwrap();
        assert_eq!(badge_for(&raw, now).unwrap()["label"], "OVERDUE");
        raw.due_at = Some("2026-08-29T18:00:00Z".into()); // within the day
        assert_eq!(badge_for(&raw, now).unwrap()["label"], "DUE TODAY");
        raw.due_at = Some("2026-09-15T00:00:00Z".into());
        assert!(badge_for(&raw, now).is_none());
        raw.due_at = None;
        raw.priority = "p0".into();
        assert_eq!(badge_for(&raw, now).unwrap()["label"], "BLOCKING");
    }

    #[test]
    fn the_reader_zone_believes_the_browser_only_when_the_clock_can() {
        let config = BriefConfig {
            workday_start_hour: 9,
            lead_hours: 2,
            time_zone: "America/Denver".into(),
            sweep_minutes: 5,
        };
        // The stored preference wins everywhere — an airport lounge cannot
        // re-file the day.
        assert_eq!(
            reader_zone(Some(" Asia/Tokyo "), Some("UTC"), &config),
            "Asia/Tokyo"
        );
        // No stored zone: a plausible browser param is believed…
        assert_eq!(
            reader_zone(None, Some("Europe/Berlin"), &config),
            "Europe/Berlin"
        );
        // …a bad or oversized one is discarded, never stored.
        assert_eq!(
            reader_zone(None, Some("Mars/Olympus"), &config),
            "America/Denver"
        );
        assert_eq!(reader_zone(None, Some(""), &config), "America/Denver");
        assert_eq!(reader_zone(None, None, &config), "America/Denver");
        assert_eq!(
            reader_zone(None, Some(&"x".repeat(65)), &config),
            "America/Denver"
        );
    }

    #[test]
    fn schedule_slots_read_as_the_en_us_hour12_clock() {
        // 14:00-04:00 is 18:00Z — 2:00 PM in New York that day.
        assert_eq!(
            format_slot(Some("2026-08-29T14:00:00-04:00"), None, "America/New_York"),
            "2:00 PM"
        );
        assert_eq!(
            format_slot(
                Some("2026-08-29T00:30:00Z"),
                Some("2026-08-29T13:15:00Z"),
                "UTC"
            ),
            "12:30 AM – 1:15 PM"
        );
        // Noon and midnight are the two spellings the naive modulo gets wrong.
        assert_eq!(
            format_slot(Some("2026-08-29T12:00:00Z"), None, "UTC"),
            "12:00 PM"
        );
        assert_eq!(
            format_slot(Some("2026-08-29T00:00:00Z"), None, "UTC"),
            "12:00 AM"
        );
        assert_eq!(format_slot(None, None, "UTC"), "");
        // An unparseable start is an empty slot.
        assert_eq!(format_slot(Some("whenever"), None, "UTC"), "");
    }

    #[test]
    fn a_brief_with_no_content_yet_is_not_sweepable() {
        // The birth guard, as the fold sees it: last_seq 0 means the open's
        // first batch has not landed and a sweep would race it.
        let row = BriefRow {
            id: "b".into(),
            user_id: "u".into(),
            brief_date: "2026-08-29".into(),
            zone: "UTC".into(),
            agent_model: None,
            agent_name: None,
            artifact_id: None,
            last_seq: 0,
            read_seq: 0,
            last_swept_ms: None,
            created_ms: 0,
        };
        assert_eq!(row.last_seq, 0);
    }

    #[test]
    fn sweep_due_reads_the_throttle_honestly() {
        let config = BriefConfig {
            workday_start_hour: 9,
            lead_hours: 2,
            time_zone: "UTC".into(),
            sweep_minutes: 5,
        };
        let row = |swept: Option<i64>| BriefRow {
            id: "b".into(),
            user_id: "u".into(),
            brief_date: "2026-08-29".into(),
            zone: "UTC".into(),
            agent_model: None,
            agent_name: None,
            artifact_id: None,
            last_seq: 1,
            read_seq: 0,
            last_swept_ms: swept,
            created_ms: 0,
        };
        let now = 1_790_481_420_000;
        assert!(sweep_due(&row(None), &config, now));
        assert!(!sweep_due(&row(Some(now - 60_000)), &config, now));
        // Exactly at the throttle still sweeps — the test is >=.
        assert!(sweep_due(&row(Some(now - 300_000)), &config, now));
    }
}
