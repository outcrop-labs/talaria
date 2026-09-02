// /api/teams/{id} — port of ui/src/routes/api/teams.$id.ts. PATCH { name } →
// rename (owner); DELETE → delete (owner) — the member rows cascade and its
// boards survive as personal boards (team_id set null, not cascaded), which
// is why both are owner-gated. A non-uuid {id} hits teamRole's raw SQL bind:
// TS answers the platform's plain-text 500, this port the house envelope
// (RUST-MIGRATION.md, divergences). Gate order is TS's: uuid bind, then the
// owner check, then the body — a non-owner with a bad body gets the 403.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::teams::{delete_team, rename_team, team_role};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The owner gate: Err(gate) is the response to return. Some(gate) shape
/// (not Result<(), Response>) keeps clippy's large-Err lint quiet.
fn uuid_gate(id: &str, action: &str) -> Option<Response> {
    crate::params::uuid_gate("teams", action, id)
}

async fn owner_gate(
    state: &AppState,
    user_id: &str,
    team_id: &str,
    action: &str,
) -> Option<Response> {
    match team_role(&state.pg, user_id, team_id).await {
        Ok(Some(role)) if role == "owner" => None,
        Ok(_) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[teams] role read on {action} failed: {e}");
            Some(thrown_internal_error())
        }
    }
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate(&id, "PATCH") {
        return gate;
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "PATCH").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = rename_team(&state.pg, &id, &name).await {
        tracing::error!("[teams] rename failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.rename",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(json!({ "name": name })),
        },
    )
    .await;
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate(&id, "DELETE") {
        return gate;
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE").await {
        return gate;
    }
    if let Err(e) = delete_team(&state.pg, &id).await {
        tracing::error!("[teams] delete failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.delete",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Json(json!({ "ok": true })).into_response()
}
