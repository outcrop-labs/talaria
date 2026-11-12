// google/pending-actions — confirm-sends: outbound Google actions an agent
// drafted, held for approval. Reads and drafts are free; anything that leaves
// the building (an email, a calendar invite) waits here until a human approves
// — then it executes.
//
//   personal action (a personal assistant, bound to its owner) → the OWNER approves
//   org action (a general agent, on the shared org account)     → an ADMIN approves
//   agent action (the agent's own Google account)               → an ADMIN approves,
//     and the agent's token executes — never the org account's

use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use std::sync::OnceLock;
use talaria_google_agent::get_agent_access_token;
use talaria_google_calendar::{
    CreateEventInput, cancel_event_with_token, create_event_with_token, update_event_with_token,
};
use talaria_google_connections::{TokenError, get_access_token};
use talaria_google_docs::{create_doc_with_token, update_doc_with_token};
use talaria_google_drive::{
    create_drive_folder_with_token, move_drive_file_with_token, rename_drive_file_with_token,
};
use talaria_google_errors::{REASON_MAX_CHARS, upstream_reason};
use talaria_google_gmail::{SendInput, send_message_with_token};
use talaria_google_org::{get_org_access_token, get_org_email, get_org_targets};
use talaria_google_sheets::update_sheet_with_token;
use talaria_google_slides::{TextReplacement, replace_text_with_token};
use talaria_realtime::RealtimeDeps;
use talaria_secretbox::SecretBox;

pub static ANNOUNCE_APPROVAL: OnceLock<
    Arc<dyn Fn(sqlx::PgPool, RealtimeDeps, String) -> BoxFuture<'static, ()> + Send + Sync>,
> = OnceLock::new();

/// One held outbound action. `created_ms` epoch — every caller renders it
/// through `as_iso`, which is what the wire's `createdAt` is.
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub id: String,
    pub kind: String,
    pub summary: Option<String>,
    pub payload: Value,
    pub agent_model: Option<String>,
    pub owner_user_id: Option<String>,
    pub is_org: bool,
    /// "owner" | "org" | "agent". Who approves is `is_org`; this is whose
    /// token executes. An agent identity is admin-approved and still not the
    /// org account.
    pub principal_kind: String,
    pub status: String,
    pub created_ms: i64,
    /// Why the last approval of this still-pending row did not run — the
    /// executor's release sentence, quoting Google when Google said why
    /// (`result.error`). None on a row nobody has tried yet.
    pub last_error: Option<String>,
}

/// One row as every pending read selects it: the columns of
/// `pending_columns!`, in order.
type PendingRow = (
    String,
    String,
    Option<String>,
    Value,
    Option<String>,
    Option<String>,
    bool,
    String,
    String,
    i64,
    Option<String>,
);

/// The select list behind `PendingRow` — a macro, not a const, so every
/// query stays one `concat!`ed static string (sqlx's static-string contract).
macro_rules! pending_columns {
    () => {
        "id::text, kind, summary, payload, agent_model, owner_user_id::text, is_org, \
         principal_kind, status, (trunc(extract(epoch from created_at) * 1000))::bigint, \
         result->>'error'"
    };
}

impl From<PendingRow> for PendingAction {
    fn from(row: PendingRow) -> Self {
        let (
            id,
            kind,
            summary,
            payload,
            agent_model,
            owner_user_id,
            is_org,
            principal_kind,
            status,
            created_ms,
            last_error,
        ) = row;
        PendingAction {
            id,
            kind,
            summary,
            payload,
            agent_model,
            owner_user_id,
            is_org,
            principal_kind,
            status,
            created_ms,
            last_error,
        }
    }
}

/// What a queueing carries. `payload` is the engine input verbatim — a
/// SendInput's to/subject/body/cc/bcc or a CreateEventInput's fields — stored
/// as drafted and executed as stored at approve time.
pub struct QueueAction<'a> {
    pub kind: &'a str,
    pub summary: &'a str,
    pub payload: &'a Value,
    pub agent_model: &'a str,
    /// Personal actions carry the owner; org actions leave it null and set
    /// `is_org` (an admin approves those).
    pub owner_user_id: Option<&'a str>,
    pub is_org: bool,
    /// Executor identity stored on the row. "owner" | "org" | "agent".
    pub principal_kind: &'a str,
}

/// What a queueing produced. `already_pending` marks the converge case: the
/// agent drafted an email a still-pending row already asks about, and the
/// row it came back with is THAT row — nothing new was queued.
pub struct QueuedAction {
    pub action: PendingAction,
    pub already_pending: bool,
}

/// An agent-drafted outbound action lands here, pending. `realtime` is the
/// fan-out the announce rides; the announce itself is detached and silent
/// (see the spawn below).
///
/// ONE PENDING DECISION PER DRAFT. An agent run that retried — a wedged
/// request answered late, a crash after the queue said yes — re-drafts the
/// same email, and every draft became its own p0 card: the dogfood brief
/// held six SEND EMAIL lines for one reply, and approving two of them was a
/// double-send to a client. So a gmail_send naming the same people in the
/// same subject as a row already pending returns THAT row and queues
/// nothing; the caller's answer says an identical draft is waiting. A
/// decided row never matches — after a reject, a fresh draft is a fresh ask.
pub async fn queue_action(
    pg: &PgPool,
    realtime: RealtimeDeps,
    input: &QueueAction<'_>,
) -> Result<QueuedAction, String> {
    if let Some(action) = find_same_pending_draft(pg, input).await? {
        return Ok(QueuedAction {
            action,
            already_pending: true,
        });
    }
    let row: PendingRow = sqlx::query_as(concat!(
        "insert into google_pending_actions \
             (kind, summary, payload, agent_model, owner_user_id, is_org, principal_kind) \
         values ($1, $2, $3, $4, $5::uuid, $6, $7) \
         returning ",
        pending_columns!()
    ))
    .bind(input.kind)
    .bind(input.summary)
    .bind(input.payload)
    .bind(input.agent_model)
    .bind(input.owner_user_id)
    .bind(input.is_org)
    .bind(input.principal_kind)
    .fetch_one(pg)
    .await
    .map_err(|e| format!("google pending action insert: {e}"))?;
    let action = PendingAction::from(row);
    let id = action.id.clone();

    // The row IS an approval the instant it lands, and the agent that drafted
    // it is stopped in front of it. Until this call existed the first a human
    // heard was the approval sweep's next tick — up to five minutes of an
    // agent waiting on a decision that takes four seconds, and before the
    // sweep existed, the next morning's digest.
    //
    // Detached: this function is servicing an agent's request and must not
    // wait on notification writes, nor fail because one of them failed.
    // Idempotent against the sweep — both mark `approval_announce_state` by
    // key, and the mark is merged in the database rather than written over,
    // so this is latency removed and never a second notification. Failures
    // inside the announce log themselves; the sweep is the floor either way.
    let announce_pg = pg.clone();
    tokio::spawn(async move {
        if let Some(f) = ANNOUNCE_APPROVAL.get() {
            f(announce_pg, realtime, format!("google_action:{id}")).await;
        }
    });
    Ok(QueuedAction {
        action,
        already_pending: false,
    })
}

async fn find_same_pending_draft(
    pg: &PgPool,
    input: &QueueAction<'_>,
) -> Result<Option<PendingAction>, String> {
    if input.kind != "gmail_send" {
        return Ok(None);
    }
    let Some(signature) = gmail_signature(input.payload) else {
        return Ok(None);
    };
    let rows: Vec<PendingRow> = sqlx::query_as(concat!(
        "select ",
        pending_columns!(),
        " from google_pending_actions \
          where kind = 'gmail_send' and status = 'pending' \
            and is_org = $1 and owner_user_id is not distinct from $2::uuid \
          order by created_at desc"
    ))
    .bind(input.is_org)
    .bind(input.owner_user_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("google pending action dedupe read: {e}"))?;
    let pending: Vec<PendingAction> = rows.into_iter().map(PendingAction::from).collect();
    Ok(pending
        .into_iter()
        .find(|action| gmail_signature(&action.payload).as_deref() == Some(signature.as_str())))
}

/// The human decision a gmail_send draft carries, with formatting boiled
/// off: the email addresses it names (lowercased, order-free) and the
/// subject with whitespace collapsed. A re-draft rewrites the body and
/// reformats the recipients — "Gareth Lynch <Gareth@x>" becomes
/// "Gareth@x" — while asking exactly the same question, so exact payload
/// equality never fires on real retries. The BODY is deliberately out: a
/// reworded draft is the same decision for the human, and putting it in
/// would rebuild the six-cards bug one retry at a time.
fn gmail_signature(payload: &Value) -> Option<String> {
    let get = |k: &str| payload.get(k).and_then(Value::as_str);
    let mut addresses: Vec<String> = ["to", "cc", "bcc"]
        .into_iter()
        .filter_map(get)
        .flat_map(|field| field.split(','))
        .filter_map(address_of)
        .collect();
    addresses.sort();
    addresses.dedup();
    if addresses.is_empty() {
        return None;
    }
    Some(format!(
        "{}\u{1f}{}",
        addresses.join(","),
        collapse_whitespace(get("subject").unwrap_or_default())
    ))
}

/// The address inside `Name <a@b.c>`, or the token itself when bare; None
/// for anything without an @ (a display fragment, an empty field).
fn address_of(raw: &str) -> Option<String> {
    let token = raw.trim();
    if token.is_empty() {
        return None;
    }
    let inner = token
        .strip_suffix('>')
        .and_then(|rest| rest.rsplit_once('<'))
        .map(|(_, addr)| addr)
        .unwrap_or(token);
    let addr = inner.trim();
    addr.contains('@').then(|| addr.to_lowercase())
}

/// Runs of whitespace folded to one space, ends trimmed.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(ch);
        }
    }
    out
}

/// A pending row as the route answers it — camelCase, `createdAt` an ISO
/// instant.
pub fn pending_wire(a: &PendingAction) -> Value {
    json!({
        "id": a.id,
        "kind": a.kind,
        "summary": a.summary,
        "payload": a.payload,
        "agentModel": a.agent_model,
        "ownerUserId": a.owner_user_id,
        "isOrg": a.is_org,
        "principalKind": a.principal_kind,
        "status": a.status,
        "createdAt": talaria_agent_auth::epoch_ms_to_iso(a.created_ms),
        "lastError": a.last_error,
    })
}

/// The actions a user should decide: their own personal ones, plus — for an
/// admin — the org-scoped ones. Newest first.
pub async fn list_pending(
    pg: &PgPool,
    user_id: &str,
    is_admin: bool,
) -> Result<Vec<PendingAction>, sqlx::Error> {
    // The admin arm must VANISH for a member, not become `is_org = false`
    // (which would match every other person's personal actions). The literal
    // query keeps sqlx's static-string contract; `$2 and …` gates the arm so a
    // member's false bind short-circuits it to no rows, exactly like an
    // absent clause.
    let rows: Vec<PendingRow> = sqlx::query_as(concat!(
        "select ",
        pending_columns!(),
        " from google_pending_actions \
          where status = 'pending' \
            and ((is_org = false and owner_user_id = $1::uuid) or ($2 and is_org = true)) \
          order by created_at desc"
    ))
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(PendingAction::from).collect())
}
/// The confirm-sends queue the approver of this draft actually opens.
///
/// Personal → `list_pending(owner, admin=false)`, the same read the owner's
/// Inbox and brief use. The admin arm stays off: being an admin is not what
/// puts a personal draft in front of its owner.
///
/// Org → the org arm of that same predicate (`is_org` and pending), which is
/// what every admin's list includes for these rows. The nil user id cannot
/// match a personal row; the `is_org` filter drops anything else.
pub async fn list_for_approver(
    pg: &PgPool,
    owner_user_id: Option<&str>,
    is_org: bool,
) -> Result<Vec<PendingAction>, sqlx::Error> {
    if is_org {
        let rows = list_pending(pg, "00000000-0000-0000-0000-000000000000", true).await?;
        return Ok(rows.into_iter().filter(|action| action.is_org).collect());
    }
    match owner_user_id {
        Some(owner) => list_pending(pg, owner, false).await,
        None => Ok(Vec::new()),
    }
}

/// `id` is in the approver's queue only when that queue returned it still
/// pending. The insert's RETURNING row is not that proof.
pub fn queued_in(id: &str, listed: &[PendingAction]) -> bool {
    listed
        .iter()
        .any(|action| action.id == id && action.status == "pending")
}

/// Why a just-queued row must not be reported as ready. `Ok(None)` means the
/// approver's queue contains it.
pub async fn queue_proof_failure(
    pg: &PgPool,
    action: &PendingAction,
) -> Result<Option<String>, String> {
    if action.status != "pending" {
        return Ok(Some(format!(
            "draft {} is {}, not pending — it is not in the confirm-sends queue. Do not report it as ready.",
            action.id, action.status
        )));
    }
    if !action.is_org && action.owner_user_id.is_none() {
        return Ok(Some(format!(
            "draft {} has no owner — a personal confirm-send with no owner is visible to nobody. Do not report it as ready.",
            action.id
        )));
    }
    let listed = list_for_approver(pg, action.owner_user_id.as_deref(), action.is_org)
        .await
        .map_err(|e| format!("confirm-sends queue read failed: {e}"))?;
    if queued_in(&action.id, &listed) {
        return Ok(None);
    }
    let who = if action.is_org {
        "an admin".to_string()
    } else {
        format!(
            "owner {}",
            action.owner_user_id.as_deref().unwrap_or("unknown")
        )
    };
    Ok(Some(format!(
        "draft {} is not in the confirm-sends queue {who} sees — nothing should be reported as ready.",
        action.id
    )))
}

/// Drop a row we just inserted that the approver's queue does not show.
/// Only a still-pending row: a decision that landed between the proof and
/// the discard is not ours to delete.
pub async fn discard_unproven(pg: &PgPool, id: &str) -> Result<(), String> {
    sqlx::query("delete from google_pending_actions where id = $1::uuid and status = 'pending'")
        .bind(id)
        .execute(pg)
        .await
        .map_err(|e| format!("discard unproven draft: {e}"))?;
    Ok(())
}

/// What a decision produced. `{status, message?}` — message present only on
/// the not-connected/failed statuses.
#[derive(Debug, Clone)]
pub struct DecideOutcome {
    pub status: String,
    pub message: Option<String>,
}

/// Approve → execute (as the owner, or the org for org actions), or reject →
/// drop. `Err` is every hard failure (a DB error, a token read that failed
/// rather than answered null); the caller's catch is the only thing that
/// should see it.
pub async fn decide_action(
    pg: &PgPool,
    sb: &SecretBox,
    action_id: &str,
    actor_id: &str,
    actor_is_admin: bool,
    decision: &str,
    now_ms: i64,
) -> Result<Option<DecideOutcome>, String> {
    #[allow(clippy::type_complexity)] // the decided row's own columns, one each
    let action: Option<(
        String,
        Value,
        Option<String>,
        Option<String>,
        bool,
        String,
        String,
    )> = sqlx::query_as(
        "select kind, payload, agent_model, owner_user_id::text, is_org, principal_kind, status \
         from google_pending_actions where id = $1::uuid",
    )
    .bind(action_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("google pending action read: {e}"))?;
    let Some((kind, payload, agent_model, owner_user_id, is_org, principal_kind, status)) = action
    else {
        return Ok(None);
    };
    let authorized = if is_org {
        actor_is_admin
    } else {
        owner_user_id.as_deref() == Some(actor_id)
    };
    if !authorized {
        return Ok(Some(DecideOutcome {
            status: "forbidden".into(),
            message: None,
        }));
    }
    if status != "pending" {
        return Ok(Some(DecideOutcome {
            status,
            message: None,
        })); // already decided
    }
    // CLAIM before anything runs. Two approvals of one row inside the Google
    // round-trip — two admins on an org send, one owner on the chip and the
    // inbox card — both used to read `pending` and both sent. The claim is
    // one conditional update: whoever flips the row to `executing` runs it,
    // and the other sees the row already decided. A claim older than ten
    // minutes is a process that died mid-send; it may be taken over.
    let claimed: Option<(String,)> = sqlx::query_as(
        "update google_pending_actions \
         set status = 'executing', decided_by = $2::uuid, decided_at = now() \
         where id = $1::uuid \
           and (status = 'pending' \
                or (status = 'executing' and decided_at < now() - interval '10 minutes')) \
         returning status",
    )
    .bind(action_id)
    .bind(actor_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("google pending action claim: {e}"))?;
    if claimed.is_none() {
        let now: Option<String> =
            sqlx::query_scalar("select status from google_pending_actions where id = $1::uuid")
                .bind(action_id)
                .fetch_optional(pg)
                .await
                .map_err(|e| format!("google pending action re-read: {e}"))?;
        return Ok(now.map(|status| DecideOutcome {
            status,
            message: None,
        })); // decided (or claimed) by someone else between the read and here
    }

    // A terminal decision resolves the approval line on the owner's (and the
    // decider's) brief. Detached and silent: the decider is waiting on this
    // response. Org actions decided by one admin leave other admins' briefs
    // to the scheduled sweep — the nudge is an optimization, never the floor.
    let nudge_brief = |ids: Vec<String>| {
        let pg = pg.clone();
        tokio::spawn(async move {
            let deps = talaria_notify::NotifyDeps::publishing(pg, None);
            let _ = talaria_notify::mark_brief_stale(&deps, &ids).await;
        });
    };
    let stale_ids: Vec<String> = [owner_user_id.clone(), Some(actor_id.to_string())]
        .into_iter()
        .flatten()
        .collect();

    if decision == "reject" {
        // The claim above already moved the row to `executing` under this
        // actor; a reject lands on that claim and nothing else.
        sqlx::query(
            "update google_pending_actions set status = 'rejected', decided_at = now(), decided_by = $2::uuid \
             where id = $1::uuid and status = 'executing'",
        )
        .bind(action_id)
        .bind(actor_id)
        .execute(pg)
        .await
        .map_err(|e| format!("google pending action update: {e}"))?;
        nudge_brief(stale_ids);
        return Ok(Some(DecideOutcome {
            status: "rejected".into(),
            message: None,
        }));
    }

    // Approve → the token of the identity that was queued, not whichever
    // account is_org happens to imply. Agent rows are admin-approved
    // (is_org) and still execute as the agent's own Google account.
    let vended = match principal_kind.as_str() {
        "agent" => {
            get_agent_access_token(
                pg,
                sb,
                agent_model
                    .as_deref()
                    .expect("agent-identity actions carry an agent model"),
                now_ms,
            )
            .await
        }
        "owner" => {
            get_access_token(
                pg,
                sb,
                owner_user_id
                    .as_deref()
                    .expect("personal actions carry an owner"),
                now_ms,
            )
            .await
        }
        _ => get_org_access_token(pg, sb, now_ms).await,
    };
    let reconnect = match principal_kind.as_str() {
        "agent" => "Reconnect this agent's Google account on its record to run this.",
        "org" => "Reconnect the org Google account to run this.",
        _ => "Reconnect Google to run this action.",
    };
    let token = match vended {
        Ok(Some(token)) => token,
        // Nothing ran: hand the row back so the same approval works once the
        // account is reconnected.
        Ok(None) => {
            release_claim(pg, action_id, reconnect).await?;
            return Ok(Some(DecideOutcome {
                status: "not_connected".into(),
                message: Some(reconnect.to_string()),
            }));
        }
        // The token itself failed. Nothing ran either, so the row goes back
        // too — a `?` here used to leave it `executing` until the ten-minute
        // takeover, with the approver told only "decide failed". Google's
        // words, when it sent any, are the answer and stay on the row for
        // the agent's next read; anything else is still the hard failure it
        // was (the log keeps the detail), released.
        Err(e) => {
            let raw = e.to_string();
            let Some(message) = token_refusal_message(&e, reconnect) else {
                release_claim(pg, action_id, TOKEN_UNAVAILABLE).await?;
                return Err(raw);
            };
            tracing::warn!("[google/pending] {action_id} token refused: {raw}");
            release_claim(pg, action_id, &message).await?;
            return Ok(Some(DecideOutcome {
                status: "not_connected".into(),
                message: Some(message),
            }));
        }
    };

    // Org actions land on the configured shared targets (calendar / send-as
    // alias). An agent identity uses its own account — no org send-as, no
    // org calendar — even though an admin approves it.
    let targets = if principal_kind == "org" {
        get_org_targets(pg).await.map_err(|e| e.to_string())?
    } else {
        Default::default()
    };
    let agent_from = if principal_kind == "org" && kind == "gmail_send" && agent_model.is_some() {
        org_agent_from_address(pg, agent_model.as_deref().expect("checked some"))
            .await
            .map_err(|e| e.to_string())?
    } else {
        None
    };

    // The payload IS the engine input (SendInput / CreateEventInput) — read
    // back exactly as it was queued.
    let executed: Result<Value, String> = if kind == "gmail_send" {
        let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or_default();
        let input = SendInput {
            to: s("to"),
            subject: s("subject"),
            body: s("body"),
            cc: payload.get("cc").and_then(Value::as_str),
            bcc: payload.get("bcc").and_then(Value::as_str),
        };
        send_message_with_token(
            &token,
            &input,
            agent_from.as_deref().or(targets.send_as.as_deref()),
        )
        .await
        .map(|(id, thread_id)| json!({ "id": id, "threadId": thread_id }))
        .map_err(|e| e.to_string())
    } else if kind == "calendar_create" {
        let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or_default();
        let input = CreateEventInput {
            summary: s("summary"),
            description: payload.get("description").and_then(Value::as_str),
            location: payload.get("location").and_then(Value::as_str),
            start: s("start"),
            end: s("end"),
            all_day: payload
                .get("allDay")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            attendees: payload
                .get("attendees")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            meet: payload
                .get("meet")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        create_event_with_token(&token, &input, targets.calendar_id.as_deref())
            .await
            .map(|e| serde_json::to_value(&e).unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    } else if kind == "doc_update" {
        let file_id = payload
            .get("fileId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if file_id.is_empty() {
            Err("doc_update missing fileId".into())
        } else {
            match update_doc_with_token(
                &token,
                file_id,
                payload
                    .get("markdown")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                payload.get("title").and_then(Value::as_str),
            )
            .await
            {
                Ok(doc) => {
                    // The agent's token wrote the body: stamp it, so the
                    // write itself is not mistaken for a person's edit next
                    // time (a no-op unless the agent still owns the file).
                    if let Some(agent) = agent_model.as_deref() {
                        stamp_agent_write(pg, agent, file_id)
                            .await
                            .map_err(|e| format!("agent write stamp: {e}"))?;
                    }
                    Ok(json!({ "id": doc.id, "url": doc.url, "name": doc.name }))
                }
                Err(e) => Err(e.to_string()),
            }
        }
    } else if kind == "slides_update" {
        let file_id = payload
            .get("fileId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // The pairs were validated by the route before they were queued; a
        // malformed entry here is a corrupted payload, so it is skipped rather
        // than failing a write the approver already said yes to.
        let owned: Vec<(String, String)> = payload
            .get("replacements")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|it| {
                        Some((
                            it.get("find")?.as_str()?.to_string(),
                            it.get("replace")?.as_str().unwrap_or("").to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if file_id.is_empty() || owned.is_empty() {
            Err("slides_update missing fileId or replacements".into())
        } else {
            let pairs: Vec<TextReplacement<'_>> = owned
                .iter()
                .map(|(f, r)| TextReplacement {
                    find: f.as_str(),
                    replace: r.as_str(),
                })
                .collect();
            let match_case = payload
                .get("matchCase")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            replace_text_with_token(&token, file_id, &pairs, match_case)
                .await
                .map(|up| {
                    json!({
                        "id": up.id,
                        "url": up.url,
                        "occurrencesChanged": up.occurrences_changed,
                    })
                })
                .map_err(|e| e.to_string())
        }
    } else if kind == "sheet_update" {
        let file_id = payload
            .get("fileId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let range = payload
            .get("range")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if file_id.is_empty() || range.is_empty() {
            Err("sheet_update missing fileId or range".into())
        } else {
            // The rows were normalised to Vec<Vec<String>> by the route
            // before they were queued, so a row that is not an array here
            // would be a corrupted payload, not a caller mistake — it reads
            // as an empty row rather than failing the whole write.
            let rows: Vec<Vec<String>> = payload
                .get("rows")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .map(|row| {
                            row.as_array()
                                .map(|cells| {
                                    cells
                                        .iter()
                                        .map(|c| {
                                            c.as_str().map(str::to_string).unwrap_or_else(|| {
                                                if c.is_null() {
                                                    String::new()
                                                } else {
                                                    c.to_string()
                                                }
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .unwrap_or_default();
            update_sheet_with_token(&token, file_id, range, &rows)
                .await
                .map(|up| {
                    json!({
                        "id": up.id,
                        "url": up.url,
                        "range": up.range,
                        "updatedCells": up.updated_cells,
                    })
                })
                .map_err(|e| e.to_string())
        }
    } else if kind == "drive_rename" {
        let file_id = payload
            .get("fileId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if file_id.is_empty() || name.is_empty() {
            Err("drive_rename missing fileId or name".into())
        } else {
            rename_drive_file_with_token(&token, file_id, name)
                .await
                .map(|entry| json!({ "id": entry.id, "name": entry.name, "url": entry.web_view_link }))
                .map_err(|e| e.to_string())
        }
    } else if kind == "drive_move" {
        let file_id = payload
            .get("fileId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parent = payload
            .get("parentId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        if file_id.is_empty() {
            Err("drive_move missing fileId".into())
        } else {
            move_drive_file_with_token(&token, file_id, parent, None)
                .await
                .map(|_| json!({ "id": file_id, "parentId": parent }))
                .map_err(|e| e.to_string())
        }
    } else if kind == "calendar_update" {
        let event_id = payload
            .get("eventId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if event_id.is_empty() {
            Err("calendar_update missing eventId".into())
        } else {
            update_event_with_token(
                &token,
                targets.calendar_id.as_deref(),
                event_id,
                payload.get("summary").and_then(Value::as_str),
                payload.get("start").and_then(Value::as_str),
                payload.get("end").and_then(Value::as_str),
                payload
                    .get("allDay")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
            .await
            .map(|e| serde_json::to_value(&e).unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
        }
    } else if kind == "calendar_cancel" {
        let event_id = payload
            .get("eventId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if event_id.is_empty() {
            Err("calendar_cancel missing eventId".into())
        } else {
            cancel_event_with_token(&token, targets.calendar_id.as_deref(), event_id)
                .await
                .map(|_| json!({ "id": event_id, "status": "cancelled" }))
                .map_err(|e| e.to_string())
        }
    } else if kind == "meeting_create" {
        let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or_default();
        let input = CreateEventInput {
            summary: s("summary"),
            description: payload.get("description").and_then(Value::as_str),
            location: payload.get("location").and_then(Value::as_str),
            start: s("start"),
            end: s("end"),
            all_day: payload
                .get("allDay")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            attendees: payload
                .get("attendees")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            meet: true,
        };
        match create_event_with_token(&token, &input, targets.calendar_id.as_deref()).await {
            Ok(event) => {
                let mut result = serde_json::to_value(&event).unwrap_or(Value::Null);
                // Agenda is created here, at decision time, never at queue time.
                if let Some(agenda) = payload
                    .get("agenda")
                    .and_then(Value::as_str)
                    .filter(|a| !a.is_empty())
                {
                    match create_doc_with_token(&token, s("summary"), agenda, None).await {
                        Ok(doc) => {
                            if let Some(obj) = result.as_object_mut() {
                                obj.insert("agendaUrl".into(), json!(doc.url));
                                obj.insert("agendaId".into(), json!(doc.id));
                            }
                        }
                        Err(e) => {
                            if let Some(obj) = result.as_object_mut() {
                                obj.insert("agendaError".into(), json!(e.to_string()));
                            }
                        }
                    }
                }
                Ok(result)
            }
            Err(e) => Err(e.to_string()),
        }
    } else if kind == "doc_create" {
        let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or_default();
        match create_doc_with_token(
            &token,
            s("title"),
            s("markdown"),
            Some(s("folderId")).filter(|f| !f.is_empty()),
        )
        .await
        {
            Ok(doc) => {
                // The approved doc is the agent's to edit, like one it made
                // without a parent — the record is what `update_google_doc`
                // asks.
                if let Some(agent) = agent_model.as_deref() {
                    record_agent_created(pg, agent, &doc.id)
                        .await
                        .map_err(|e| format!("agent-created doc record: {e}"))?;
                }
                Ok(
                    json!({ "id": doc.id, "url": doc.url, "name": doc.name, "mimeType": doc.mime_type }),
                )
            }
            Err(e) => Err(e.to_string()),
        }
    } else if kind == "drive_folder_create" {
        let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or_default();
        match create_drive_folder_with_token(
            &token,
            s("name"),
            Some(s("parentId")).filter(|p| !p.is_empty()),
        )
        .await
        {
            Ok(folder) => {
                if let Some(agent) = agent_model.as_deref() {
                    record_agent_created(pg, agent, &folder.id)
                        .await
                        .map_err(|e| format!("agent-created folder record: {e}"))?;
                }
                Ok(json!({ "id": folder.id, "name": folder.name, "url": folder.web_view_link }))
            }
            Err(e) => Err(e.to_string()),
        }
    } else {
        Err(format!("unknown action kind: {kind}"))
    };

    match executed {
        Ok(result) => {
            sqlx::query(
                "update google_pending_actions \
                 set status = 'executed', result = $2, decided_at = now(), decided_by = $3::uuid \
                 where id = $1::uuid and status = 'executing'",
            )
            .bind(action_id)
            .bind(&result)
            .bind(actor_id)
            .execute(pg)
            .await
            .map_err(|e| format!("google pending action update: {e}"))?;
            nudge_brief(stale_ids);
            Ok(Some(DecideOutcome {
                status: "executed".into(),
                message: None,
            }))
        }
        Err(message) => {
            sqlx::query(
                "update google_pending_actions \
                 set status = 'failed', result = $2, decided_at = now(), decided_by = $3::uuid \
                 where id = $1::uuid and status = 'executing'",
            )
            .bind(action_id)
            .bind(json!({ "error": message }))
            .bind(actor_id)
            .execute(pg)
            .await
            .map_err(|e| format!("google pending action update: {e}"))?;
            nudge_brief(stale_ids);
            Ok(Some(DecideOutcome {
                status: "failed".into(),
                message: Some(failure_message(&message)),
            }))
        }
    }
}

/// What a released row says when the token failed without Google saying why
/// (the refresh never got an answer, or a local fault): the detail is the
/// log's, not the queue's.
const TOKEN_UNAVAILABLE: &str =
    "Nothing ran: a Google token could not be had for this account. Approve again to retry.";

/// Hand a claimed row back to `pending`, with why on it: the sentence the
/// approver was answered, kept as `result.error` so the agent's next read of
/// the queue (`lastError`) says why its approved action has not run.
async fn release_claim(pg: &PgPool, action_id: &str, why: &str) -> Result<(), String> {
    sqlx::query(
        "update google_pending_actions \
         set status = 'pending', decided_by = null, decided_at = null, result = $2 \
         where id = $1::uuid and status = 'executing'",
    )
    .bind(action_id)
    .bind(json!({ "error": why }))
    .execute(pg)
    .await
    .map_err(|e| format!("google pending action release: {e}"))?;
    Ok(())
}

/// The sentence a refused token answers with, or None when Google said
/// nothing (a transport or local fault — the caller's hard-failure rung).
/// A refused grant keeps the principal's reconnect sentence; a refused
/// CLIENT is the admin's to fix, so it names that instead — reconnecting
/// would not help.
pub fn token_refusal_message(e: &TokenError, reconnect: &str) -> Option<String> {
    let reason = upstream_reason(&e.to_string())?;
    Some(match e {
        TokenError::ClientRejected(_) => format!(
            "Google refused this server's OAuth client: «{reason}». Nothing ran; an admin needs to check the client under Admin → Google Workspace."
        ),
        TokenError::InvalidGrant(_) => {
            format!("Google refused the stored Google connection: «{reason}». {reconnect}")
        }
        TokenError::Other(_) => {
            format!(
                "Google refused the token refresh: «{reason}». Nothing ran; approve again to retry."
            )
        }
    })
}

/// What a failed execution tells the approver: Google's words when the
/// failure carries them, otherwise what failed — never a bare "Google
/// rejected the action" over a fault that was not Google's.
pub fn failure_message(raw: &str) -> String {
    match upstream_reason(raw) {
        Some(reason) => format!("Google rejected the action: «{reason}»"),
        None => {
            let said: String = raw.chars().take(REASON_MAX_CHARS).collect();
            format!("The action did not run: «{said}»")
        }
    }
}

/// A Drive file this agent brought into being — a Doc or a folder — which it
/// may then edit, or create inside, without another card. The file id is
/// Drive's; on conflict the first creator is kept, and only that creator's
/// own re-record moves the write stamp. Creating is the first agent write,
/// so the row is born stamped.
pub async fn record_agent_created(
    pg: &PgPool,
    agent_model: &str,
    file_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into agent_created_google_docs (file_id, agent_model, last_agent_write_at) \
         values ($1, $2, now()) \
         on conflict (file_id) do update set last_agent_write_at = now() \
         where agent_created_google_docs.agent_model = excluded.agent_model",
    )
    .bind(file_id)
    .bind(agent_model)
    .execute(pg)
    .await?;
    Ok(())
}

/// The agent wrote the file's body just now. Every path that puts the
/// agent's token to a Doc it created calls this after Drive answered, so the
/// stamp is never earlier than the revision it describes; the next replace
/// compares Drive's `modifiedTime` against it. A no-op for a file this agent
/// does not own.
pub async fn stamp_agent_write(
    pg: &PgPool,
    agent_model: &str,
    file_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update agent_created_google_docs set last_agent_write_at = now() \
         where file_id = $1 and agent_model = $2",
    )
    .bind(file_id)
    .bind(agent_model)
    .execute(pg)
    .await?;
    Ok(())
}

/// What is known about a Drive file this agent created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCreated {
    /// The agent's last write, epoch milliseconds; `None` on a row recorded
    /// before the stamp existed and not written since.
    pub last_agent_write_ms: Option<i64>,
}

/// The agent-created record for a Drive file, or `None` when this agent did
/// not create it (or a person has since taken it over).
pub async fn agent_created_record(
    pg: &PgPool,
    agent_model: &str,
    file_id: &str,
) -> Result<Option<AgentCreated>, sqlx::Error> {
    let row: Option<(Option<f64>,)> = sqlx::query_as(
        "select extract(epoch from last_agent_write_at) * 1000 \
         from agent_created_google_docs where file_id = $1 and agent_model = $2",
    )
    .bind(file_id)
    .bind(agent_model)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(ms,)| AgentCreated {
        last_agent_write_ms: ms.map(|v| v as i64),
    }))
}

/// Whether this agent created the Drive file (a Doc, or a folder it may
/// place files in).
pub async fn agent_created(
    pg: &PgPool,
    agent_model: &str,
    file_id: &str,
) -> Result<bool, sqlx::Error> {
    Ok(agent_created_record(pg, agent_model, file_id)
        .await?
        .is_some())
}

/// A human took the file over: the agent's standing to edit it without a
/// card ends here, and the next edit queues like any other.
pub async fn forget_agent_created(
    pg: &PgPool,
    agent_model: &str,
    file_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from agent_created_google_docs where file_id = $1 and agent_model = $2")
        .bind(file_id)
        .bind(agent_model)
        .execute(pg)
        .await?;
    Ok(())
}

// ── Org agent addressing (the send-path half) ─────────────────────────────────

async fn agent_address_identity(
    pg: &PgPool,
    model: &str,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("select slug, email_alias from agent_defs where model = $1")
            .bind(model)
            .fetch_optional(pg)
            .await?;
    Ok(row)
}

async fn org_agent_from_address(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    let identity = agent_address_identity(pg, agent_model).await?;
    let Some((slug, email_alias)) = identity else {
        return Ok(None);
    };
    let org_email = get_org_email(pg).await?;
    Ok(agent_from_address(
        slug.as_str(),
        email_alias.as_deref(),
        org_email.as_deref(),
    ))
}

/// A slug folded into a plus-address tag: lowercase, [a-z0-9-], single
/// dashes, nothing else.
fn plus_tag(slug: &str) -> String {
    let lower = slug.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut pending_dash = false;
    for ch in lower.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The org account's plus-address for a tag. An org email that is ITSELF
/// plus-addressed has its tag replaced, not stacked.
fn plus_address(org_email: &str, tag: &str) -> Option<String> {
    let at = org_email.rfind('@')?;
    if at < 1 || at == org_email.len() - 1 {
        return None;
    }
    let local = org_email[..at].split('+').next().unwrap_or_default();
    let domain = &org_email[at + 1..];
    let clean = plus_tag(tag);
    if local.is_empty() || domain.is_empty() || clean.is_empty() {
        return None;
    }
    Some(format!("{local}+{clean}@{domain}"))
}

/// Override, else derived plus-address, else null. Never called for personal
/// assistants — they send as their owner's account, where no alias applies.
/// Public for the provisioning read, which shows every org agent's effective
/// address.
pub fn agent_from_address(
    slug: &str,
    email_alias: Option<&str>,
    org_email: Option<&str>,
) -> Option<String> {
    let override_ = email_alias.map(str::trim).filter(|s| !s.is_empty());
    if let Some(o) = override_ {
        return Some(o.to_string());
    }
    let org = org_email?;
    plus_address(org, slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_boils_the_formatting_off_a_redraft() {
        // The three live rows that became six brief cards differed exactly
        // this way: display names dropped, case flipped, reworded body.
        let a = json!({
            "to": "Gareth Lynch <Gareth@cooperbev.com>, Toby@cooperbev.com",
            "cc": "Luke Tierney <luke@growwithwaypoint.com>",
            "subject": "Re: Proposal & Deck",
            "body": "Just circling back on the proposal and deck I sent over on Tuesday."
        });
        let b = json!({
            "to": "gareth@cooperbev.com, toby@cooperbev.com",
            "cc": "luke@growwithwaypoint.com",
            "subject": "Re:  Proposal\t& Deck ",
            "body": "Just wanted to circle back and make sure you both got the proposal."
        });
        assert_eq!(gmail_signature(&a), gmail_signature(&b));
    }

    #[test]
    fn signature_splits_on_the_fields_that_change_the_ask() {
        let base = json!({"to": "a@x.com", "subject": "Hi", "body": "b"});
        let other_to = json!({"to": "b@x.com", "subject": "Hi", "body": "b"});
        let other_subject = json!({"to": "a@x.com", "subject": "Bye", "body": "b"});
        let other_cc = json!({"to": "a@x.com", "cc": "c@x.com", "subject": "Hi", "body": "b"});
        assert_ne!(gmail_signature(&base), gmail_signature(&other_to));
        assert_ne!(gmail_signature(&base), gmail_signature(&other_subject));
        assert_ne!(gmail_signature(&base), gmail_signature(&other_cc));
    }

    #[test]
    fn signature_recipient_order_is_not_the_ask() {
        let a = json!({"to": "a@x.com, b@x.com", "subject": "Hi"});
        let b = json!({"to": "B@x.com , a@x.com", "subject": "Hi"});
        assert_eq!(gmail_signature(&a), gmail_signature(&b));
    }

    #[test]
    fn no_addresses_no_signature_so_no_dedupe() {
        assert_eq!(gmail_signature(&json!({"subject": "Hi"})), None);
        assert_eq!(
            gmail_signature(&json!({"to": "not-an-address", "subject": "Hi"})),
            None
        );
    }

    #[test]
    fn address_of_reads_display_names_and_bare_addresses() {
        assert_eq!(
            address_of(" Gareth Lynch <Gareth@X.com> ").as_deref(),
            Some("gareth@x.com")
        );
        assert_eq!(
            address_of("toby@cooperbev.com").as_deref(),
            Some("toby@cooperbev.com")
        );
        assert_eq!(address_of(""), None);
        assert_eq!(address_of("   "), None);
        assert_eq!(address_of("no at sign"), None);
    }

    #[test]
    fn plus_address_rules() {
        assert_eq!(plus_tag("Field Ops!"), "field-ops");
        assert_eq!(plus_tag("--x--"), "x");
        assert_eq!(
            plus_address("jon@x.com", "triage").as_deref(),
            Some("jon+triage@x.com")
        );
        // An already-plus-addressed org email replaces its tag.
        assert_eq!(
            plus_address("jon+old@x.com", "new").as_deref(),
            Some("jon+new@x.com")
        );
        assert_eq!(plus_address("jon@", "x"), None);
        assert_eq!(plus_address("@x.com", "x"), None);
        assert_eq!(plus_address("jon@x.com", "!!!"), None);
    }

    #[test]
    fn agent_from_address_precedence() {
        assert_eq!(
            agent_from_address("triage", Some(" ops@x.com "), Some("jon@x.com")).as_deref(),
            Some("ops@x.com"),
            "the stored override wins, trimmed"
        );
        assert_eq!(
            agent_from_address("triage", None, Some("jon@x.com")).as_deref(),
            Some("jon+triage@x.com")
        );
        assert_eq!(agent_from_address("triage", None, None), None);
        // A blank override is no override.
        assert_eq!(
            agent_from_address("triage", Some("   "), Some("jon@x.com")).as_deref(),
            Some("jon+triage@x.com")
        );
    }

    // Google's token endpoint, 2026-11-12, no real credential (captured in
    // talaria-google-connections' tests) — and a Drive 404 from the
    // outcrop-labs log the same day.
    const TOKEN_BAD_SECRET: &str = "google token refresh failed: 401 Unauthorized {\n  \"error\": \"invalid_client\",\n  \"error_description\": \"The provided client secret is invalid.\"\n}";
    const TOKEN_BAD_REFRESH: &str = "google token refresh failed: 400 Bad Request {\n  \"error\": \"invalid_grant\",\n  \"error_description\": \"Bad Request\"\n}";
    const DRIVE_404: &str = "drive rename failed: 404 Not Found {\n  \"error\": {\n    \"code\": 404,\n    \"message\": \"File not found: talariaCaptureNonexistentDoc0000000000000.\",\n    \"errors\": [\n      {\n        \"message\": \"File not found: talariaCaptureNonexistentDoc0000000000000.\",\n        \"domain\": \"global\",\n        \"reason\": \"notFound\",\n        \"location\": \"fileId\",\n        \"locationType\": \"parameter\"\n      }\n    ]\n  }\n}\n";

    #[test]
    fn a_refused_token_quotes_google_and_names_whose_fix_it_is() {
        let reconnect = "Reconnect the org Google account to run this.";
        assert_eq!(
            token_refusal_message(
                &TokenError::InvalidGrant(TOKEN_BAD_REFRESH.into()),
                reconnect
            )
            .as_deref(),
            Some(
                "Google refused the stored Google connection: «invalid_grant: Bad Request». Reconnect the org Google account to run this."
            )
        );
        let client = token_refusal_message(
            &TokenError::ClientRejected(TOKEN_BAD_SECRET.into()),
            reconnect,
        )
        .expect("quoted");
        assert!(client.starts_with(
            "Google refused this server's OAuth client: «invalid_client: The provided client secret is invalid.»"
        ));
        assert!(
            !client.contains("Reconnect"),
            "a refused client is not fixed by a reconnect"
        );
        // Google said nothing: no sentence, so the caller keeps the hard failure.
        assert_eq!(
            token_refusal_message(
                &TokenError::Other("no google client configured".into()),
                reconnect
            ),
            None
        );
    }

    #[test]
    fn a_failed_action_says_what_google_said_or_what_failed() {
        assert_eq!(
            failure_message(DRIVE_404),
            "Google rejected the action: «notFound: File not found: talariaCaptureNonexistentDoc0000000000000.»"
        );
        assert_eq!(
            failure_message("doc_update missing fileId"),
            "The action did not run: «doc_update missing fileId»"
        );
    }

    #[test]
    fn queued_in_requires_the_same_pending_id() {
        let listed = vec![PendingAction {
            id: "p-1".into(),
            kind: "gmail_send".into(),
            summary: None,
            payload: json!({}),
            agent_model: None,
            owner_user_id: Some("owner".into()),
            is_org: false,
            principal_kind: "owner".into(),
            status: "pending".into(),
            created_ms: 0,
            last_error: None,
        }];
        assert!(queued_in("p-1", &listed));
        assert!(!queued_in("p-2", &listed));
        let decided = PendingAction {
            status: "executed".into(),
            ..listed[0].clone()
        };
        assert!(
            !queued_in("p-1", &[decided]),
            "a decided row is not in the confirm-sends queue"
        );
    }
}
