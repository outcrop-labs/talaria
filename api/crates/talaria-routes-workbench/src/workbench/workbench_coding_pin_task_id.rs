// /api/workbench/coding/pin/{taskId}. Which plan this ticket's coding work
// runs on.
//
// WHY A TICKET NEEDS ITS OWN ANSWER. A subscription runs out. Somebody is
// halfway through a ticket when the plan behind it hits its weekly cap, and
// the fix they want is "move THIS ticket onto the other account" — not
// "re-point the agent and every other ticket with it". So the pin lives on the
// ticket, outranks the agent's default, and names the model as well as the
// plan: the other account's flagship is not always the one the agent's default
// role would have picked.
//
//   GET    the pin, the plans this ticket's agent could use, and what a job
//          started right now would actually resolve to
//   PUT    pin a plan (`accountId: null` is the org's Talaria gateway) and
//          optionally one model
//   DELETE unpin — back to the agent's default
//
// The ticket's agent is the ASSIGNEE, which is what makes this safe: the plans
// offered are that agent's, so a pin can never reach another agent's
// credential. A ticket with no agent assigned has nothing to pin.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_audit::spawn_audit;
use talaria_body::{NumKind, nullable_number_member, optional_max_string_member, parse};
use talaria_coding_accounts as ca;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::actor_of;
use talaria_state::AppState;

use super::coding_gate::require_feature;

/// The largest integer a JSON body can carry exactly.
const SAFE_INT: f64 = 9_007_199_254_740_991.0;

/// The agent a ticket's coding work would run as, resolved to an
/// `agent_defs.id`. `None` when the ticket is unassigned or assigned only to
/// people — in both cases there is no plan to pin.
///
/// `tasks.assignees` is a jsonb ARRAY of fleet model ids mixed with human
/// ones, not a single column, so the join is through a jsonb containment test
/// against the agent's model rather than an equality on a field that does not
/// exist. A ticket with two agents on it resolves to the first by model, which
/// is the same tie-break the board's own agent column shows.
async fn ticket_agent(state: &AppState, task_id: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "select d.id::text from tasks t \
         join agent_defs d on t.assignees @> to_jsonb(array[d.model]) \
         where t.id = $1::uuid \
         order by d.model limit 1",
    )
    .bind(task_id)
    .fetch_optional(&state.pg)
    .await
}

// doc: This ticket's pinned coding plan, the plans its agent could use, and
// doc: what a job started now would resolve to.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Response, Response> {
    require_feature(&state, &headers).await?;
    let agent_id = ticket_agent(&state, &task_id)
        .await
        .map_err(|e| internal("[coding] ticket agent lookup failed", e))?;
    let Some(agent_id) = agent_id else {
        // Not an error: a ticket with no agent assigned simply has no plan to
        // choose, and the strip renders nothing rather than an empty picker.
        return Ok(axum::Json(json!({
            "pin": Value::Null, "plans": [], "resolved": Value::Null,
        }))
        .into_response());
    };
    let pin = ca::task_pin(&state.pg, &task_id)
        .await
        .map_err(|e| internal("[coding] pin read failed", e))?;
    let accounts = ca::list_accounts(&state.pg, &agent_id)
        .await
        .map_err(|e| internal("[coding] account list failed", e))?;
    let resolved = ca::resolve(&state.pg, &agent_id, Some(&task_id))
        .await
        .map_err(|e| internal("[coding] resolve failed", e))?;
    Ok(axum::Json(json!({
        "pin": pin,
        // The gateway first: it is always available and needs no credential,
        // so it is the one choice that cannot fail.
        "plans": plans(&accounts),
        "resolved": resolved.map(|r| json!({
            "accountId": r.plan.account_id(),
            "provider": r.provider,
            "source": r.source,
            "gateway": r.is_gateway(),
            "models": r.roles.iter().map(|(role, model)| json!({
                "role": role, "model": model,
            })).collect::<Vec<_>>(),
        })),
    }))
    .into_response())
}

/// The pickable plans: the gateway, then every signed-in account the org still
/// permits and that is not disabled. A disabled account is deliberately absent
/// — offering a plan that cannot run is worse than not offering it.
fn plans(accounts: &[ca::Account]) -> Vec<Value> {
    let mut out = vec![json!({
        "accountId": Value::Null,
        "provider": ca::GATEWAY_PROVIDER,
        "label": "Talaria gateway",
        "gateway": true,
    })];
    for a in accounts {
        if !a.permitted || a.disabled_at.is_some() {
            continue;
        }
        out.push(json!({
            "accountId": a.id,
            "provider": a.provider,
            "label": a.email.clone().or_else(|| a.org_name.clone()).unwrap_or_else(|| a.provider.clone()),
            "gateway": false,
            "roles": a.roles,
        }));
    }
    out
}

// doc: Pin this ticket's coding work to one plan and optionally one model.
// doc: `accountId: null` pins it to the org's Talaria gateway.
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_feature(&state, &headers).await?;
    // Pinning is a WORK decision, not an agent-configuration one: anyone who
    // can act on the ticket can say which of the agent's plans this ticket
    // uses. It cannot reach a credential — the plans are the agent's own, the
    // pin only chooses among them, and nothing here reveals a token.
    let agent_id = ticket_agent(&state, &task_id)
        .await
        .map_err(|e| internal("[coding] ticket agent lookup failed", e))?;
    let Some(agent_id) = agent_id else {
        return Ok(house_error(
            StatusCode::CONFLICT,
            "this ticket has no agent assigned, so there is no coding plan to pin",
        ));
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // null is the gateway, not a validation error.
    let account_id = match nullable_number_member(obj, "accountId", NumKind::Int, 1.0, SAFE_INT) {
        Ok(v) => v.map(|n| n as i64),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let plan = ca::Plan::from_account_id(account_id);
    let model = optional_max_string_member(obj, "model", 200)
        .ok()
        .flatten()
        .filter(|m| !m.trim().is_empty());

    let ok = ca::set_task_pin(
        &state.pg,
        &task_id,
        &agent_id,
        plan,
        model.as_deref(),
        Some(&user.id),
    )
    .await
    .map_err(|e| internal("[coding] pin write failed", e))?;
    if !ok {
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            "that coding account is not this ticket's agent's",
        ));
    }
    spawn_audit(
        &state.pg,
        actor_of(&user),
        "task.coding_pin",
        "task",
        Some(task_id.clone()),
        Some(json!({ "accountId": account_id, "model": model })),
    );
    get(State(state), headers, Path(task_id)).await
}

// doc: Unpin — this ticket goes back to the agent's default coding plan.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Response, Response> {
    let user = require_feature(&state, &headers).await?;
    ca::clear_task_pin(&state.pg, &task_id)
        .await
        .map_err(|e| internal("[coding] pin clear failed", e))?;
    spawn_audit(
        &state.pg,
        actor_of(&user),
        "task.coding_unpin",
        "task",
        Some(task_id.clone()),
        Some(json!({})),
    );
    get(State(state), headers, Path(task_id)).await
}
