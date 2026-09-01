// /api/skills — port of ui/src/routes/api/skills.ts.
// Skills across the fleet: shared + per-agent, straight from the mounts the
// agents actually read. Any member reads (the library grounds the Studio and
// what agents will be told); each owner carries canEdit for THIS user —
// admins/agents.manage everywhere, explicit user_agent_access grants (or a
// personal assistant) for that agent's own skills.

use crate::agent_skills::list_all_skills;
use crate::error::thrown_internal_error;
use crate::session::require_user;
use crate::skill_access::can_edit_skills;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let owners = match list_all_skills(&state).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("[skills] list failed: {e}");
            return thrown_internal_error();
        }
    };
    // `{...o, canEdit}` — the engine's summary plus this user's write right,
    // appended after the engine's own keys.
    let mut with_edit: Vec<Value> = Vec::with_capacity(owners.len());
    for owner in owners {
        let can_edit = match can_edit_skills(&state.pg, &user.id, &user.role, &owner.owner).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[skills] edit gate failed: {e}");
                return thrown_internal_error();
            }
        };
        let mut entry = serde_json::to_value(&owner).unwrap_or(Value::Null);
        if let Some(map) = entry.as_object_mut() {
            map.insert("canEdit".into(), json!(can_edit));
        }
        with_edit.push(entry);
    }
    Json(json!({ "owners": with_edit })).into_response()
}
