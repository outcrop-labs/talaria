// /api/boards/{id}/agents. GET → { allowAll, models }. PUT → set the board's
// agent policy (owner/editor,
// or a personal assistant acting as its owner): either the full { allowAll,
// models } shape, or incremental { add, remove } merged onto the current list
// (the assistant-friendly spelling). Boards are restrictive by default.

use crate::boards::{
    BoardAgentConfig, board_info, board_role, can_edit, get_board_agent_config,
    set_board_agent_config,
};
use crate::body::{as_object, optional_boolean_member, optional_string_array_member, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{acting_user, require_user, unauthorized};
use crate::state::AppState;
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
