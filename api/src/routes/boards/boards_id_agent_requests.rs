// /api/boards/{id}/agent-requests. The request half of board access for a
// personal assistant whose owner CANNOT read the board — self-service
// (`POST …/agents/self`) is deliberately not available to it, so this is the
// path around that hole.
//
//   GET  → the open queue (owner/editor, or an elevated assistant).
//   POST → file one. Dual: the assistant itself (proven personal), or its
//          owner in a session, filing for their OWN agent. A duplicate open
//          request is swallowed by the partial unique index and answered
//          {ok:true, alreadyPending:true} — the report_gap posture, not an
//          error the caller has to catch.
//   PUT  → decide one ({agentModel, action: approve|reject}). Same gate as
//          GET. Approve grants the row and closes the request in ONE
//          transaction — the grant and the state change commit together.
//
// The file raises a `board_access` approval (approvals.rs — the sixth kind),
// which is the only thing that tells the board's editors; nothing here writes
// its own notification for the REQUEST. The DECISION notifies the requester's
// owner, who cannot see the board and would otherwise never learn the outcome.

use std::sync::Arc;

use crate::agent_auth::{AgentSubject, agent_caller, epoch_ms_to_iso, refuse_legacy};
use crate::approvals::{ApprovalDeps, announce_approval};
use crate::audit::{AuditEntry, log_audit};
use crate::boards::{add_board_agent_row, board_info, board_role, can_edit};
use crate::body::{as_object, enum_member, optional_max_string_member, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::notify::{NotificationInput, NotifyDeps, add_notification};
use crate::realtime::RealtimeDeps;
use crate::runs::define::run_definition;
use crate::session::{acting_user, require_user, unauthorized};
use crate::state::AppState;
use crate::users::assistant_owner_for;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// THE ONLY spelling of the announce key outside approvals.rs — the census
/// builds the same one, and a second derivation would announce one request
/// twice.
fn announce_key(board_id: &str, agent_model: &str) -> String {
    format!("board_access:{board_id}:{agent_model}")
}

/// Detached announce of a freshly filed request — same shape as
/// google/pending_actions: the caller may be servicing an agent's tool call,
/// the sweep is the floor either way, and announce_approval is idempotent
/// against it by mark.
fn announce_raised(pg: sqlx::PgPool, realtime: RealtimeDeps, key: String) {
    tokio::spawn(async move {
        let deps = ApprovalDeps::new(pg, realtime, Arc::new(run_definition), Arc::new(wall_ms));
        announce_approval(&deps, &key).await;
    });
}

/// One open request as the select hands it back, in column order — display
/// name, requester id and requester label are nullable (the def or the user
/// may have been deleted), the epoch-ms tail is created_at.
type QueueRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);

fn queue_wire(r: &QueueRow) -> serde_json::Value {
    json!({
        "id": r.0,
        "agentModel": r.1,
        "agentName": r.2,
        "requestedBy": r.3,
        "requestedByName": r.4,
        "reason": r.5,
        "status": "open",
        "createdAt": epoch_ms_to_iso(r.6),
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return unauthorized(),
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "GET agent requests", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(role) if can_edit(role.as_deref()) || user.elevated => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on agent requests failed: {e}");
            return thrown_internal_error();
        }
    }
    let rows: Vec<QueueRow> = match sqlx::query_as(
        "select r.id::text, r.agent_model, d.display_name, r.requested_by_user_id::text, \
                coalesce(u.name, u.email), r.reason, \
                (trunc(extract(epoch from r.created_at) * 1000))::bigint \
         from board_agent_requests r \
         left join agent_defs d on d.model = r.agent_model \
         left join users u on u.id = r.requested_by_user_id \
         where r.board_id = $1::uuid and r.status = 'open' \
         order by r.created_at",
    )
    .bind(&id)
    .fetch_all(&state.pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[boards] agent request queue read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "requests": rows.iter().map(queue_wire).collect::<Vec<_>>() })).into_response()
}

/// The insert behind both file branches — the partial unique index
/// (`one_open` on board+agent where status='open') is the dedup, and its
/// rows_affected is the caller's whole answer.
async fn insert_request(
    pg: &sqlx::PgPool,
    board_id: &str,
    agent_model: &str,
    requested_by: &str,
    reason: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "insert into board_agent_requests \
            (board_id, agent_model, requested_by_user_id, reason) \
         values ($1::uuid, $2, $3::uuid, $4) on conflict do nothing",
    )
    .bind(board_id)
    .bind(agent_model)
    .bind(requested_by)
    .bind(reason)
    .execute(pg)
    .await?;
    Ok(result.rows_affected())
}

/// Shared tail of both file branches: announce (fresh files only — the first
/// one announced it, and the sweep is the floor), audit, answer.
async fn filed(
    state: &AppState,
    id: &str,
    agent_model: &str,
    actor: &str,
    reason: Option<&str>,
    inserted: u64,
) -> Response {
    if inserted > 0 {
        announce_raised(
            state.pg.clone(),
            RealtimeDeps::publish_only(state.redis().await.ok()),
            announce_key(id, agent_model),
        );
        let label = board_info(&state.pg, id).await.ok().map(|i| i.label);
        log_audit(
            &state.pg,
            AuditEntry {
                actor,
                action: "board.agent_access_requested",
                target_type: "board",
                target_id: Some(id),
                target_label: label.as_deref(),
                before: None,
                after: Some(serde_json::json!({
                    "agentModel": agent_model,
                    "reason": reason,
                })),
            },
        )
        .await;
    }
    Json(json!({ "ok": true, "alreadyPending": inserted == 0 })).into_response()
}

/// The audit actor for the agent branch — acting_user's "<model> (for
/// <owner>)" shape, so the audit stream cannot tell a proxied file from a
/// session one apart by formatting. Best-effort label; the owner id is the
/// authority.
async fn proxied_actor(pg: &sqlx::PgPool, model: &str, owner: &str) -> String {
    let label: Option<(String,)> =
        sqlx::query_as("select coalesce(email, name, id::text) from users where id = $1::uuid")
            .bind(owner)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
    format!("{} (for {})", model, label.map(|(l,)| l).unwrap_or(owner.into()))
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    if let Some(gate) = crate::params::uuid_gate("boards", "POST agent requests", &id) {
        return gate;
    }
    // Both branches read the same optional body members; an empty body is a
    // valid file (the reason is optional). The parsed value lives out here so
    // the borrowed object outlives the branch that read it.
    let parsed = if body.is_empty() {
        None
    } else {
        Some(parse(&body))
    };
    let (obj, reason) = match &parsed {
        None => (None, None),
        Some(value) => {
            let obj = match as_object(value) {
                Ok(o) => o,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let reason = match optional_max_string_member(obj, "reason", 500) {
                Ok(v) => v,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            (Some(obj), reason)
        }
    };
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    // The requester — the agent's owner on both branches, proven differently.
    let (agent_model, requested_by, actor) = match caller {
        Some(caller) => {
            if let Some(gate) = refuse_legacy(&caller, "Requesting board access") {
                return gate;
            }
            // An agent files for ITSELF; a body naming another model is a
            // spoof attempt, not a convenience.
            if let Some(obj) = obj {
                if let Ok(Some(model)) = optional_max_string_member(obj, "agentModel", 200) {
                    if model != caller.model {
                        return house_error(
                            StatusCode::FORBIDDEN,
                            &format!(
                                "an agent may only request access for itself (\"{}\")",
                                caller.model
                            ),
                        );
                    }
                }
            }
            let owner = match assistant_owner_for(
                &state.pg,
                &AgentSubject::Caller(caller.clone()),
            )
            .await
            {
                Ok(Some(owner)) => owner,
                Ok(None) => {
                    return house_error(
                        StatusCode::FORBIDDEN,
                        &format!(
                            "agent \"{}\" is not a personal assistant — its access is the board \
                             policy's to set, not a request's",
                            caller.model
                        ),
                    )
                }
                Err(e) => {
                    tracing::error!("[boards] owner read on agent request failed: {e}");
                    return thrown_internal_error();
                }
            };
            let actor = proxied_actor(&state.pg, &caller.model, &owner).await;
            (caller.model, owner, actor)
        }
        None => {
            let user = match require_user(&state, &headers).await {
                Ok(u) => u,
                Err(gate) => return gate,
            };
            let Some(obj) = obj else {
                return house_error(StatusCode::BAD_REQUEST, "body must be an object");
            };
            let agent_model = match string_member(obj, "agentModel", 1, 200) {
                Ok(v) => v,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            // Only the owner may file for their own assistant — anything
            // else is somebody else's access being requested in their name.
            let owner = match sqlx::query_as::<_, (Option<String>,)>(
                "select owner_user_id::text from agent_defs where model = $1",
            )
            .bind(&agent_model)
            .fetch_optional(&state.pg)
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("[boards] agent def read on agent request failed: {e}");
                    return thrown_internal_error();
                }
            };
            match owner {
                Some((Some(owner_id,),)) if owner_id == user.id => {}
                Some(_) => {
                    return house_error(
                        StatusCode::FORBIDDEN,
                        "only the assistant's owner may request access for it",
                    )
                }
                _ => return house_error(StatusCode::BAD_REQUEST, "unknown agentModel"),
            }
            let label = user
                .email
                .clone()
                .or_else(|| user.name.clone())
                .unwrap_or_else(|| "user".into());
            (agent_model, user.id.clone(), label)
        }
    };
    match board_info(&state.pg, &id).await {
        Ok(info) if info.exists => {}
        Ok(_) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[boards] board read on agent request failed: {e}");
            return thrown_internal_error();
        }
    }
    // The whole point of the request path: the owner CANNOT read this board.
    // If they can, the one-step grant is available and this queue is not the
    // tool — refuse with the route that is.
    match board_role(&state.pg, &requested_by, &id).await {
        Ok(None) => {}
        Ok(Some(_)) => {
            return house_error(
                StatusCode::FORBIDDEN,
                &format!(
                    "this board's owner can already read it — self-serve instead: \
                     POST /api/boards/{}/agents/self (agentModel \"{}\")",
                    id, agent_model
                ),
            )
        }
        Err(e) => {
            tracing::error!("[boards] owner role read on agent request failed: {e}");
            return thrown_internal_error();
        }
    }
    let inserted = match insert_request(&state.pg, &id, &agent_model, &requested_by, reason.as_deref())
        .await
    {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("[boards] agent request insert failed: {e}");
            return thrown_internal_error();
        }
    };
    filed(&state, &id, &agent_model, &actor, reason.as_deref(), inserted).await
}

/// The request a decide acts on: its id, a display name for the outcome
/// message, and the user to tell — in column order, requester nullable.
type OpenRequestRow = (String, String, Option<String>);

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return unauthorized(),
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "PUT agent requests", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(role) if can_edit(role.as_deref()) || user.elevated => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on agent request decide failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let agent_model = match string_member(obj, "agentModel", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let action = match enum_member(obj, "action", &["approve", "reject"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let req: Option<OpenRequestRow> = match sqlx::query_as(
        "select r.id::text, coalesce(d.display_name, r.agent_model), \
                r.requested_by_user_id::text \
         from board_agent_requests r \
         left join agent_defs d on d.model = r.agent_model \
         where r.board_id = $1::uuid and r.agent_model = $2 and r.status = 'open'",
    )
    .bind(&id)
    .bind(&agent_model)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[boards] agent request read on decide failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some((req_id, agent_name, requested_by)) = req else {
        return house_error(StatusCode::NOT_FOUND, "not found or already decided");
    };
    let approve = action == "approve";
    // ONE transaction: the grant and the close commit together, so an approve
    // can never leave a granted agent with a still-open request (re-filed and
    // approved twice) or a closed request with no grant behind it.
    let mut tx = match state.pg.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("[boards] agent request decide tx begin failed: {e}");
            return thrown_internal_error();
        }
    };
    let decided = async {
        if approve {
            add_board_agent_row(&mut *tx, &id, &agent_model).await?;
        }
        let closed = sqlx::query(
            "update board_agent_requests \
             set status = $1, decided_by = $2::uuid, decided_at = now() \
             where id = $3::uuid and status = 'open'",
        )
        .bind(if approve { "approved" } else { "declined" })
        .bind(&user.id)
        .bind(&req_id)
        .execute(&mut *tx)
        .await?;
        Ok::<u64, sqlx::Error>(closed.rows_affected())
    }
    .await;
    match decided {
        Ok(1) => {}
        Ok(_) => {
            // Another editor decided between the read and the close — their
            // decision stands, whole transaction rolled back.
            let _ = tx.rollback().await;
            return house_error(StatusCode::NOT_FOUND, "not found or already decided");
        }
        Err(e) => {
            tracing::error!("[boards] agent request decide write failed: {e}");
            let _ = tx.rollback().await;
            return thrown_internal_error();
        }
    }
    if let Err(e) = tx.commit().await {
        tracing::error!("[boards] agent request decide commit failed: {e}");
        return thrown_internal_error();
    }
    let label = board_info(&state.pg, &id).await.ok().map(|i| i.label);
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &user.label,
            action: "board.agent_access_decision",
            target_type: "board",
            target_id: Some(&id),
            target_label: label.as_deref(),
            before: Some(serde_json::json!({ "agentModel": agent_model, "status": "open" })),
            after: Some(serde_json::json!({
                "agentModel": agent_model,
                "status": if approve { "approved" } else { "declined" },
            })),
        },
    )
    .await;
    // The outcome reaches the requester's owner — who cannot see this board
    // and would otherwise never learn how the request ended. Quiet by design:
    // one in-app notification, no email; the approve already announced
    // nothing (the request's announcement was the editors' to receive).
    if let Some(requested_by) = requested_by.as_deref() {
        let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
        let (title, body) = if approve {
            (
                format!("Board access approved for {agent_name}"),
                format!(
                    "The editors of board {label} approved {agent_name}'s access request — it \
                     can now read and draft on that board.",
                    label = label.clone().unwrap_or_else(|| id.clone())
                ),
            )
        } else {
            (
                format!("Board access declined for {agent_name}"),
                format!(
                    "The editors of board {label} declined {agent_name}'s access request.",
                    label = label.clone().unwrap_or_else(|| id.clone())
                ),
            )
        };
        if let Err(e) = add_notification(
            &notify,
            requested_by,
            &NotificationInput {
                kind: "board_access",
                title: &title,
                body: Some(&body),
                // No href: the recipient cannot open this board — the
                // notification is the outcome, not a door.
                href: None,
            },
        )
        .await
        {
            tracing::error!("[boards] agent request outcome notify failed: {e}");
        }
    }
    Json(json!({ "ok": true, "agentModel": agent_model, "status": if approve { "approved" } else { "declined" } }))
        .into_response()
}
