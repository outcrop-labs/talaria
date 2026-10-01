// GET /api/fleet/defs. The harness registry: agent definitions (latest
// version inline) + LLM endpoints + brain routability.
//
// Who sees what: `agents.manage` (and admins) read the whole fleet; everyone
// else reads the agents they MANAGE and nothing more, because being named a
// manager is the grant — it should not also need an org-wide permission
// (docs/PERMISSIONS.md, "Agent managers"). Someone who holds neither reads
// nothing, and `canHire` tells the roster which of the two empty states to
// render: "no agents yet" or "not available to you".
//
// A manager who is not fleet-wide gets the endpoints PARED to what the model
// picker needs — id, name, provider, class, context, models, effort ladders.
// The config surface includes infra layout (base urls, key env names, the
// price sheet), and managing one agent is not a reason to be shown the
// instance's wiring.
//
// Each def carries its `managers` and the caller's own `canManage`, so the
// roster and the manage modal gate their affordances on the same answer the
// write routes will give.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use talaria_agent_defs::list_agent_defs_wire;
use talaria_agent_managers::{managed_agent_ids, managers_by_agent, reads_whole_fleet};
use talaria_api_facades::gateway::registry::list_endpoints_wire;
use talaria_brain_health::fleet_brain_health;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

/// The model picker's fields, and only those.
const PICKER_FIELDS: [&str; 7] = [
    "id",
    "name",
    "provider",
    "class",
    "contextLength",
    "models",
    "modelEfforts",
];

fn pare_endpoint(e: &Value) -> Value {
    let mut out = Map::new();
    for key in PICKER_FIELDS {
        if let Some(v) = e.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    Value::Object(out)
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let fleet_wide = match reads_whole_fleet(&state.pg, &user.id, &user.role).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[fleet] permission read failed", e)),
    };
    let mine = match managed_agent_ids(&state.pg, &user.id).await {
        Ok(ids) => ids,
        Err(e) => return Ok(internal("[fleet] managed_agent_ids failed", e)),
    };
    let defs = match list_agent_defs_wire(&state.pg).await {
        Ok(d) => d,
        Err(e) => return Ok(internal("[fleet] list_agent_defs_wire failed", e)),
    };
    let managers = match managers_by_agent(&state.pg).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[fleet] managers_by_agent failed", e)),
    };
    let mut visible_models: std::collections::HashSet<String> = std::collections::HashSet::new();
    let defs: Vec<Value> = defs
        .into_iter()
        .filter_map(|mut d| {
            let id = d
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let can_manage = user.role == "admin" || mine.contains(&id);
            if !fleet_wide && !can_manage {
                return None;
            }
            if let Some(m) = d.get("model").and_then(Value::as_str) {
                visible_models.insert(m.to_string());
            }
            let rows = managers.get(&id).cloned().unwrap_or_default();
            if let Some(obj) = d.as_object_mut() {
                obj.insert(
                    "managers".into(),
                    serde_json::to_value(rows).unwrap_or(Value::Null),
                );
                obj.insert("canManage".into(), Value::Bool(can_manage));
            }
            Some(d)
        })
        .collect();
    let endpoints = match list_endpoints_wire(&state.pg).await {
        Ok(e) => e,
        Err(e) => return Ok(internal("[fleet] list_endpoints_wire failed", e)),
    };
    let endpoints: Vec<Value> = if fleet_wide {
        endpoints
    } else if defs.is_empty() {
        // Nothing to configure, nothing to configure it with. A member who
        // manages no agent has no business reading the instance's model
        // list, pared or not.
        Vec::new()
    } else {
        endpoints.iter().map(pare_endpoint).collect()
    };
    let brains = match fleet_brain_health(&state.pg).await {
        Ok(b) => b,
        Err(e) => return Ok(internal("[fleet] fleet_brain_health failed", e)),
    };
    let brains = if fleet_wide {
        brains
    } else {
        brains
            .into_iter()
            .filter(|b| visible_models.contains(&b.agent))
            .collect()
    };
    // The endpoints' prices ride as numbers — a whole $/MTok price prints the
    // JS way (`3`, never `3.0`).
    let mut body = json!({
        "defs": defs,
        "endpoints": endpoints,
        "brains": brains,
        "canHire": fleet_wide,
    });
    talaria_body::js_numberify(&mut body);
    Ok(Json(body).into_response())
}
