// POST /api/agents/register. An agent registers with Talaria (MC-compatible
// contract, so the existing plugin works repointed). Agent-key auth: the
// registry row is bound to the caller's PROVEN identity — a `tak_` caller
// registers only its own model, a legacy caller only the name it asserted
// (and `agent_caller` already refuses a personal or elevated name there).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::fleet_caller;
use talaria_agents_registry::register_agent;
use talaria_body::{optional_max_string_member, parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    // Fleet-plane: the registration body names the subject, but the
    // credential decides WHO may write that row. "Holds any fleet key"
    // used to be the whole gate, and the upsert is by name — so one agent
    // (or any legacy-key holder) could rewrite every other agent's role,
    // framework, capabilities and liveness. The caller is resolved the way
    // every other agent door resolves it; an unnamed legacy caller gets no
    // identity and therefore no row.
    let caller = match fleet_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(house_error(StatusCode::UNAUTHORIZED, "unauthorized")),
        Err(gate) => return Err(gate),
    };
    if caller.model.is_empty() {
        return Ok(house_error(StatusCode::FORBIDDEN, "x-agent-name required"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match string_member(obj, "name", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if name != caller.model {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            &format!(
                "this credential belongs to \"{}\", not \"{name}\"",
                caller.model
            ),
        ));
    }
    let role = match optional_max_string_member(obj, "role", 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // capabilities: optional array — elements are unconstrained strings.
    let capabilities = match obj.get("capabilities") {
        None => Value::Array(Vec::new()),
        Some(Value::Array(items)) => {
            for item in items {
                if !item.is_string() {
                    return Ok(house_error(
                        StatusCode::BAD_REQUEST,
                        &talaria_body::string_msg(talaria_body::zod_type_name(item)),
                    ));
                }
            }
            Value::Array(items.clone())
        }
        Some(v) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &talaria_body::array_msg(talaria_body::zod_type_name(v)),
            ));
        }
    };
    let framework = match optional_max_string_member(obj, "framework", 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
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
        },
    )
}
