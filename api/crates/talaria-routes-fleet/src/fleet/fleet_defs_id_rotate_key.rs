// POST /api/fleet/defs/{id}/rotate-key. Revoke an agent's `tak_` credential
// and mint a fresh one — the answer to a leaked key that is not "retire the
// agent". This agent's managers (and admins). The rotation is immediate at
// the door (`agent_keys` is keyed by agent, so the old secret stops
// authenticating the moment the row is replaced); the running container
// still holds the old one until the roll this route queues recreates it,
// which is the window the response's `rolling` names. A stopped agent is not
// started for it — no roll starts a stopped agent — and takes the new key
// when it next starts (its service moved, so that start recreates it).
// `rolling` is false then. The secret is never echoed: the render stamps it
// into fleet/.env and the roll carries it in.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::rotate_agent_api_key;
use talaria_agent_defs::agent_def_by_id;
use talaria_api_facades::fleet::docker::{Observed, observe};
use talaria_api_facades::fleet::render::render_fleet;
use talaria_api_facades::mcp::apply::roll_agent_for_model;
use talaria_audit::{AuditEntry, log_audit};
use talaria_error::{house_error, internal};
use talaria_session::{actor_of, require_agent_manager, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    let def = match agent_def_by_id(&state.pg, &id).await {
        Ok(Some(d)) => d,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[fleet/defs/rotate-key] def read failed", e)),
    };
    let sb = secretbox_or_500(&state, "[fleet/defs/rotate-key] secretbox unavailable").await?;
    // The rotation itself: the old secret is dead from here on, whatever
    // happens to the render and roll below.
    if let Err(e) = rotate_agent_api_key(&state.pg, &sb, &def.id).await {
        return Ok(internal("[fleet/defs/rotate-key] rotate failed", e));
    }
    // Credentials audit by NAME only — never the value.
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "agent.key_rotate",
            target_type: "agent",
            target_id: Some(&def.id),
            target_label: Some(&def.display_name),
            before: None,
            after: Some(json!({ "key": talaria_fleet_layout_var(&def.slug) })),
        },
    )
    .await;
    // The render stamps the new secret into fleet/.env (the DB is the
    // source of truth and the line is rewritten, never skipped). A render
    // that fails leaves the container on a dead key with no way back but
    // another render, so that IS the caller's problem; the roll is not —
    // it is detached, and its verdict goes to the audit trail like every
    // other roll's.
    if let Err(e) = render_fleet(&state.pg, &sb).await {
        return Ok(internal("[fleet/defs/rotate-key] render failed", e));
    }
    // `roll_agent_for_model` itself rolls only a managed+enabled agent that
    // is running; a retired or stopped one gets its new key on the render
    // and picks it up when it next starts.
    let rolling = def.managed
        && matches!(
            observe(&state.pg, &def.department).await,
            Observed::Running { .. }
        );
    if def.managed {
        let (pg, model) = (state.pg.clone(), def.model.clone());
        tokio::spawn(async move {
            roll_agent_for_model(&pg, &sb, &model).await;
        });
    }
    Ok(Json(json!({ "ok": true, "rolling": rolling })).into_response())
}

/// The env variable the rotated secret lands under — the audit row's
/// `after` names it so an operator reading the trail knows which line in
/// fleet/.env changed, without the trail ever holding the secret.
fn talaria_fleet_layout_var(slug: &str) -> String {
    format!("TALARIA_AGENT_KEY_{}", slug.to_uppercase())
}
