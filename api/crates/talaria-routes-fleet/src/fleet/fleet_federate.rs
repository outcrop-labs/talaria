// POST /api/fleet/federate. Federate outside agents into Talaria: read a
// Hermes-format directory and create each agent natively (Talaria def,
// fresh key + state volume, our chassis, skills copied in). One-way and
// re-runnable. Admin.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::federate::federate_from_dir;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, trimmed_string_member};
use talaria_error::{house_error, object_or_400};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // trim, 1..500 — a server-side path to a Hermes-format directory
    // (admin trust model).
    let dir = match trimmed_string_member(obj, "dir", 1, 500) {
        Ok(d) => d,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
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
    Ok(Json(json!({ "result": result })).into_response())
}
