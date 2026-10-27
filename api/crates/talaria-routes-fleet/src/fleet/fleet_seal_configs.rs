// POST /api/fleet/seal-configs. Seal the credential literals agent configs
// stored before federation sealed them on the way in: each becomes one of
// that agent's CONFIG_* secrets and every version row keeps a handle in its
// place, rewritten in place. A running agent that gains secrets is rolled
// first, so its container holds them before any config names them. Body
// { dryRun?, agentId?, dropUndeliverable? } — dryRun defaults to true and
// only reports; nothing is written until it is false. Admin. Audits each
// sealed agent — names and paths only, never values.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::seal::{SealRequest, seal_stored_configs};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{optional_boolean_member, optional_uuid_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    // An empty body is the dry run over every agent.
    let parsed = if body.is_empty() {
        json!({})
    } else {
        parse(&body)
    };
    let obj = object_or_400(&parsed)?;
    // Default true: report what sealing would do, write nothing.
    let dry_run = match optional_boolean_member(obj, "dryRun") {
        Ok(v) => v.unwrap_or(true),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Take a literal no secret can carry (multi-line, over 8 KB) out of the config instead of skipping its agent.
    let drop_undeliverable = match optional_boolean_member(obj, "dropUndeliverable") {
        Ok(v) => v.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // One agent; absent, every agent.
    let agent_id = match optional_uuid_member(obj, "agentId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let actor = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let sb = secretbox_or_500(&state, "[fleet/seal-configs] secretbox failed").await?;
    let report = match seal_stored_configs(
        &state.pg,
        &sb,
        &SealRequest {
            dry_run,
            agent_id: agent_id.as_deref(),
            drop_undeliverable,
            actor: &actor,
        },
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[fleet/seal-configs] failed", e)),
    };
    if !dry_run {
        let who = actor_of(&user);
        for agent in report.agents.iter().filter(|a| a.status == "sealed") {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &who,
                    action: "agent.config_sealed",
                    target_type: "agent",
                    target_id: Some(&agent.agent_id),
                    target_label: Some(&agent.display_name),
                    before: None,
                    after: Some(json!({
                        "versions": agent.versions,
                        "sealed": agent.sealed,
                        "dropped": agent.dropped,
                        "undeliverable": agent.undeliverable,
                        "rolled": agent.rolled,
                    })),
                },
            )
            .await;
        }
    }
    Ok(Json(json!({ "result": report })).into_response())
}
