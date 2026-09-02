// google/pending-actions — confirm-sends: outbound Google actions an agent
// drafted, held for approval. Reads and drafts are free; anything that leaves
// the building (an email, a calendar invite) waits here until a human approves
// — then it executes.
//
//   personal action (a personal assistant, bound to its owner) → the OWNER approves
//   org action (a general agent, on the shared org account)     → an ADMIN approves

use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;

use crate::approvals::{ApprovalDeps, announce_approval};
use crate::google::calendar::{CreateEventInput, create_event_with_token};
use crate::google::connections::get_access_token;
use crate::google::gmail::{SendInput, send_message_with_token};
use crate::google::org::{get_org_access_token, get_org_email, get_org_targets};
use crate::realtime::RealtimeDeps;
use crate::runs::define::run_definition;
use crate::secretbox::SecretBox;

fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

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
    pub status: String,
    pub created_ms: i64,
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
}

/// An agent-drafted outbound action lands here, pending. `realtime` is the
/// fan-out the announce rides; the announce itself is detached and silent
/// (see the spawn below).
pub async fn queue_action(
    pg: &PgPool,
    realtime: RealtimeDeps,
    input: &QueueAction<'_>,
) -> Result<PendingAction, String> {
    #[allow(clippy::type_complexity)] // the queued row's own columns, one each
    let row: (
        String,
        String,
        Option<String>,
        Value,
        Option<String>,
        Option<String>,
        bool,
        String,
        i64,
    ) = sqlx::query_as(
        "insert into google_pending_actions (kind, summary, payload, agent_model, owner_user_id, is_org) \
         values ($1, $2, $3, $4, $5::uuid, $6) \
         returning id::text, kind, summary, payload, agent_model, owner_user_id::text, is_org, status, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint",
    )
    .bind(input.kind)
    .bind(input.summary)
    .bind(input.payload)
    .bind(input.agent_model)
    .bind(input.owner_user_id)
    .bind(input.is_org)
    .fetch_one(pg)
    .await
    .map_err(|e| format!("google pending action insert: {e}"))?;
    let (id, kind, summary, payload, agent_model, owner_user_id, is_org, status, created_ms) = row;
    let action = PendingAction {
        id: id.clone(),
        kind,
        summary,
        payload,
        agent_model,
        owner_user_id,
        is_org,
        status,
        created_ms,
    };

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
        let deps = ApprovalDeps::new(
            announce_pg,
            realtime,
            Arc::new(run_definition),
            Arc::new(wall_ms),
        );
        announce_approval(&deps, &format!("google_action:{id}")).await;
    });
    Ok(action)
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
        "status": a.status,
        "createdAt": crate::agent_auth::epoch_ms_to_iso(a.created_ms),
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
    #[allow(clippy::type_complexity)] // the listing's own columns, one each
    let rows: Vec<(
        String,
        String,
        Option<String>,
        Value,
        Option<String>,
        Option<String>,
        bool,
        String,
        i64,
    )> = sqlx::query_as(
        "select id::text, kind, summary, payload, agent_model, owner_user_id::text, is_org, status, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from google_pending_actions \
         where status = 'pending' \
           and ((is_org = false and owner_user_id = $1::uuid) or ($2 and is_org = true)) \
         order by created_at desc",
    )
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                kind,
                summary,
                payload,
                agent_model,
                owner_user_id,
                is_org,
                status,
                created_ms,
            )| PendingAction {
                id,
                kind,
                summary,
                payload,
                agent_model,
                owner_user_id,
                is_org,
                status,
                created_ms,
            },
        )
        .collect())
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
    let action: Option<(String, Value, Option<String>, Option<String>, bool, String)> =
        sqlx::query_as(
            "select kind, payload, agent_model, owner_user_id::text, is_org, status \
             from google_pending_actions where id = $1::uuid",
        )
        .bind(action_id)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("google pending action read: {e}"))?;
    let Some((kind, payload, agent_model, owner_user_id, is_org, status)) = action else {
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

    // A terminal decision resolves the approval line on the owner's (and the
    // decider's) brief. Detached and silent: the decider is waiting on this
    // response. Org actions decided by one admin leave other admins' briefs
    // to the scheduled sweep — the nudge is an optimization, never the floor.
    let nudge_brief = |ids: Vec<String>| {
        let pg = pg.clone();
        tokio::spawn(async move {
            let deps = crate::notify::NotifyDeps::publishing(pg, None);
            let _ = crate::notify::mark_brief_stale(&deps, &ids).await;
        });
    };
    let stale_ids: Vec<String> = [owner_user_id.clone(), Some(actor_id.to_string())]
        .into_iter()
        .flatten()
        .collect();

    if decision == "reject" {
        sqlx::query(
            "update google_pending_actions set status = 'rejected', decided_at = now(), decided_by = $2::uuid \
             where id = $1::uuid",
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

    // Approve → resolve the executing token (org account, or the owner's).
    let token = if is_org {
        get_org_access_token(pg, sb, now_ms)
            .await
            .map_err(|e| e.to_string())?
    } else {
        get_access_token(
            pg,
            sb,
            owner_user_id
                .as_deref()
                .expect("personal actions carry an owner"),
            now_ms,
        )
        .await
        .map_err(|e| e.to_string())?
    };
    let Some(token) = token else {
        return Ok(Some(DecideOutcome {
            status: "not_connected".into(),
            message: Some(
                if is_org {
                    "Reconnect the org Google account to run this."
                } else {
                    "Reconnect Google to run this action."
                }
                .to_string(),
            ),
        }));
    };

    // Org actions land on the configured shared targets (calendar / send-as
    // alias) — except an org agent's mail, which carries ITS OWN address: the
    // stored override, else the org account's plus-address for its slug, else
    // the send-as target as before. Resolved at execution (not queueing) so an
    // alias edit between draft and approve is honored.
    let targets = if is_org {
        get_org_targets(pg).await.map_err(|e| e.to_string())?
    } else {
        Default::default()
    };
    let agent_from = if is_org && kind == "gmail_send" && agent_model.is_some() {
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
        };
        create_event_with_token(&token, &input, targets.calendar_id.as_deref())
            .await
            .map(|e| serde_json::to_value(&e).unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    } else {
        Err(format!("unknown action kind: {kind}"))
    };

    match executed {
        Ok(result) => {
            sqlx::query(
                "update google_pending_actions \
                 set status = 'executed', result = $2, decided_at = now(), decided_by = $3::uuid \
                 where id = $1::uuid",
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
                 where id = $1::uuid",
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
                message: Some("Google rejected the action.".into()),
            }))
        }
    }
}

// ── Org agent addressing (the send-path half) ─────────────────────────────────

/// The agent's slug + stored alias override.
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

/// The From address an org agent sends as: its alias override, else its
/// derived plus-address of the org account (get_org_email — plus-addresses
/// hang off the org account, so no connection means no derived addresses; an
/// override still wins). null when neither resolves — the caller falls back
/// to the org sendAs target.
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
}
