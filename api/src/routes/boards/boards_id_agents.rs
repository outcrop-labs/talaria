// /api/boards/{id}/agents. GET → { allowAll, models }. PUT → set the board's
// agent policy (owner/editor,
// or a personal assistant acting as its owner): either the full { allowAll,
// models } shape, or incremental { add, remove } merged onto the current list
// (the assistant-friendly spelling). Boards are restrictive by default.
// POST/DELETE on `/agents/self` → the one-step grant: the calling personal
// assistant adds or removes only its own row (see that section below).

use crate::agent_auth::{AgentCaller, AgentSubject, refuse_legacy, require_agent};
use crate::boards::{
    BoardAgentConfig, add_board_agent_row, board_info, board_role, can_edit,
    get_board_agent_config, remove_board_agent_row, set_board_agent_config,
};
use crate::body::{as_object, optional_boolean_member, optional_string_array_member, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{acting_user, require_user, unauthorized};
use crate::state::AppState;
use crate::users::assistant_owner_for;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "GET agents", &id) {
        return gate;
    }
    // Any member may read the policy; only owner/editor (or elevated) may
    // write it.
    match board_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on agents failed: {e}");
            return thrown_internal_error();
        }
    }
    match get_board_agent_config(&state.pg, &id).await {
        Ok(cfg) => Json(json!(cfg)).into_response(),
        Err(e) => {
            tracing::error!("[boards] agent config read failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return unauthorized(),
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "PUT agents", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(role) if can_edit(role.as_deref()) || user.elevated => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on agent put failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // PUT's fields in schema order: allowAll, models, add, remove — each an
    // array of model ids, each id ≤200 chars, ≤100 of them.
    let allow_all = match optional_boolean_member(obj, "allowAll") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let models = match optional_string_array_member(obj, "models", 0, 200, 100) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let add = match optional_string_array_member(obj, "add", 0, 200, 100) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let remove = match optional_string_array_member(obj, "remove", 0, 200, 100) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let current: BoardAgentConfig = match get_board_agent_config(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] agent config read failed: {e}");
            return thrown_internal_error();
        }
    };
    // Incremental spelling merges onto the current list: survivors first,
    // additions after, first occurrence wins — insertion order.
    let incremental = add.is_some() || remove.is_some();
    let merged: Vec<String> = if incremental {
        let mut seen = std::collections::HashSet::new();
        let mut list: Vec<String> = Vec::new();
        for m in current
            .models
            .iter()
            .filter(|m| !remove.as_deref().unwrap_or_default().contains(m))
            .chain(add.iter().flatten())
        {
            if seen.insert(m.clone()) {
                list.push(m.clone());
            }
        }
        list
    } else {
        models.unwrap_or_default()
    };
    // An absent allowAll rides the current flag on the incremental arm and
    // the restrictive default on the full-shape arm.
    let allow_all = allow_all.unwrap_or(if incremental {
        current.allow_all
    } else {
        false
    });
    if let Err(e) = set_board_agent_config(&state.pg, &id, allow_all, &merged).await {
        tracing::error!("[boards] agent config write failed: {e}");
        return thrown_internal_error();
    }
    // The answer is the FRESH config — re-read after the write, not echoed
    // from it. The policy write is an audited mutation (it decides which
    // agents may touch the board at all): before = the config read above,
    // after = the fresh one, actor = the ActingUser label so an assistant
    // proxying its owner is recorded as "model (for owner)".
    let fresh = match get_board_agent_config(&state.pg, &id).await {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!("[boards] agent config re-read failed: {e}");
            return thrown_internal_error();
        }
    };
    let label = match board_info(&state.pg, &id).await {
        Ok(info) if info.exists => Some(info.label),
        Ok(_) => None,
        Err(e) => {
            tracing::error!("[boards] board label read on agent put failed: {e}");
            None
        }
    };
    crate::audit::log_audit(
        &state.pg,
        crate::audit::AuditEntry {
            actor: &user.label,
            action: "board.agent_policy",
            target_type: "board",
            target_id: Some(&id),
            target_label: label.as_deref(),
            before: Some(serde_json::json!({
                "allowAll": current.allow_all,
                "models": current.models,
            })),
            after: Some(serde_json::json!(fresh)),
        },
    )
    .await;
    Json(json!(fresh)).into_response()
}

// ── The one-step grant (/agents/self) ─────────────────────────────────────────
//
// The PUT above is the policy's authority and stays editor-gated. Self-service
// is the friction-free spelling for the ONE row a personal assistant may
// manage itself: boards its owner can already read (any role — the owner's
// reach, not the owner's rank). Single-row writes only, never
// `set_board_agent_config`, whose delete-all-then-insert would race a
// concurrent editor's save.

/// The caller behind a self-service verb: a PROVEN personal assistant, its
/// owner's user id, and the audit actor in acting_user's "<model> (for
/// <owner>)" shape so the audit stream cannot tell a proxied write from a
/// session one apart by formatting. Every refusal says what to do instead —
/// a bare 403 here reads as a broken integration, not a migration step
/// (same posture as `refuse_legacy`).
async fn self_service_actor(
    state: &AppState,
    headers: &HeaderMap,
    verb: &str,
) -> Result<(AgentCaller, String, String), Response> {
    let caller = match require_agent(&state.pg, headers).await {
        Ok(c) => c,
        Err(gate) => return Err(gate),
    };
    if let Some(gate) = refuse_legacy(&caller, verb) {
        return Err(gate);
    }
    let owner = match assistant_owner_for(&state.pg, &AgentSubject::Caller(caller.clone())).await {
        Ok(Some(owner)) => owner,
        Ok(None) => {
            return Err(house_error(
                StatusCode::FORBIDDEN,
                &format!(
                    "agent \"{}\" is not a personal assistant — self-service is a personal \
                     assistant's reach. A board editor can add it to the allow-list instead.",
                    caller.model
                ),
            ))
        }
        Err(e) => {
            tracing::error!("[boards] owner read on agents/self failed: {e}");
            return Err(thrown_internal_error());
        }
    };
    // Best-effort label for the audit actor only — the owner id above is the
    // authority; this is how the audit line reads.
    let for_label: Option<(String,)> = sqlx::query_as(
        "select coalesce(email, name, id::text) from users where id = $1::uuid",
    )
    .bind(&owner)
    .fetch_optional(&state.pg)
    .await
    .map_err(|e| {
        tracing::error!("[boards] owner label read on agents/self failed: {e}");
        thrown_internal_error()
    })?;
    let actor = format!(
        "{} (for {})",
        caller.model,
        for_label.map(|(l,)| l).unwrap_or(owner.clone())
    );
    Ok((caller, owner, actor))
}

/// Shared tail of both self verbs: the audited single-row write and the
/// fresh-config answer. `before` is read by the caller (POST needs it before
/// its gate anyway); the audit row is the same shape as the editor PUT's so
/// the stream stays one action per policy mutation, actor telling them apart.
async fn self_write(
    state: &AppState,
    id: &str,
    actor: &str,
    before: BoardAgentConfig,
    write: impl std::future::Future<Output = Result<(), sqlx::Error>>,
) -> Response {
    if let Err(e) = write.await {
        tracing::error!("[boards] agents/self write failed: {e}");
        return thrown_internal_error();
    }
    let fresh = match get_board_agent_config(&state.pg, id).await {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!("[boards] agent config re-read failed: {e}");
            return thrown_internal_error();
        }
    };
    let label = match board_info(&state.pg, id).await {
        Ok(info) if info.exists => Some(info.label),
        Ok(_) => None,
        Err(e) => {
            tracing::error!("[boards] board label read on agents/self failed: {e}");
            None
        }
    };
    crate::audit::log_audit(
        &state.pg,
        crate::audit::AuditEntry {
            actor,
            action: "board.agent_policy",
            target_type: "board",
            target_id: Some(id),
            target_label: label.as_deref(),
            before: Some(serde_json::json!({
                "allowAll": before.allow_all,
                "models": before.models,
            })),
            after: Some(serde_json::json!(fresh)),
        },
    )
    .await;
    Json(json!(fresh)).into_response()
}

/// POST /api/boards/{id}/agents/self — the assistant appends itself. The gate
/// is the owner's READ reach (any board role), not edit rank: reading a board
/// is exactly what the inherited-permissions promise covers, and the editor
/// PUT remains the only way to touch anybody else's row.
pub async fn post_self(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let (caller, owner, actor) = match self_service_actor(&state, &headers, "Joining a board").await
    {
        Ok(v) => v,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "POST agents self", &id) {
        return gate;
    }
    match board_info(&state.pg, &id).await {
        Ok(info) if info.exists => {}
        Ok(_) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[boards] board read on agents/self failed: {e}");
            return thrown_internal_error();
        }
    }
    match board_role(&state.pg, &owner, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return house_error(
                StatusCode::FORBIDDEN,
                &format!(
                    "agent \"{}\" cannot self-serve on this board — its owner cannot read it. \
                     File a request instead: POST /api/boards/{}/agent-requests with \
                     agentModel \"{}\"",
                    caller.model, id, caller.model
                ),
            )
        }
        Err(e) => {
            tracing::error!("[boards] owner role read on agents/self failed: {e}");
            return thrown_internal_error();
        }
    }
    let before = match get_board_agent_config(&state.pg, &id).await {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!("[boards] agent config read failed: {e}");
            return thrown_internal_error();
        }
    };
    let pg = state.pg.clone();
    self_write(
        &state,
        &id,
        &actor,
        before,
        add_board_agent_row(&pg, &id, &caller.model),
    )
    .await
}

/// DELETE /api/boards/{id}/agents/self — the assistant removes only its own
/// row. No owner-can-read gate here, deliberately: removal only ever narrows
/// the caller's own access (an editor granted via request-approve on a board
/// the owner cannot see must still be able to walk away), and editors keep
/// revocation through the PUT either way.
pub async fn delete_self(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let (caller, _owner, actor) =
        match self_service_actor(&state, &headers, "Leaving a board").await {
            Ok(v) => v,
            Err(gate) => return gate,
        };
    if let Some(gate) = crate::params::uuid_gate("boards", "DELETE agents self", &id) {
        return gate;
    }
    match board_info(&state.pg, &id).await {
        Ok(info) if info.exists => {}
        Ok(_) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[boards] board read on agents/self failed: {e}");
            return thrown_internal_error();
        }
    }
    let before = match get_board_agent_config(&state.pg, &id).await {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!("[boards] agent config read failed: {e}");
            return thrown_internal_error();
        }
    };
    let pg = state.pg.clone();
    self_write(
        &state,
        &id,
        &actor,
        before,
        remove_board_agent_row(&pg, &id, &caller.model),
    )
    .await
}
