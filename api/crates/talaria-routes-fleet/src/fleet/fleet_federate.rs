// POST /api/fleet/federate. Federate outside agents into Talaria: read a
// Hermes-format directory and create each agent natively (Talaria def,
// fresh key + state volume, our chassis, skills copied in). One-way and
// re-runnable. Admin. A roster that would put a second agent in a department
// (one another agent holds, or an earlier entry claims) is refused whole with
// a 409 that names each entry, and nothing is written: a department names one
// agent's container and memory.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::federate::federate_from_dir;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, trimmed_string_member};
use talaria_error::{house_error, object_or_400};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
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
    // A source config's literal credentials are sealed as per-agent secrets
    // on the way in.
    let sb = secretbox_or_500(&state, "[fleet/federate] secretbox failed").await?;
    let result = federate_from_dir(&state.pg, &sb, &dir, &actor).await;
    // A roster that would put two agents in one department imported nothing:
    // the conflict is the answer, each entry named.
    if !result.conflicts.is_empty() {
        return Ok(house_error(
            StatusCode::CONFLICT,
            &format!("nothing was federated — {}", result.conflicts.join("; ")),
        ));
    }
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
