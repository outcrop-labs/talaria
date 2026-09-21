// POST /api/fleet/render. Render every managed agent's config + the fleet
// compose + the gateway manifest (the bridge hot-reloads the manifest).
// Admin.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::render::render_fleet;
use talaria_audit::{AuditEntry, log_audit};
use talaria_error::house_error;
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let sb = secretbox_or_500(&state, "[fleet] secretbox failed").await?;
    Ok(match render_fleet(&state.pg, &sb, None).await {
        Ok(result) => {
            let actor = actor_of(&user);
            let pg = state.pg.clone();
            tokio::spawn(async move {
                log_audit(
                    &pg,
                    AuditEntry {
                        actor: &actor,
                        action: "fleet.render",
                        target_type: "fleet",
                        target_id: Some("fleet"),
                        target_label: None,
                        before: None,
                        after: None,
                    },
                )
                .await;
            });
            // json({ result }) — wrapped, keys in order: agents, files, warnings.
            Json(json!({
                "result": {
                    "agents": result.agents,
                    "files": result.files,
                    "warnings": result.warnings,
                }
            }))
            .into_response()
        }
        // a house-shaped 500 sentence — the raw error never reaches the wire.
        Err(_) => house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "render failed — see server logs",
        ),
    })
}
