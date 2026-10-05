// /api/fleet/defs/{id}/managers. Who owns this agent. GET → the roster, for
// anyone who can see the agent. PUT { userIds } → replace it: this agent's
// managers and admins, because handing an agent over is itself a change to
// the agent (docs/PERMISSIONS.md, "Agent managers").
//
// THE SET IS NEVER EMPTY. A manager who cleared the roster would lock
// themselves out of their own agent with nothing to click to get back in —
// only an admin could restore it. An empty PUT is a 400 that says so;
// handing the agent to someone else means naming them in the same request
// that drops yourself.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_managers::{existing_user_ids, list_managers, set_managers};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, string_array_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_agent_manager, require_agent_reader};
use talaria_state::AppState;

async fn agent_label(pg: &sqlx::PgPool, id: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("select display_name from agent_defs where id = $1::uuid")
        .bind(id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten()
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    // Reading the roster is the same reach as reading the agent: whoever
    // sees the def on /api/fleet/defs already sees its managers there.
    require_agent_reader(&state, &headers, &id).await?;
    if agent_label(&state.pg, &id).await.is_none() {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    Ok(match list_managers(&state.pg, &id).await {
        Ok(managers) => Json(json!({ "managers": managers })).into_response(),
        Err(e) => internal("[fleet/managers] list failed", e),
    })
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Required array of uuids, at least one, at most fifty — the body
    // validates before the agent lookup, as everywhere else in this family.
    let user_ids = match string_array_member(obj, "userIds", 36, 36, 1, 50) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mut user_ids: Vec<String> = user_ids;
    user_ids.sort();
    user_ids.dedup();
    let Some(label) = agent_label(&state.pg, &id).await else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    // A uuid that is not a person is a typo, not a silent no-op: the whole
    // PUT is refused rather than writing the rows that happened to resolve.
    let known = match existing_user_ids(&state.pg, &user_ids).await {
        Ok(k) => k,
        Err(e) => return Ok(internal("[fleet/managers] user lookup failed", e)),
    };
    if let Some(missing) = user_ids.iter().find(|u| !known.contains(*u)) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &format!("no such user: {missing}"),
        ));
    }
    let before = list_managers(&state.pg, &id).await.unwrap_or_default();
    if let Err(e) = set_managers(&state.pg, &id, &user_ids, Some(&user.id)).await {
        return Ok(internal("[fleet/managers] write failed", e));
    }
    let after = match list_managers(&state.pg, &id).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[fleet/managers] list failed", e)),
    };
    // Who may change an agent is governance — the audit carries both rosters.
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "agent.managers_set",
            target_type: "agent",
            target_id: Some(&id),
            target_label: Some(&label),
            before: Some(json!({
                "managers": before.iter().map(|m| m.user_id.clone()).collect::<Vec<_>>()
            })),
            after: Some(json!({
                "managers": after.iter().map(|m| m.user_id.clone()).collect::<Vec<_>>()
            })),
        },
    )
    .await;
    Ok(Json(json!({ "managers": after })).into_response())
}
