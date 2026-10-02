// /api/workbench/coding/accounts/{agentId}. One agent's coding accounts: what
// is signed in, which plan is its default, and which model fills each of the
// harness's roles.
//
//   GET    the accounts, their role picks, the gateway plan's picks, and what
//          a job with no ticket pin would resolve to right now
//   PUT    set the default plan, or replace one plan's role picks
//   DELETE sign an account out
//
// The GATEWAY IS A PLAN HERE TOO. `accountId: null` in a PUT means the org's
// Talaria gateway — so an agent with subscriptions signed in can still be told
// to run its coding work through the gateway, on models the org picked, and
// `defaultPlan: null` is how its default goes back there without signing
// anything out.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_audit::spawn_audit;
use talaria_body::{NumKind, enum_member, nullable_number_member, parse, string_member};
use talaria_coding_accounts as ca;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::actor_of;
use talaria_state::AppState;

use super::coding_gate::{agent_exists, require_agent_editor};

/// The largest integer a JSON body can carry exactly.
const SAFE_INT: f64 = 9_007_199_254_740_991.0;

// doc: One agent's coding accounts with their per-plan role picks, plus what a
// doc: job that pins nothing would run on right now.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Result<Response, Response> {
    require_agent_editor(&state, &headers, &agent_id).await?;
    if !agent_exists(&state, &agent_id).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent"));
    }
    let accounts = ca::list_accounts(&state.pg, &agent_id)
        .await
        .map_err(|e| internal("[coding] account list failed", e))?;
    let gateway_roles = ca::role_picks(&state.pg, &agent_id, ca::Plan::Gateway)
        .await
        .map_err(|e| internal("[coding] gateway role read failed", e))?;
    // What a job would resolve to with no ticket pin — the honest answer to
    // "what does this agent code with", including the fall-down.
    let resolved = ca::resolve(&state.pg, &agent_id, None)
        .await
        .map_err(|e| internal("[coding] resolve failed", e))?;
    Ok(axum::Json(json!({
        "accounts": accounts,
        "gatewayRoles": roles_object(&gateway_roles),
        "roles": ca::ROLES,
        "resolved": resolved.map(|r| json!({
            "accountId": r.plan.account_id(),
            "provider": r.provider,
            "email": r.email,
            "source": r.source,
            "gateway": r.is_gateway(),
            "models": roles_object(&r.roles),
        })),
    }))
    .into_response())
}

fn roles_object(picks: &[(String, String)]) -> Value {
    let mut out = serde_json::Map::new();
    for (role, model) in picks {
        out.insert(role.clone(), json!(model));
    }
    Value::Object(out)
}

// doc: Set the agent's default coding plan, or replace one plan's role picks.
// doc: An `accountId` of null means the org's Talaria gateway in both cases.
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_agent_editor(&state, &headers, &agent_id).await?;
    if !agent_exists(&state, &agent_id).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let action = match enum_member(obj, "action", &["default", "roles"]) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Absent or null accountId is the gateway plan in both actions — which is
    // why this is the nullable reader and not the optional-only one, whose
    // whole job is to refuse an explicit null.
    let account_id = match nullable_number_member(obj, "accountId", NumKind::Int, 1.0, SAFE_INT) {
        Ok(v) => v.map(|n| n as i64),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let plan = ca::Plan::from_account_id(account_id);

    match action.as_str() {
        "default" => {
            let ok = match plan {
                ca::Plan::Gateway => {
                    ca::use_gateway_by_default(&state.pg, &agent_id)
                        .await
                        .map_err(|e| internal("[coding] default write failed", e))?;
                    true
                }
                ca::Plan::Account(id) => ca::set_primary(&state.pg, &agent_id, id)
                    .await
                    .map_err(|e| internal("[coding] default write failed", e))?,
            };
            if !ok {
                return Ok(house_error(StatusCode::NOT_FOUND, "unknown coding account"));
            }
            spawn_audit(
                &state.pg,
                actor_of(&user),
                "agent.coding_default",
                "agent",
                Some(agent_id.clone()),
                Some(json!({ "accountId": account_id })),
            );
        }
        _ => {
            // `roles` is a record of role → model id. An absent role is a role
            // that falls back, so an empty object clears the plan's picks.
            let Some(roles) = obj.get("roles").and_then(Value::as_object) else {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    "roles must be an object of role → model",
                ));
            };
            let mut picks: Vec<(String, String)> = Vec::new();
            for role in ca::ROLES {
                let Some(raw) = roles.get(role) else { continue };
                if raw.is_null() {
                    continue;
                }
                let model = match string_member(roles, role, 1, 200) {
                    Ok(m) => m,
                    Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
                };
                picks.push((role.to_string(), model));
            }
            // A role this product does not have is a typo worth saying out
            // loud rather than silently dropping.
            if let Some(unknown) = roles.keys().find(|k| !ca::ROLES.contains(&k.as_str())) {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    &format!("unknown harness role \"{unknown}\""),
                ));
            }
            let ok = ca::set_role_picks(&state.pg, &agent_id, plan, &picks)
                .await
                .map_err(|e| internal("[coding] role write failed", e))?;
            if !ok {
                return Ok(house_error(StatusCode::NOT_FOUND, "unknown coding account"));
            }
            spawn_audit(
                &state.pg,
                actor_of(&user),
                "agent.coding_roles",
                "agent",
                Some(agent_id.clone()),
                Some(json!({ "accountId": account_id, "roles": roles_object(&picks) })),
            );
        }
    }
    get(State(state), headers, Path(agent_id)).await
}

// doc: Sign one coding account out. Its role picks and any ticket pins naming
// doc: it go with it, and the credential is deleted from this instance.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_agent_editor(&state, &headers, &agent_id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Signing out names an account; there is no "sign out of the gateway".
    let id = match nullable_number_member(obj, "accountId", NumKind::Int, 1.0, SAFE_INT) {
        Ok(Some(n)) => n as i64,
        Ok(None) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "accountId is required",
            ));
        }
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let gone = ca::delete_account(&state.pg, &agent_id, id)
        .await
        .map_err(|e| internal("[coding] sign-out failed", e))?;
    if !gone {
        return Ok(house_error(StatusCode::NOT_FOUND, "unknown coding account"));
    }
    spawn_audit(
        &state.pg,
        actor_of(&user),
        "agent.coding_signout",
        "agent",
        Some(agent_id.clone()),
        Some(json!({ "accountId": id })),
    );
    Ok(axum::Json(json!({ "ok": true })).into_response())
}
