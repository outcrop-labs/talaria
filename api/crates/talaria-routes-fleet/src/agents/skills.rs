// /api/skills.
// Skills across the fleet: shared + per-agent, straight from the mounts the
// agents actually read. Any member reads (the library grounds the Studio and
// what agents will be told); each owner carries canEdit for THIS user —
// admins/agents.manage everywhere, explicit user_agent_access grants (or a
// personal assistant) for that agent's own skills.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_skills::list_all_skills;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_skill_access::can_edit_skills;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let owners = match list_all_skills(&state).await {
        Ok(o) => o,
        Err(e) => return internal("[skills] list failed", e),
    };
    // each entry is the engine's summary plus this user's write right —
    // canEdit appended after the engine's own keys.
    let mut with_edit: Vec<Value> = Vec::with_capacity(owners.len());
    for owner in owners {
        let can_edit = match can_edit_skills(&state.pg, &user.id, &user.role, &owner.owner).await {
            Ok(v) => v,
            Err(e) => return internal("[skills] edit gate failed", e),
        };
        let mut entry = serde_json::to_value(&owner).unwrap_or(Value::Null);
        if let Some(map) = entry.as_object_mut() {
            map.insert("canEdit".into(), json!(can_edit));
        }
        with_edit.push(entry);
    }
    Json(json!({ "owners": with_edit })).into_response()
}
