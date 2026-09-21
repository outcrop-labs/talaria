// POST /api/agents/register. An agent registers with Talaria (MC-compatible
// contract, so the existing plugin works repointed). Agent-key auth.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::check_fleet_key;
use talaria_agents_registry::register_agent;
use talaria_body::{as_object, optional_max_string_member, parse, string_member};
use talaria_error::{house_error, internal};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Fleet-plane: the registration body names the subject, and an agent
    // registers BEFORE it has a credential of its own.
    let ok = match check_fleet_key(&state.pg, &headers).await {
        Ok(ok) => ok,
        Err(e) => return internal("[fleet] check_fleet_key failed", e),
    };
    if !ok {
        return house_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let role = match optional_max_string_member(obj, "role", 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // capabilities: optional array — elements are unconstrained strings.
    let capabilities = match obj.get("capabilities") {
        None => Value::Array(Vec::new()),
        Some(Value::Array(items)) => {
            for item in items {
                if !item.is_string() {
                    return house_error(
                        StatusCode::BAD_REQUEST,
                        &talaria_body::string_msg(talaria_body::zod_type_name(item)),
                    );
                }
            }
            Value::Array(items.clone())
        }
        Some(v) => {
            return house_error(
                StatusCode::BAD_REQUEST,
                &talaria_body::array_msg(talaria_body::zod_type_name(v)),
            );
        }
    };
    let framework = match optional_max_string_member(obj, "framework", 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match register_agent(
        &state.pg,
        &name,
        role.as_deref(),
        framework.as_deref(),
        &capabilities,
    )
    .await
    {
        Ok((id, name)) => Json(json!({
            "agent": { "id": id, "name": name },
            "registered": true,
        }))
        .into_response(),
        Err(e) => internal("[fleet] register_agent failed", e),
    }
}
