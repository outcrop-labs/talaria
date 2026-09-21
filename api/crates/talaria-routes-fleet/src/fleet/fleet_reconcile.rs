// POST /api/fleet/reconcile. Render + start every enabled managed agent
// that isn't running. One button to bring the fleet to desired state
// (drift, cold start). Admin.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_api_facades::fleet::reconcile::reconcile_fleet;
use talaria_audit::{AuditEntry, log_audit};
use talaria_error::{house_error, internal};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => return internal("[fleet] secretbox failed", e),
    };
    match reconcile_fleet(&state.pg, &sb).await {
        Ok(result) => {
            let actor = actor_of(&user);
            let pg = state.pg.clone();
            tokio::spawn(async move {
                log_audit(
                    &pg,
                    AuditEntry {
                        actor: &actor,
                        action: "fleet.reconcile",
                        target_type: "fleet",
                        target_id: Some("fleet"),
                        target_label: None,
                        before: None,
                        after: None,
                    },
                )
                .await;
            });
            // json(result) — UNwrapped (unlike /render).
            Json(result).into_response()
        }
        Err(_) => house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "reconcile failed — see server logs",
        ),
    }
}
