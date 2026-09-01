// /api/fleet/crons — port of ui/src/routes/api/fleet.crons.ts. Fleet-wide
// crons (admin). GET → every managed agent's jobs (down containers reported
// per-agent, not fatal). POST → create the same job across agents, staggered
// per agent when the schedule is a fixed-minute cron expression.

use crate::agent_crons::{create_fleet_crons, list_fleet_crons};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, trimmed_string_member, uuid_array_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    match list_fleet_crons(&state.pg).await {
        Ok(agents) => Json(json!({ "agents": agents })).into_response(),
        Err(_) => thrown_internal_error(),
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // agentIds: z.array(Uuid).min(1).max(64) — the helper carries the max;
    // the min (a fleet cron with no agents is nothing) is checked here.
    let agent_ids = match uuid_array_member(obj, "agentIds", 64) {
        Ok(ids) => ids,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if agent_ids.is_empty() {
        return house_error(
            StatusCode::BAD_REQUEST,
            &crate::body::array_too_small_msg(1),
        );
    }
    let name = match trimmed_string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let schedule = match trimmed_string_member(obj, "schedule", 1, 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let prompt = match trimmed_string_member(obj, "prompt", 1, 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let stagger = match stagger_minutes(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let results =
        match create_fleet_crons(&state.pg, &agent_ids, &name, &schedule, &prompt, stagger).await {
            Ok(r) => r,
            Err(_) => return thrown_internal_error(),
        };
    let actor = actor_of(&user);
    let after = json!({
        "agentIds": agent_ids,
        "schedule": schedule,
    });
    let label = name.clone();
    let pg = state.pg.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "cron.create",
                target_type: "fleet",
                target_id: Some("fleet"),
                target_label: Some(&label),
                before: None,
                after: Some(after),
            },
        )
        .await;
    });
    Json(json!({ "results": results })).into_response()
}

/// `z.number().int().min(0).max(30).optional()` — optional int in [0, 30].
fn stagger_minutes(obj: &serde_json::Map<String, Value>) -> Result<Option<i64>, String> {
    match obj.get("staggerMinutes") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            let f = n.as_f64().unwrap_or(f64::NAN);
            if f.fract() != 0.0 {
                return Err("Invalid input: expected int, received number".into());
            }
            let v = f as i64;
            if !(0..=30).contains(&v) {
                // zod prints the violated bound; both bounds are 0 and 30.
                return Err(crate::body::too_big_msg(30));
            }
            Ok(Some(v))
        }
        Some(v) => Err(format!(
            "Invalid input: expected number, received {}",
            crate::body::zod_type_name(v)
        )),
    }
}
