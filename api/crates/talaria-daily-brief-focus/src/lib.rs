// The brief's three row sources and the shared policy they are shaped by.
//
// Conversations are deliberately NOT one of them: the brief takes its comms
// from `comms_lines` because "unread" is the wrong question for a document
// about who is waiting — see that module's header.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_boards::board_visibility_sql;
use talaria_statuses::status_category_sql;

pub use talaria_daily_brief_types::{as_iso, fingerprint, key_of, nullable_iso};

/// The notification kinds the brief lists — the actionable set, not the bell.
pub const ACTIONABLE_NOTIFICATION_KINDS: [&str; 11] = [
    "mention",
    "dm",
    "agent-outreach",
    "kb-comment",
    "agent-problem",
    "workbench-repo-request",
    "research",
    "research-share",
    "plan-share",
    "task-assigned",
    "task-status",
];

pub fn priority_for_bucket(bucket: i64) -> &'static str {
    if bucket == 0 {
        "p0"
    } else if bucket <= 3 {
        "p1"
    } else if bucket == 4 {
        "p2"
    } else {
        "ok"
    }
}

pub fn task_bucket(status: &str, is_review: bool, is_triage: bool) -> i64 {
    if status == "failed" {
        0
    } else if status == "blocked" {
        1
    } else if is_review {
        3
    } else if is_triage {
        4
    } else {
        5
    }
}

pub fn task_status_label(status: &str, is_review: bool, is_triage: bool) -> String {
    if status == "failed" {
        "FAILED".into()
    } else if status == "blocked" {
        "BLOCKED".into()
    } else if is_review {
        "REVIEW".into()
    } else if is_triage {
        "TRIAGE".into()
    } else {
        status.replace('_', " ").to_uppercase()
    }
}

pub fn task_question(title: &str, status: &str, is_review: bool, is_triage: bool) -> String {
    if status == "failed" {
        format!("What should happen next for \u{201c}{title}\u{201d}?")
    } else if status == "blocked" {
        format!("How should we unblock \u{201c}{title}\u{201d}?")
    } else if is_review {
        format!("Approve the completed work for \u{201c}{title}\u{201d}?")
    } else if is_triage {
        format!("Where should \u{201c}{title}\u{201d} go next?")
    } else {
        format!("Review \u{201c}{title}\u{201d}?")
    }
}

pub fn task_recommendation(status: &str, is_review: bool) -> &'static str {
    if is_review {
        "Review the evidence, then approve it or request changes."
    } else if status == "failed" {
        "Open the task, inspect the failure, and decide whether to retry or reassign it."
    } else if status == "blocked" {
        "Open the task and resolve its dependency or reassign the next step."
    } else {
        "Open the task and assign an owner or move it into the right workflow stage."
    }
}

/// One source row, folded to the fields the brief reads: the key, the
/// rendering fields, and — because the fingerprint is over what the line SAYS
/// — the action IDS.
#[derive(Debug, Clone)]
pub struct RawFocusItem {
    pub key: String,
    pub source_type: String,
    pub source_id: String,
    pub priority: String,
    pub status_label: String,
    /// ISO, or null — badges compare it to the clock.
    pub due_at: Option<String>,
    pub question: String,
    pub recommendation: String,
    pub evidence: Vec<BriefEvidence>,
    /// The only metadata fields anything in the brief's half reads: `kind`
    /// (which section a notification lands in) and `priority` (sort rank).
    pub metadata_kind: String,
    pub metadata_priority: Option<String>,
    pub source_href: String,
    /// The ids of the actions the source offers, in source order — they ride
    /// the fingerprint but not the line.
    pub action_ids: Vec<String>,
    pub bucket: i64,
    pub created_at_ms: i64,
    /// item_fingerprint of the fields above — stamped once here, compared by
    /// the sweep forever.
    pub source_fingerprint: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct BriefEvidence {
    pub label: String,
    pub text: String,
}

impl From<&BriefEvidence> for Value {
    fn from(e: &BriefEvidence) -> Value {
        json!({"label": e.label, "text": e.text})
    }
}

/// What makes an item "materially different", as the brief's append-only diff
/// reads it: same fingerprint → the line stands (closed stays closed); moved →
/// a change row reopens it.
///
/// THE FIELDS ARE WHAT THE LINE SAYS AND RENDERS, and NOT `updatedAt` — that
/// exclusion is load-bearing. For a task, `updated_at` bumps on ANY write: a
/// comment, an assignee edit, a label, an agent touching the row. With it in
/// the fingerprint, checking off "Where should X go next?" survived only until
/// the next unrelated edit, and a dismissal was a mute on the row's noise
/// rather than a verdict. Everything a reader would call new information is
/// already here through other fields — status change rewords `question`,
/// description/error/outcome ride `evidence`, review gates ride the action ids
/// — so `updatedAt` contributed churn and nothing else.
///
/// `statusLabel` and `dueAt` are IN, conversely, because the line renders both:
/// the status chip and the DUE TODAY/OVERDUE badge.
pub fn item_fingerprint(item: &RawFocusItem) -> String {
    fingerprint(&json!({
        "sourceType": item.source_type,
        "sourceId": item.source_id,
        "question": item.question,
        "evidence": item.evidence.iter().map(Value::from).collect::<Vec<_>>(),
        "actions": item.action_ids,
        "statusLabel": item.status_label,
        "dueAt": item.due_at,
    }))
}

// ── Tasks ─────────────────────────────────────────────────────────────────────

/// Everything on this person's boards that needs a human: failed, blocked, in
/// review, or waiting in triage — the read ACL (board membership or the owning
/// team's) carried in the WHERE.
pub async fn task_items(pg: &PgPool, user_id: &str) -> Result<Vec<RawFocusItem>, sqlx::Error> {
    // An APPROVED ticket is not awaiting review any more: the sign-off is the
    // answer to the question this queue asks. Folding `approved_at is null`
    // into the review fragment itself covers both uses below — the is_review
    // flag the card reads and the WHERE that admits the row at all — so a
    // ticket a person already signed off stops turning up in the brief while
    // it waits for its merge or release.
    let review = format!(
        "( {} and t.approved_at is null )",
        status_category_sql("review", &["quality_review"])
    );
    let sql = format!(
        "select t.id::text, t.board_id::text, \
                t.title, t.description, t.status, t.priority, \
                (trunc(extract(epoch from t.due_date) * 1000))::bigint as due_ms, \
                t.error_message, t.outcome, \
                (trunc(extract(epoch from t.created_at) * 1000))::bigint as created_ms, \
                (trunc(extract(epoch from t.updated_at) * 1000))::bigint as updated_ms, \
                {review} as is_review, \
                ( t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'open' and not bs.agent_start) \
                  or (not exists (select 1 from board_statuses bs where bs.board_id = t.board_id) and t.status = 'inbox') ) as is_triage \
         from tasks t \
         join boards b on b.id = t.board_id \
         where {vis} \
           and t.archived_at is null \
           and ( t.status in ('failed', 'blocked') \
                 or {review} \
                 or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'open' and not bs.agent_start) \
                 or (not exists (select 1 from board_statuses bs where bs.board_id = t.board_id) and t.status = 'inbox') ) \
         order by t.created_at asc limit 200",
        vis = board_visibility_sql("$1", "$1", false),
    );
    // AssertSqlSafe: the interpolations are this module's own review/triage
    // category SQL and the crate's board-visibility fragment.
    let rows: Vec<TaskRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let bucket = task_bucket(&row.status, row.is_review, row.is_triage);
            let mut evidence = Vec::new();
            // Each evidence field is capped like every other source — enough
            // to judge, never enough to bury the card.
            if let Some(d) = row.description.as_deref() {
                evidence.push(BriefEvidence {
                    label: "Task".into(),
                    text: d.chars().take(1_000).collect(),
                });
            }
            if let Some(m) = row.error_message.as_deref() {
                evidence.push(BriefEvidence {
                    label: "Failure".into(),
                    text: m.chars().take(1_000).collect(),
                });
            }
            if let Some(o) = row.outcome.as_deref() {
                evidence.push(BriefEvidence {
                    label: "Outcome".into(),
                    text: o.chars().take(1_000).collect(),
                });
            }
            // The review pair. 'Safe' DESCRIBES A HUMAN CLICK on this card;
            // whether a MODEL may run it is decided elsewhere — the ids are
            // all that ride the fingerprint.
            let action_ids: Vec<String> = if row.is_review {
                vec!["approve_task".into(), "request_changes".into()]
            } else {
                Vec::new()
            };
            let base = RawFocusItem {
                key: key_of("task", &row.id),
                source_type: "task".into(),
                source_id: row.id.clone(),
                priority: priority_for_bucket(bucket).into(),
                status_label: task_status_label(&row.status, row.is_review, row.is_triage),
                due_at: nullable_iso(row.due_ms),
                question: task_question(&row.title, &row.status, row.is_review, row.is_triage),
                recommendation: task_recommendation(&row.status, row.is_review).into(),
                evidence,
                metadata_kind: String::new(),
                metadata_priority: Some(row.priority.clone()),
                source_href: format!("/boards/{}/{}", row.board_id, row.id),
                action_ids,
                bucket,
                created_at_ms: row.created_ms,
                // Stamped by the struct update below — `item_fingerprint`
                // never reads this field, so the placeholder is exact, not a
                // default.
                source_fingerprint: String::new(),
            };
            RawFocusItem {
                source_fingerprint: item_fingerprint(&base),
                ..base
            }
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct TaskRow {
    id: String,
    board_id: String,
    title: String,
    description: Option<String>,
    status: String,
    priority: String,
    due_ms: Option<i64>,
    error_message: Option<String>,
    outcome: Option<String>,
    created_ms: i64,
    #[allow(dead_code)]
    updated_ms: i64,
    is_review: bool,
    is_triage: bool,
}

// ── Pending Google actions (approvals) ───────────────────────────────────────

/// `google_pending_actions` held for a person — the outbound payloads an agent
/// drafted under their identity. Personal actions are the owner's alone (an
/// admin is not entitled to the subject line of somebody's mailbox by virtue
/// of being an admin); org actions are every admin's.
/// One pending outbound action: (id, kind, summary, payload, agent model,
/// org-wide?, created ms).
type PendingActionRow = (
    String,
    String,
    Option<String>,
    Value,
    Option<String>,
    bool,
    i64,
);

pub async fn approval_items(
    pg: &PgPool,
    user_id: &str,
    is_admin: bool,
) -> Result<Vec<RawFocusItem>, sqlx::Error> {
    let rows: Vec<PendingActionRow> = sqlx::query_as(
        "select id::text, kind, summary, payload, agent_model, is_org, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from google_pending_actions \
         where status = 'pending' and ((is_org = false and owner_user_id = $1::uuid) or ($2::boolean and is_org)) \
         order by created_at desc",
    )
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, kind, summary, payload, agent_model, _is_org, created_ms)| {
            let label = match kind.as_str() {
                "gmail_send" => "SEND EMAIL",
                "doc_update" => "UPDATE DOC",
                _ => "CREATE EVENT",
            };
            // The payload's own fields, not a paraphrase: the card recommends
            // "review the exact outbound payload", so the evidence IS the
            // payload. The actor rides last (4 rows max render).
            let mut evidence = match kind.as_str() {
                "gmail_send" => approval_evidence_email(&payload),
                "doc_update" | "drive_move" | "drive_rename" => approval_evidence_file(&payload),
                _ => approval_evidence_event(&payload),
            };
            evidence.push(BriefEvidence {
                label: "Drafted by".into(),
                text: agent_model.unwrap_or_else(|| "Agent".into()),
            });
            let base = RawFocusItem {
                key: key_of("approval", &id),
                source_type: "approval".into(),
                source_id: id,
                priority: "p0".into(),
                status_label: format!("APPROVAL \u{b7} {label}"),
                due_at: None,
                question: format!(
                    "{} \u{201c}{}\u{201d}?",
                    match kind.as_str() {
                        "gmail_send" => "Send",
                        "doc_update" => "Update",
                        _ => "Create",
                    },
                    summary.unwrap_or_else(|| label.to_lowercase())
                ),
                recommendation: "Review the exact outbound payload before confirming this identity-bearing action.".into(),
                evidence,
                metadata_kind: kind.clone(),
                metadata_priority: None,
                source_href: "/".into(),
                action_ids: vec!["approve".into(), "reject".into()],
                bucket: 0,
                created_at_ms: created_ms,
                source_fingerprint: String::new(),
            };
            RawFocusItem { source_fingerprint: item_fingerprint(&base), ..base }
        })
        .collect())
}

/// An email draft as evidence rows: who gets it, what it says.
fn approval_evidence_email(payload: &Value) -> Vec<BriefEvidence> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str());
    let mut out = Vec::new();
    if let Some(to) = get("to") {
        let cc = get("cc")
            .map(|cc| format!(" \u{b7} cc {cc}"))
            .unwrap_or_default();
        out.push(BriefEvidence {
            label: "To".into(),
            text: format!("{to}{cc}").chars().take(1_000).collect(),
        });
    }
    // A present-but-empty subject still gets its row.
    match payload.get("subject") {
        Some(Value::String(s)) => out.push(BriefEvidence {
            label: "Subject".into(),
            text: s.chars().take(1_000).collect(),
        }),
        _ => out.push(BriefEvidence {
            label: "Subject".into(),
            text: "(empty)".into(),
        }),
    }
    match get("body").filter(|b| !b.is_empty()) {
        Some(body) => out.push(BriefEvidence {
            label: "Body".into(),
            text: body.chars().take(1_000).collect(),
        }),
        None => out.push(BriefEvidence {
            label: "Body".into(),
            text: "(empty)".into(),
        }),
    }
    out
}

/// An event draft as evidence rows: when, where, who.
fn approval_evidence_event(payload: &Value) -> Vec<BriefEvidence> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str());
    let mut out = Vec::new();
    if let (Some(start), Some(end)) = (get("start"), get("end")) {
        let all_day = payload
            .get("allDay")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let suffix = if all_day { " \u{b7} all day" } else { "" };
        out.push(BriefEvidence {
            label: "When".into(),
            text: format!("{start} \u{2192} {end}{suffix}")
                .chars()
                .take(1_000)
                .collect(),
        });
    }
    let mut where_who = Vec::new();
    if let Some(location) = get("location") {
        where_who.push(location.to_string());
    }
    if let Some(attendees) = payload.get("attendees").and_then(|v| v.as_array()) {
        where_who.extend(
            attendees
                .iter()
                .filter_map(|a| a.as_str().map(str::to_string)),
        );
    }
    let joined = where_who.join(" \u{b7} ");
    if !joined.is_empty() {
        out.push(BriefEvidence {
            label: "Where / who".into(),
            text: joined.chars().take(1_000).collect(),
        });
    }
    if let Some(notes) = get("description") {
        out.push(BriefEvidence {
            label: "Notes".into(),
            text: notes.chars().take(1_000).collect(),
        });
    }
    out
}

fn approval_evidence_file(payload: &Value) -> Vec<BriefEvidence> {
    let get = |k: &str| payload.get(k).and_then(|v| v.as_str());
    let mut out = Vec::new();
    let push = |out: &mut Vec<BriefEvidence>, label: &str, text: &str| {
        out.push(BriefEvidence {
            label: label.into(),
            text: text.chars().take(1_000).collect(),
        });
    };
    if let Some(id) = get("fileId").filter(|v| !v.is_empty()) {
        push(&mut out, "File", id);
    }
    if let Some(name) = get("name")
        .or_else(|| get("title"))
        .filter(|v| !v.is_empty())
    {
        push(&mut out, "Name", name);
    }
    if let Some(parent) = get("parentId").filter(|v| !v.is_empty()) {
        push(&mut out, "Folder", parent);
    }
    if let Some(body) = get("markdown").filter(|v| !v.is_empty()) {
        push(&mut out, "Incoming", body);
    }
    out
}

// ── Notifications ─────────────────────────────────────────────────────────────

// A brief/queue line exists to be FOLLOWED — "a brief that names a blocked
// ticket and cannot open it is a newsletter". A notification whose link points
// at a board or channel the reader is not a member of is a line about somebody
// else's resource, and never enters the set.
static BOARD_HREF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^/boards/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})")
        .expect("static regex compiles")
});
static CHANNEL_HREF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^/comms/channel/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})",
    )
    .expect("static regex compiles")
});

/// Which of these notification hrefs lead somewhere THE READER may go.
///
/// Resource-shaped hrefs — the two shapes the product's own notification
/// writers emit — are checked against the same membership predicates the task
/// and comms sources use (board: member or team; channel: member, not
/// archived), so a notification about a ticket on a board the reader was
/// removed from cannot re-enter their list as a line that 403s when opened.
/// Every other href — app surfaces like /research — passes: it is linkable,
/// and its accessibility is the surface's own route rule, not a membership.
pub async fn accessible_notification_hrefs(
    pg: &PgPool,
    user_id: &str,
    hrefs: &[String],
) -> Result<HashSet<String>, sqlx::Error> {
    let mut board_ids: Vec<String> = Vec::new();
    let mut channel_ids: Vec<String> = Vec::new();
    for href in hrefs {
        if let Some(id) = BOARD_HREF.captures(href).and_then(|c| c.get(1)) {
            let id = id.as_str().to_string();
            if !board_ids.contains(&id) {
                board_ids.push(id);
            }
        }
        if let Some(id) = CHANNEL_HREF.captures(href).and_then(|c| c.get(1)) {
            let id = id.as_str().to_string();
            if !channel_ids.contains(&id) {
                channel_ids.push(id);
            }
        }
    }
    let boards: HashSet<String> = if board_ids.is_empty() {
        HashSet::new()
    } else {
        // AssertSqlSafe: the interpolation is the board-visibility fragment.
        let sql = format!(
            "select distinct b.id::text from boards b \
             where b.id = any($1::uuid[]) and {}",
            board_visibility_sql("$2", "$2", false)
        );
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(&board_ids)
            .bind(user_id)
            .fetch_all(pg)
            .await?
            .into_iter()
            .collect()
    };
    let channels: HashSet<String> = if channel_ids.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar(
            "select distinct c.id::text from channels c \
             join channel_members m on m.channel_id = c.id and m.user_id = $2::uuid \
             where c.id = any($1::uuid[]) and c.archived_at is null",
        )
        .bind(&channel_ids)
        .bind(user_id)
        .fetch_all(pg)
        .await?
        .into_iter()
        .collect()
    };
    Ok(hrefs
        .iter()
        .filter(|href| {
            let board = BOARD_HREF
                .captures(href)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());
            if let Some(id) = board {
                return boards.contains(&id);
            }
            let channel = CHANNEL_HREF
                .captures(href)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());
            if let Some(id) = channel {
                return channels.contains(&id);
            }
            true
        })
        .cloned()
        .collect())
}

pub async fn notification_items(
    pg: &PgPool,
    user_id: &str,
) -> Result<Vec<RawFocusItem>, sqlx::Error> {
    let kinds: Vec<String> = ACTIONABLE_NOTIFICATION_KINDS
        .iter()
        .map(|k| k.to_string())
        .collect();
    let rows: Vec<(String, String, String, String, String, i64)> = sqlx::query_as(
        "select id::text, kind, title, body, href, (trunc(extract(epoch from created_at) * 1000))::bigint \
         from notifications \
         where user_id = $1::uuid and read_at is null and kind = any($2) \
           and coalesce(href, '') not in ('', '/') \
         order by created_at asc limit 200",
    )
    .bind(user_id)
    .bind(&kinds)
    .fetch_all(pg)
    .await?;

    // ACCESSIBILITY, not just linkability: a row whose href leads to a resource
    // this reader is not on is about somebody else's work, and a list they
    // cannot act from is where it must not appear.
    let hrefs: Vec<String> = rows.iter().map(|r| r.4.clone()).collect();
    let accessible = accessible_notification_hrefs(pg, user_id, &hrefs).await?;
    Ok(rows
        .into_iter()
        .filter(|row| accessible.contains(&row.4))
        .map(|(id, kind, title, body, href, created_ms)| {
            let direct = matches!(kind.as_str(), "mention" | "dm" | "agent-outreach");
            let bucket = if direct { 2 } else { 5 };
            let base = RawFocusItem {
                key: key_of("notification", &id),
                source_type: "notification".into(),
                source_id: id,
                priority: priority_for_bucket(bucket).into(),
                status_label: kind.replace('-', " ").to_uppercase(),
                due_at: None,
                question: format!("Review \u{201c}{title}\u{201d}?"),
                recommendation: if direct {
                    "Open the source and respond if needed."
                } else {
                    "Review the source, then mark this item read."
                }
                .into(),
                evidence: vec![BriefEvidence {
                    label: "Source".into(),
                    text: if body.is_empty() { title.clone() } else { body }
                        .chars()
                        .take(1_000)
                        .collect(),
                }],
                metadata_kind: kind,
                metadata_priority: None,
                source_href: if href.is_empty() { "/".into() } else { href },
                action_ids: vec!["mark_read".into()],
                bucket,
                created_at_ms: created_ms,
                source_fingerprint: String::new(),
            };
            RawFocusItem {
                source_fingerprint: item_fingerprint(&base),
                ..base
            }
        })
        .collect())
}

// ── Dedupe and order ─────────────────────────────────────────────────────────

/// Drop a notification that merely points at a ticket or a channel already in
/// the set. What the brief passes here never contains channel items (the
/// conversations are their own source), so the channel half never fires — the
/// rule is one policy over whatever lines the set holds.
pub fn dedupe_items(items: Vec<RawFocusItem>) -> Vec<RawFocusItem> {
    // Owned id sets: borrowing them off `items` would pin the vec for the
    // whole function and the consume-with-into_iter below could not move it.
    let task_ids: HashSet<String> = items
        .iter()
        .filter(|i| i.source_type == "task")
        .map(|i| i.source_id.clone())
        .collect();
    let channel_ids: HashSet<String> = items
        .iter()
        .filter(|i| i.source_type == "channel")
        .map(|i| i.source_id.clone())
        .collect();
    let mut seen_links: HashSet<String> = HashSet::new();
    items
        .into_iter()
        .filter(|item| {
            if item.source_type == "notification" {
                if task_ids
                    .iter()
                    .any(|id| item.source_href.contains(id.as_str()))
                {
                    return false;
                }
                if channel_ids
                    .iter()
                    .any(|id| item.source_href.contains(id.as_str()))
                {
                    return false;
                }
            }
            let link_key = if !item.source_href.is_empty() && item.source_href != "/" {
                item.source_href.clone()
            } else {
                item.key.clone()
            };
            if seen_links.contains(&link_key) && item.source_type == "notification" {
                return false;
            }
            seen_links.insert(link_key);
            true
        })
        .collect()
}

/// The reading order: bucket first (failed work outranks waiting messages),
/// then due-ness, then the ticket's own priority, then age. `now_ms` is passed
/// in because a sort against the wall clock is untestable.
pub fn sort_items(items: Vec<RawFocusItem>, now_ms: i64) -> Vec<RawFocusItem> {
    let due_rank = |item: &RawFocusItem| -> i64 {
        // No due date at all is its own rank, BELOW everything with one.
        let Some(raw) = item.due_at.as_deref() else {
            return 2;
        };
        // An unparseable due date lands in the "due, but not soon" rank —
        // not the none-at-all one.
        let Some(due) = talaria_agent_auth::iso_to_epoch_ms(raw) else {
            return 1;
        };
        if due <= now_ms + 7 * 86_400_000 { 0 } else { 1 }
    };
    let priority_rank = |p: &str| -> i64 {
        match p {
            "urgent" => 0,
            "high" => 1,
            "medium" => 2,
            "low" => 3,
            // An absent metadata.priority ranks at 4, below every real tier.
            _ => 4,
        }
    };
    let mut sorted = items;
    sorted.sort_by(|a, b| {
        a.bucket
            .cmp(&b.bucket)
            .then_with(|| due_rank(a).cmp(&due_rank(b)))
            .then_with(|| {
                priority_rank(a.metadata_priority.as_deref().unwrap_or("undefined")).cmp(
                    &priority_rank(b.metadata_priority.as_deref().unwrap_or("undefined")),
                )
            })
            .then_with(|| a.created_at_ms.cmp(&b.created_at_ms))
    });
    sorted
}

/// `sort_items`, with the within-bucket tiebreak ASKED instead of assumed.
///
/// The deterministic sort runs first and is both the baseline and the
/// fallback, so with the port off — or the site switched off, which is the
/// default — this returns byte-for-byte what `sort_items` returns.
///
/// WHAT MOVES AND WHAT DOES NOT. Buckets never move: failed work outranks a
/// waiting message because those are different CATEGORIES, not different
/// intensities, and no confidence makes it right to drop a failed deploy below
/// an unread mention. Inside a bucket, the `(due-ness, priority, age)`
/// tiebreak is replaced by judged urgency — which is where that comparator was
/// weakest, since "due in six days, high priority, filed Tuesday" versus "due
/// in eight days, urgent, filed this morning" was decided by field order
/// rather than by anything about the work.
///
/// A bucket where ANY item lacks a usable judgment keeps its whole
/// deterministic order. Half-judged is ordered by neither policy and would
/// read as a bug in whichever one the reader expected.
pub async fn sort_items_judged(
    state: &talaria_state::AppState,
    items: Vec<RawFocusItem>,
    now_ms: i64,
) -> Vec<RawFocusItem> {
    let sorted = sort_items(items, now_ms);
    let candidates: Vec<talaria_decide::focus::Candidate> = sorted
        .iter()
        .map(|i| talaria_decide::focus::Candidate {
            key: i.key.clone(),
            state: json!({
                "key": i.key,
                "asks": i.question,
                "suggested": i.recommendation,
                "state": i.status_label,
                // Empty-as-empty rather than omitted: "no due date" is a fact
                // the judgment turns on, and an absent key reads as unknown.
                "due": i.due_at.clone().unwrap_or_default(),
                "stated_priority": i.metadata_priority.clone().unwrap_or_default(),
                "evidence": i.evidence.iter().map(Value::from).collect::<Vec<_>>(),
            }),
        })
        .collect();
    let http = talaria_retrieval_http::real_http();
    let ranking = talaria_decide::focus::rank(state, &http, &as_iso(now_ms), &candidates).await;
    if ranking.urgency.is_empty() {
        return sorted;
    }
    let order = apply_within_buckets(&sorted, &ranking.urgency);

    // THE MEASUREMENT, and the question it answers is the one worth asking of
    // an ordering: did the judgment change what the person sees FIRST? A
    // reorder that only shuffles rows 6 through 9 is not worth a provider
    // call, and a panel that reported "the order differed" without saying
    // where would not distinguish the two.
    //
    // Recorded whether or not the site acted — with it off this is the whole
    // point, and with it on it is the audit trail for an order somebody
    // queries.
    let before = sorted.first().map(|i| i.key.clone()).unwrap_or_default();
    let after = order.first().cloned().unwrap_or_default();
    talaria_decide::shadow::record_derived(
        &state.pg,
        talaria_decide::shadow::Derived {
            site: talaria_decide::focus::SITE,
            subject_ref: Some(&before),
            baseline: &before,
            port_answer: &after,
            agreed: before == after,
            provider: "",
            latency_ms: None,
        },
    )
    .await;

    if !ranking.acts {
        // Shadow: the comparison is recorded and the deterministic order is
        // what the brief gets. Nothing above this line can change that.
        return sorted;
    }
    let mut by_key: std::collections::HashMap<String, RawFocusItem> =
        sorted.into_iter().map(|i| (i.key.clone(), i)).collect();
    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

/// Apply a within-bucket reorder, bucket by bucket, and answer the key order.
///
/// Pure and separate from the request for one reason: the property that must
/// hold is that NO ITEM EVER CROSSES A BUCKET BOUNDARY, and that is worth
/// pinning with a test rather than trusting a loop to be read correctly. The
/// output is built by concatenating runs, so the bucket sequence cannot change
/// even if `reordered` misbehaves.
fn apply_within_buckets(
    sorted: &[RawFocusItem],
    urgency: &std::collections::BTreeMap<String, f64>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(sorted.len());
    let mut run: Vec<String> = Vec::new();
    let mut run_bucket: Option<i64> = None;
    let flush = |run: &mut Vec<String>, out: &mut Vec<String>| {
        if run.is_empty() {
            return;
        }
        match talaria_decide::focus::reordered(run, urgency) {
            Some(order) => out.extend(order),
            // No usable judgment for every item in this run — it keeps its
            // whole deterministic order.
            None => out.append(&mut run.clone()),
        }
        run.clear();
    };
    for item in sorted {
        if run_bucket != Some(item.bucket) {
            flush(&mut run, &mut out);
            run_bucket = Some(item.bucket);
        }
        run.push(item.key.clone());
    }
    flush(&mut run, &mut out);
    out
}

/// The evidence list as the jsonb the entries table stores.
pub fn evidence_value(evidence: &[BriefEvidence]) -> Value {
    Value::Array(evidence.iter().map(Into::into).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_fingerprint(mut item: RawFocusItem) -> RawFocusItem {
        item.source_fingerprint = item_fingerprint(&item);
        item
    }

    // THE FINGERPRINT IS BYTE-STABLE BY CONTRACT: these vectors pin the exact
    // bytes the hash is computed over, in the same key order as the `json!`
    // calls above. If this test fails, the bytes changed and every line in
    // every existing brief reads as "changed" on the next sweep.
    #[test]
    fn fingerprint_bytes_match_json_stringify() {
        let value = json!({
            "sourceType": "task",
            "sourceId": "t1",
            "question": "What should happen next for \"Fix the login flow\"?",
            "evidence": [
                {"label": "Task", "text": "It 500s on <b>every</b> POST"},
                {"label": "Failure", "text": "segfault"}
            ],
            "actions": ["approve_task", "request_changes"],
            "statusLabel": "FAILED",
            "dueAt": null
        });
        assert_eq!(
            fingerprint(&value),
            "e44728b62b72a38666e875b6069e3940ad8f3ed6dcd33c44afd1bdba50ebc2f1"
        );
        // The comms fingerprint's object: string/number/null mix.
        let comms = json!({"u": 3, "a": "you", "s": 41, "l": 42, "d": "d7:null", "g": true});
        assert_eq!(
            fingerprint(&comms),
            "1562f0b8ad6e99f71a16c1367df86423edc4e8f62a2f06222098ca011b169b8d"
        );
    }

    #[test]
    fn item_fingerprint_excludes_updated_at_and_includes_what_renders() {
        let mut item = RawFocusItem {
            key: "task:t1".into(),
            source_type: "task".into(),
            source_id: "t1".into(),
            priority: "p0".into(),
            status_label: "FAILED".into(),
            due_at: None,
            question: "What next?".into(),
            recommendation: "Open it.".into(),
            evidence: vec![BriefEvidence {
                label: "Task".into(),
                text: "x".into(),
            }],
            metadata_kind: String::new(),
            metadata_priority: Some("high".into()),
            source_href: "/boards/b/t1".into(),
            action_ids: vec!["approve_task".into()],
            bucket: 0,
            created_at_ms: 0,
            source_fingerprint: String::new(),
        };
        let before = with_fingerprint(item.clone());
        // An unrelated edit — the exact case the exclusion exists for — does
        // not move the line.
        item.created_at_ms += 50_000;
        assert_eq!(
            with_fingerprint(item.clone()).source_fingerprint,
            before.source_fingerprint
        );
        // The status chip does.
        item.status_label = "BLOCKED".into();
        assert_ne!(
            with_fingerprint(item).source_fingerprint,
            before.source_fingerprint
        );
    }

    #[test]
    fn dedupe_drops_a_notification_pointing_at_a_task_already_in_the_set() {
        let task = |key: &str, href: &str| RawFocusItem {
            key: key.into(),
            source_type: "task".into(),
            source_id: key.trim_start_matches("task:").into(),
            priority: "p1".into(),
            status_label: "TRIAGE".into(),
            due_at: None,
            question: "Where next?".into(),
            recommendation: "Open it.".into(),
            evidence: vec![],
            metadata_kind: String::new(),
            metadata_priority: None,
            source_href: href.into(),
            action_ids: vec![],
            bucket: 4,
            created_at_ms: 0,
            source_fingerprint: "f".into(),
        };
        let notif = |key: &str, href: &str| RawFocusItem {
            source_type: "notification".into(),
            metadata_kind: "task-status".into(),
            ..task(key, href)
        };
        let kept = dedupe_items(vec![
            task("task:t1", "/boards/b/t1"),
            notif("notification:n1", "/boards/b/t1"),
            notif("notification:n2", "/research/r1"),
        ]);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|i| i.key != "notification:n1"));
    }

    #[test]
    fn sort_puts_blocked_work_above_an_old_unread() {
        let item = |bucket: i64, created: i64| RawFocusItem {
            key: format!("x{created}"),
            source_type: "task".into(),
            source_id: "s".into(),
            priority: "ok".into(),
            status_label: "X".into(),
            due_at: None,
            question: "q".into(),
            recommendation: "r".into(),
            evidence: vec![],
            metadata_kind: String::new(),
            metadata_priority: None,
            source_href: "/".into(),
            action_ids: vec![],
            bucket,
            created_at_ms: created,
            source_fingerprint: "f".into(),
        };
        let sorted = sort_items(vec![item(5, 100), item(1, 900)], 0);
        assert_eq!(sorted[0].bucket, 1);
        // Same bucket: oldest first.
        let aged = sort_items(vec![item(3, 900), item(3, 100)], 0);
        assert_eq!(aged[0].created_at_ms, 100);
    }

    #[test]
    fn task_labels_read_the_pinned_sentences() {
        assert_eq!(task_status_label("quality_review", true, false), "REVIEW");
        assert_eq!(
            task_status_label("in_progress", false, false),
            "IN PROGRESS"
        );
        assert_eq!(
            task_question("Fix login", "failed", false, false),
            "What should happen next for \u{201c}Fix login\u{201d}?"
        );
        assert_eq!(priority_for_bucket(0), "p0");
        assert_eq!(priority_for_bucket(4), "p2");
        assert_eq!(priority_for_bucket(5), "ok");
    }

    #[test]
    fn email_evidence_rows_carry_the_payload_verbatim() {
        let payload = json!({
            "to": "priya@example.com",
            "cc": "sam@example.com",
            "subject": "Quarterly numbers",
            "body": "Here they are."
        });
        let rows = approval_evidence_email(&payload);
        assert_eq!(rows[0].text, "priya@example.com \u{b7} cc sam@example.com");
        assert_eq!(rows[1].text, "Quarterly numbers");
        // A payload with no body at all says so, rather than rendering nothing.
        let empty = approval_evidence_email(&json!({}));
        assert_eq!(empty[0].text, "(empty)");
        assert_eq!(empty[0].label, "Subject");
    }

    // ── The judged tiebreak ─────────────────────────────────────────────────

    fn ranked(bucket: i64, key: &str) -> RawFocusItem {
        RawFocusItem {
            key: key.into(),
            source_type: "task".into(),
            source_id: "s".into(),
            priority: "ok".into(),
            status_label: "X".into(),
            due_at: None,
            question: "q".into(),
            recommendation: "r".into(),
            evidence: vec![],
            metadata_kind: String::new(),
            metadata_priority: None,
            source_href: "/".into(),
            action_ids: vec![],
            bucket,
            created_at_ms: 0,
            source_fingerprint: "f".into(),
        }
    }

    #[test]
    fn no_item_ever_crosses_a_bucket_boundary_however_urgent_it_is_judged() {
        // THE ONE PROPERTY THAT MATTERS. Buckets are categories, not
        // intensities: a failed deploy outranks an unread mention because
        // somebody decided that, and no confidence makes it right to swap
        // them. So the most urgent judgment possible on a bucket-5 item must
        // leave it behind every bucket-0 item.
        let items = vec![
            ranked(0, "failed-a"),
            ranked(0, "failed-b"),
            ranked(5, "mention"),
        ];
        let mut u = std::collections::BTreeMap::new();
        u.insert("failed-a".to_string(), 0.0);
        u.insert("failed-b".to_string(), 0.0);
        u.insert("mention".to_string(), 1.0);
        let order = apply_within_buckets(&items, &u);
        assert_eq!(
            order,
            vec!["failed-a", "failed-b", "mention"],
            "a bucket-5 item scored 1.0 must stay below every bucket-0 item"
        );
        // Every item is present exactly once — a reorder is a permutation, and
        // losing or duplicating a row on a page somebody works from is worse
        // than any ordering mistake.
        assert_eq!(order.len(), items.len());
        let mut sorted = order.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), items.len(), "a row was lost or duplicated");
    }

    #[test]
    fn within_a_bucket_the_judgment_breaks_the_tie_and_a_gap_in_it_changes_nothing() {
        let items = vec![ranked(3, "a"), ranked(3, "b"), ranked(3, "c")];
        let mut u = std::collections::BTreeMap::new();
        u.insert("a".to_string(), 0.2);
        u.insert("b".to_string(), 0.9);
        u.insert("c".to_string(), 0.5);
        assert_eq!(apply_within_buckets(&items, &u), vec!["b", "c", "a"]);

        // One item without a usable judgment, and the WHOLE bucket keeps its
        // deterministic order — half-judged is ordered by neither policy.
        u.remove("c");
        assert_eq!(apply_within_buckets(&items, &u), vec!["a", "b", "c"]);

        // And no judgments at all is the default every install runs on.
        let none = std::collections::BTreeMap::new();
        assert_eq!(apply_within_buckets(&items, &none), vec!["a", "b", "c"]);
    }

    #[test]
    fn each_bucket_is_judged_on_its_own_so_one_missing_score_does_not_freeze_the_others() {
        // Runs are flushed per bucket, so the bucket that has every score is
        // reordered even while a neighbouring one is not.
        let items = vec![
            ranked(1, "p"),
            ranked(1, "q"),
            ranked(4, "r"),
            ranked(4, "s"),
        ];
        let mut u = std::collections::BTreeMap::new();
        u.insert("p".to_string(), 0.1);
        u.insert("q".to_string(), 0.9);
        // bucket 4 is missing `s`, so it stays as it is.
        u.insert("r".to_string(), 0.9);
        assert_eq!(apply_within_buckets(&items, &u), vec!["q", "p", "r", "s"]);
    }
}
