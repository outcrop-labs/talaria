// POST /api/fleet/federate — port of ui/src/routes/api/fleet.federate.ts.
// Federate outside agents into Talaria: read a Hermes-format directory and
// create each agent natively (Talaria def, fresh key + state volume, our
// chassis, skills copied in). One-way and re-runnable. Admin.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, trimmed_string_member};
use crate::error::house_error;
use crate::fleet_federate::federate_from_dir;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

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
    // z.string().trim().min(1).max(500) — a server-side path to a
    // Hermes-format directory (admin trust model).
    let dir = match trimmed_string_member(obj, "dir", 1, 500) {
        Ok(d) => d,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let actor = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let result = federate_from_dir(&state.pg, &dir, &actor).await;
    if result.agents.iter().any(|a| a.status == "federated") {
        let actor = actor_of(&user);
        let label = dir.clone();
        let pg = state.pg.clone();
        tokio::spawn(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &actor,
                    action: "agent.federate",
                    target_type: "fleet",
                    target_id: Some("fleet"),
                    target_label: Some(&label),
                    before: None,
                    after: None,
                },
            )
            .await;
        });
    }
    Json(json!({ "result": result })).into_response()
}
