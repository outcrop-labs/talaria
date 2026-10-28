// POST /api/fleet/seal-configs. Seal the credential literals agent configs
// stored before federation sealed them on the way in: each becomes one of
// that agent's CONFIG_* secrets and every version row keeps a handle in its
// place, rewritten in place. A running agent that gains secrets is rolled
// first, so its container holds them before any config names them, and a
// stopped one's container is removed so its next start creates one that
// does. Body { dryRun?, agentId?, agentIds?, dropUndeliverable? }: dryRun
// defaults to true and only reports; nothing is written until it is false.
// agentIds (or one agentId) limits it to those agents, so an apply does what
// the dry run listed and no more. Admin. Each applied agent audits as its
// outcome is known (talaria-fleet-seal), names and paths only, never values;
// the run itself audits as `fleet.seal_configs`, like a render or reconcile.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::seal::{SealRequest, seal_stored_configs};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    optional_boolean_member, optional_uuid_array_member, optional_uuid_member, parse,
};
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
    // One agent; with agentIds, added to them. Neither: every agent.
    let agent_id = match optional_uuid_member(obj, "agentId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // These agents: an apply sends the ones its dry run listed.
    let agent_ids = match optional_uuid_array_member(obj, "agentIds", 1000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let agent_ids = match (agent_id, agent_ids) {
        (None, None) => None,
        (one, many) => Some(one.into_iter().chain(many.unwrap_or_default()).collect()),
    };
    let actor = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let sb = secretbox_or_500(&state, "[fleet/seal-configs] secretbox failed").await?;
    let req = SealRequest {
        dry_run,
        agent_ids,
        drop_undeliverable,
        actor,
        audit_as: actor_of(&user),
    };
    // Detached, and awaited: an apply rolls agents one after another, which
    // can outlast the caller's connection. Dropped with the request, it would
    // stop between one agent's secrets and its configs, and before the
    // closing render; spawned, it finishes, and each agent's audit row says
    // what happened to it.
    let pg = state.pg.clone();
    let report = match tokio::spawn(async move {
        let report = seal_stored_configs(&pg, &sb, &req).await;
        if let (false, Ok(r)) = (req.dry_run, &report) {
            let count = |status: &str| r.agents.iter().filter(|a| a.status == status).count();
            log_audit(
                &pg,
                AuditEntry {
                    actor: &req.audit_as,
                    action: "fleet.seal_configs",
                    target_type: "fleet",
                    target_id: Some("fleet"),
                    target_label: None,
                    before: None,
                    after: Some(json!({
                        "sealed": count("sealed"),
                        "skipped": count("skipped"),
                        "clean": count("clean"),
                        "dropUndeliverable": req.drop_undeliverable,
                    })),
                },
            )
            .await;
        }
        report
    })
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Ok(internal("[fleet/seal-configs] failed", e)),
        Err(e) => return Ok(internal("[fleet/seal-configs] task failed", e)),
    };
    Ok(Json(json!({ "result": report })).into_response())
}
