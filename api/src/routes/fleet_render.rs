// POST /api/fleet/render — port of ui/src/routes/api/fleet.render.ts. Render
// every managed agent's config + the fleet compose + the gateway manifest
// (the bridge hot-reloads the manifest). Admin.

use crate::audit::{log_audit, AuditEntry};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet_render::render_fleet;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(_) => return thrown_internal_error(),
    };
    match render_fleet(&state.pg, &sb, None).await {
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
            // json({ result }) — wrapped, keys in the interface's order.
            Json(json!({
                "result": {
                    "agents": result.agents,
                    "files": result.files,
                    "warnings": result.warnings,
                }
            }))
            .into_response()
        }
        // console.error + a house-shaped 500 sentence — not the throw shape.
        Err(_) => house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "render failed — see server logs",
        ),
    }
}
