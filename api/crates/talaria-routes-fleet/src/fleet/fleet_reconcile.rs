// POST /api/fleet/reconcile. Render + start every enabled managed agent
// that isn't running. One button to bring the fleet to desired state
// (drift, cold start). Admin.
//
// On a task of its own, answered within `ANSWER_WITHIN` like the control
// verbs: each start can be a download of the agent image first (minutes on a
// host that lacks it), and a request held past Cloudflare's 100 s reaches the
// browser as a 524 while the starts go on. Past the bound it answers 202
// `{pending: true}`; the roster shows each agent as it comes up.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::docker::{ANSWER_WITHIN, Detached, run_detached};
use talaria_api_facades::fleet::reconcile::reconcile_fleet;
use talaria_audit::{AuditEntry, log_audit};
use talaria_error::house_error;
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let sb = secretbox_or_500(&state, "[fleet] secretbox failed").await?;
    // The audit names the act, not its outcome: it is written once the
    // reconcile answered, or once it is still going past the bound.
    let audit = {
        let pg = state.pg.clone();
        let actor = actor_of(&user);
        move || {
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
        }
    };
    let pg = state.pg.clone();
    let reconciled = match run_detached(
        async move { reconcile_fleet(&pg, &sb).await },
        ANSWER_WITHIN,
    )
    .await
    {
        Detached::Settled(r) => r,
        Detached::StillRunning => {
            audit();
            return Ok((
                StatusCode::ACCEPTED,
                Json(json!({ "pending": true, "started": [], "alreadyRunning": [] })),
            )
                .into_response());
        }
        Detached::Lost(e) => Err(e),
    };
    Ok(match reconciled {
        Ok(result) => {
            audit();
            // json(result) — UNwrapped (unlike /render).
            Json(result).into_response()
        }
        Err(_) => house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "reconcile failed — see server logs",
        ),
    })
}
