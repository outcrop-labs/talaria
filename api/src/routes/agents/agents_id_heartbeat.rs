// GET /api/agents/{id}/heartbeat — port of
// ui/src/routes/api/agents.$id.heartbeat.ts. Refresh last_seen and return
// the agent's assigned work (tasks assigned to it, across boards).
// MC-compatible.

use crate::agent_auth::fleet_caller;
use crate::agents_registry::heartbeat_agent;
use crate::error::{house_error, thrown_internal_error};
use crate::state::AppState;
use crate::tasks::assigned_work;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    // Fleet-plane: the subject is the :id in the URL. Work items carry
    // ticket titles and descriptions, so a caller we CAN name must be that
    // agent — otherwise agent A enumerates agent B's assignments. A legacy
    // container that sends no x-agent-name is unnameable and still allowed;
    // that ends when the shared key does.
    let caller = match fleet_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return house_error(axum::http::StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(gate) => return gate,
    };

    // AUTHORIZE, THEN WRITE. `heartbeatAgent()` is a write — it stamps
    // last_seen and lifts offline → idle — so resolving the subject through
    // it meant agent A stamped agent B's liveness and was refused only
    // afterwards: the 403 was honest but the side effect had already landed,
    // which is enough to forge another agent's presence (it reads live in
    // the fleet UI and to anything keyed off FRESH_MS). Name the subject
    // with a read first, decide, and only then heartbeat.
    //
    // The cast rides the parameter (`id = $1::uuid`), so a non-uuid :id is
    // a Postgres error here — the same 500 the TS throw produces, not a
    // quiet 404.
    let name: Option<String> =
        match sqlx::query_scalar("select name from fleet_agents where id = $1::uuid")
            .bind(&id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(name) => name,
            Err(_) => return thrown_internal_error(),
        };
    let Some(name) = name else {
        return house_error(axum::http::StatusCode::NOT_FOUND, "unknown agent");
    };
    if !caller.model.is_empty() && caller.model != name {
        return house_error(
            axum::http::StatusCode::FORBIDDEN,
            &format!(
                "this credential belongs to \"{}\", not \"{}\"",
                caller.model, name
            ),
        );
    }

    if heartbeat_agent(&state.pg, &id, None).await.is_err() {
        return thrown_internal_error();
    }
    match assigned_work(&state.pg, &name).await {
        Ok(work_items) => Json(json!({ "work_items": work_items })).into_response(),
        Err(_) => thrown_internal_error(),
    }
}
