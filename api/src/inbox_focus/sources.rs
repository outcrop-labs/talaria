// inbox-focus-sources.ts — the four provenances a queue item can have.
//
// Every SQL statement is the TS one verbatim (epoch-ms extraction instead of
// Date columns — the wire renders both through `as_iso`), and every metadata
// record is built in its TS key order: metadata rides the wire and the item
// fingerprint, and `json!` + preserve_order keeps that order byte-identical.

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::body::truncate_utf16;
use crate::google::pending_actions::list_pending;
use crate::harness::defs::inbox_focus::FocusEvidence;
use crate::inbox_focus::policy::{
    as_iso, finalize_item, focus_action, key_of, nullable_iso, priority_for_bucket, task_bucket,
    task_question, task_recommendation, task_status_label,
};
use crate::inbox_focus::types::RawFocusItem;
use crate::session::SessionUser;

fn evidence(label: &str, text: &str) -> FocusEvidence {
    FocusEvidence {
        label: label.to_string(),
        text: truncate_utf16(text, 1_000).to_string(),
    }
}

// ── Tasks ────────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct TaskRow {
    id: String,
    board_id: String,
    board: String,
    ticket_ref: Option<String>,
    title: String,
    description: Option<String>,
    status: String,
    priority: String,
    due_ms: Option<i64>,
    error_message: Option<String>,
    outcome: Option<String>,
    created_ms: i64,
    updated_ms: i64,
    is_review: bool,
    is_triage: bool,
}

pub async fn task_items(
    pg: &PgPool,
    user_id: &str,
    source_id: Option<&str>,
) -> Result<Vec<RawFocusItem>, sqlx::Error> {
    let review = crate::statuses::status_category_sql("review", &["quality_review"]);
    let sql = format!(
        "select t.id::text, t.board_id::text, b.name as board, \
                case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end as ticket_ref, \
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
           and ($2::text is null or t.id::text = $2) \
           and ( t.status in ('failed', 'blocked') \
                 or {review} \
                 or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'open' and not bs.agent_start) \
                 or (not exists (select 1 from board_statuses bs where bs.board_id = t.board_id) and t.status in ('inbox', 'quality_review')) ) \
         order by t.created_at asc limit 200",
        review = review,
        vis = crate::boards::board_visibility_sql("$1", "$1", false),
    );
    // AssertSqlSafe: the interpolations are the statuses module's category SQL
    // and the crate's board-visibility fragment.
    let rows: Vec<TaskRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(source_id)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let bucket = task_bucket(&row.status, row.is_review, row.is_triage);
            // `risk` DESCRIBES A HUMAN CLICK ON THIS CARD, and 'safe' is still
            // right for both: the reviewer is looking at the evidence, and
            // asking them to confirm their own click would be a modal for its
            // own sake. Whether a MODEL may run these without a person is
            // decided by the proposal-source gate, not by this flag.
            let actions = if row.is_review {
                vec![
                    focus_action("approve_task", "Approve", "safe", None, None),
                    focus_action("request_changes", "Request changes", "safe", None, None),
                ]
            } else {
                Vec::new()
            };
            let mut items = Vec::new();
            if let Some(d) = row.description.as_deref() {
                items.push(evidence("Task", d));
            }
            if let Some(m) = row.error_message.as_deref() {
                items.push(evidence("Failure", m));
            }
            if let Some(o) = row.outcome.as_deref() {
                items.push(evidence("Outcome", o));
            }
            finalize_item(RawFocusItem {
                key: key_of("task", &row.id),
                source_type: "task".into(),
                source_id: row.id.clone(),
                priority: priority_for_bucket(bucket).into(),
                status_label: task_status_label(&row.status, row.is_review, row.is_triage),
                created_at: as_iso(row.created_ms),
                updated_at: as_iso(row.updated_ms),
                due_at: nullable_iso(row.due_ms),
                question: task_question(&row.title, &row.status, row.is_review, row.is_triage),
                recommendation: task_recommendation(&row.status, row.is_review).into(),
                recommended_action_id: row.is_review.then(|| "approve_task".to_string()),
                evidence: items,
                metadata: json!({
                    "board": row.board,
                    "ticket": row.ticket_ref,
                    "status": row.status,
                    "priority": row.priority,
                }),
                source_href: format!("/boards/{}/{}", row.board_id, row.id),
                brief_status: "fallback".into(),
                actions,
                bucket,
                source_fingerprint: String::new(),
            })
        })
        .collect())
}

// ── Pending Google actions (approvals) ───────────────────────────────────────

pub async fn approval_items(
    pg: &PgPool,
    user: &SessionUser,
    source_id: Option<&str>,
) -> Result<Vec<RawFocusItem>, sqlx::Error> {
    let pending = list_pending(pg, &user.id, user.role == "admin").await?;
    let approvals = match source_id {
        Some(id) => pending
            .into_iter()
            .filter(|a| a.id == id)
            .collect::<Vec<_>>(),
        None => pending,
    };
    Ok(approvals
        .into_iter()
        .map(|approval| {
            let label = if approval.kind == "gmail_send" { "SEND EMAIL" } else { "CREATE EVENT" };
            // The payload's own fields, not a paraphrase: the card recommends
            // "review the exact outbound payload", so the evidence IS the
            // payload. Capped per entry like every other source (1_000) —
            // enough to judge, never enough to bury the card. The actor rides
            // last (4 rows max render).
            let mut evidence = if approval.kind == "gmail_send" {
                approval_evidence_email(&approval.payload)
            } else {
                approval_evidence_event(&approval.payload)
            };
            evidence.push(FocusEvidence {
                label: "Drafted by".into(),
                text: approval.agent_model.clone().unwrap_or_else(|| "Agent".into()),
            });
            let created_at = as_iso(approval.created_ms);
            finalize_item(RawFocusItem {
                key: key_of("approval", &approval.id),
                source_type: "approval".into(),
                source_id: approval.id.clone(),
                priority: "p0".into(),
                status_label: format!("APPROVAL · {label}"),
                created_at: created_at.clone(),
                updated_at: created_at,
                due_at: None,
                question: format!(
                    "{} \u{201c}{}\u{201d}?",
                    if approval.kind == "gmail_send" { "Send" } else { "Create" },
                    approval.summary.clone().unwrap_or_else(|| label.to_lowercase())
                ),
                recommendation: "Review the exact outbound payload before confirming this identity-bearing action.".into(),
                recommended_action_id: Some("approve".into()),
                evidence,
                metadata: json!({
                    "kind": approval.kind,
                    "agent": approval.agent_model,
                    "organizationAction": approval.is_org,
                }),
                source_href: "/".into(),
                brief_status: "fallback".into(),
                actions: vec![
                    // Asymmetric on purpose: approving SENDS something under the
                    // owner's identity, rejecting only declines to. Same note as
                    // the review pair above — 'safe' is about a human's click,
                    // and a model that proposes `reject` still takes the
                    // confirmation path.
                    focus_action(
                        "approve",
                        if approval.kind == "gmail_send" { "Approve send" } else { "Approve event" },
                        "confirmation",
                        None,
                        None,
                    ),
                    focus_action("reject", "Reject", "safe", None, None),
                ],
                bucket: 0,
                source_fingerprint: String::new(),
            })
        })
        .collect())
}

/// An email draft as evidence rows: who gets it, what it says.
fn approval_evidence_email(payload: &Value) -> Vec<FocusEvidence> {
    let s = |k: &str| payload.get(k).and_then(Value::as_str);
    let mut out = Vec::new();
    if let Some(to) = s("to").filter(|t| !t.is_empty()) {
        let cc = s("cc").filter(|c| !c.is_empty());
        let text = match cc {
            Some(cc) => format!("{to} · cc {cc}"),
            None => to.to_string(),
        };
        out.push(evidence("To", &text));
    }
    // `p.subject !== undefined` — an empty subject is still a Subject row.
    if let Some(subject) = payload.get("subject") {
        let text = subject.as_str().unwrap_or_default();
        out.push(evidence("Subject", truncate_utf16(text, 1_000)));
    }
    match s("body").filter(|b| !b.is_empty()) {
        Some(body) => out.push(evidence("Body", body)),
        None => out.push(FocusEvidence {
            label: "Body".into(),
            text: "(empty)".into(),
        }),
    }
    out
}

/// An event draft as evidence rows: when, where, who.
fn approval_evidence_event(payload: &Value) -> Vec<FocusEvidence> {
    let s = |k: &str| payload.get(k).and_then(Value::as_str);
    let mut out = Vec::new();
    let all_day = payload
        .get("allDay")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let (Some(start), Some(end)) = (
        s("start").filter(|v| !v.is_empty()),
        s("end").filter(|v| !v.is_empty()),
    ) {
        let suffix = if all_day { " · all day" } else { "" };
        out.push(evidence("When", &format!("{start} → {end}{suffix}")));
    }
    let mut where_parts: Vec<String> = Vec::new();
    if let Some(location) = s("location").filter(|l| !l.is_empty()) {
        where_parts.push(location.to_string());
    }
    if let Some(attendees) = payload.get("attendees").and_then(Value::as_array) {
        where_parts.extend(
            attendees
                .iter()
                .filter_map(Value::as_str)
                .filter(|a| !a.is_empty())
                .map(str::to_string),
        );
    }
    if !where_parts.is_empty() {
        out.push(evidence("Where / who", &where_parts.join(" · ")));
    }
    if let Some(description) = s("description").filter(|d| !d.is_empty()) {
        out.push(evidence("Notes", description));
    }
    out
}

// ── Unread direct messages (channels) ────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct ChannelRow {
    id: String,
    peer_name: Option<String>,
    peer_email: Option<String>,
    unread_count: i32,
    excerpt: Option<String>,
    author: Option<String>,
    max_seq: i64,
    created_ms: i64,
    updated_ms: i64,
}

pub async fn channel_items(
    pg: &PgPool,
    user_id: &str,
    source_id: Option<&str>,
) -> Result<Vec<RawFocusItem>, sqlx::Error> {
    let rows: Vec<ChannelRow> = sqlx::query_as(
        "select c.id::text, pu.name as peer_name, pu.email as peer_email, \
                (select count(*)::int from channel_messages msg \
                  where msg.channel_id = c.id and msg.seq > member.last_read_seq and msg.status = 'complete' \
                    and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user'))) as unread_count, \
                (select msg.content from channel_messages msg \
                  where msg.channel_id = c.id and msg.seq > member.last_read_seq and msg.status = 'complete' \
                    and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user')) \
                  order by msg.seq desc limit 1) as excerpt, \
                (select msg.author from channel_messages msg \
                  where msg.channel_id = c.id and msg.seq > member.last_read_seq and msg.status = 'complete' \
                    and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user')) \
                  order by msg.seq desc limit 1) as author, \
                c.msg_seq::int8 as max_seq, \
                (trunc(extract(epoch from c.created_at) * 1000))::bigint as created_ms, \
                (trunc(extract(epoch from c.updated_at) * 1000))::bigint as updated_ms \
         from channels c \
         join channel_members member on member.channel_id = c.id and member.user_id = $1::uuid \
         join users self on self.id = $1::uuid \
         left join channel_members peer on c.kind = 'dm' and peer.channel_id = c.id and peer.user_id <> $1::uuid \
         left join users pu on pu.id = peer.user_id \
         where c.archived_at is null and c.kind = 'dm' \
           and ($2::text is null or c.id::text = $2) \
           and exists ( \
             select 1 from channel_messages msg \
             where msg.channel_id = c.id and msg.seq > member.last_read_seq and msg.status = 'complete' \
               and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user'))) \
         order by c.updated_at asc limit 100",
    )
    .bind(user_id)
    .bind(source_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let peer = row
                .peer_name
                .clone()
                .or(row.peer_email.clone())
                .or(row.author.clone())
                .unwrap_or_else(|| "this conversation".to_string());
            finalize_item(RawFocusItem {
                key: key_of("channel", &row.id),
                source_type: "channel".into(),
                source_id: row.id.clone(),
                priority: "p1".into(),
                status_label: format!("DIRECT · {} UNREAD", row.unread_count),
                created_at: as_iso(row.created_ms),
                updated_at: as_iso(row.updated_ms),
                due_at: None,
                question: format!("Reply to {peer}?"),
                recommendation: "Read the latest message, then reply or mark the conversation read.".into(),
                recommended_action_id: Some("reply".into()),
                evidence: vec![FocusEvidence {
                    label: row.author.clone().unwrap_or_else(|| peer.clone()),
                    text: truncate_utf16(
                        row.excerpt.as_deref().unwrap_or("Unread direct message"),
                        1_000,
                    )
                    .to_string(),
                }],
                metadata: json!({ "peer": peer, "unread": row.unread_count, "maxSeq": row.max_seq }),
                source_href: format!("/comms/channel/{}", row.id),
                brief_status: "fallback".into(),
                actions: vec![
                    focus_action("reply", "Reply", "confirmation", None, None),
                    focus_action("mark_read", "Mark read", "reversible", None, None),
                ],
                bucket: 2,
                source_fingerprint: String::new(),
            })
        })
        .collect())
}

// ── Notifications ────────────────────────────────────────────────────────────

pub async fn notification_items(
    pg: &PgPool,
    user_id: &str,
    source_id: Option<&str>,
) -> Result<Vec<RawFocusItem>, sqlx::Error> {
    let kinds: Vec<String> = crate::inbox_focus::policy::ACTIONABLE_NOTIFICATION_KINDS
        .iter()
        .map(|k| k.to_string())
        .collect();
    let rows: Vec<(String, String, String, String, String, i64)> = sqlx::query_as(
        "select id::text, kind, title, body, href, (trunc(extract(epoch from created_at) * 1000))::bigint \
         from notifications \
         where user_id = $1::uuid and read_at is null and kind = any($3) \
           and coalesce(href, '') not in ('', '/') \
           and ($2::text is null or id::text = $2) \
         order by created_at asc limit 200",
    )
    .bind(user_id)
    .bind(source_id)
    .bind(&kinds)
    .fetch_all(pg)
    .await?;

    // ACCESSIBILITY, not just linkability: a row whose href leads to a resource
    // this reader is not on is about somebody else's work, and a list they
    // cannot act from is where it must not appear.
    let hrefs: Vec<String> = rows.iter().map(|r| r.4.clone()).collect();
    let accessible =
        crate::daily_brief::focus::accessible_notification_hrefs(pg, user_id, &hrefs).await?;
    Ok(rows
        .into_iter()
        .filter(|row| accessible.contains(&row.4))
        .map(|(id, kind, title, body, href, created_ms)| {
            let direct = matches!(kind.as_str(), "mention" | "dm" | "agent-outreach");
            let bucket = if direct { 2 } else { 5 };
            let created_at = as_iso(created_ms);
            finalize_item(RawFocusItem {
                key: key_of("notification", &id),
                source_type: "notification".into(),
                source_id: id,
                priority: priority_for_bucket(bucket).into(),
                status_label: kind.replace('-', " ").to_uppercase(),
                created_at: created_at.clone(),
                updated_at: created_at,
                due_at: None,
                question: format!("Review \u{201c}{title}\u{201d}?"),
                recommendation: if direct {
                    "Open the source and respond if needed."
                } else {
                    "Review the source, then mark this item read."
                }
                .into(),
                recommended_action_id: Some("mark_read".into()),
                evidence: vec![evidence(
                    "Source",
                    if body.is_empty() { &title } else { &body },
                )],
                metadata: json!({ "kind": kind }),
                source_href: if href.is_empty() { "/".into() } else { href },
                brief_status: "fallback".into(),
                actions: vec![focus_action(
                    "mark_read",
                    "Mark read",
                    "reversible",
                    None,
                    None,
                )],
                bucket,
                source_fingerprint: String::new(),
            })
        })
        .collect())
}
