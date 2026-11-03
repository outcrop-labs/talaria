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
    // The outcome is recorded INSIDE the detached task, so a reconcile that
    // outlives the request's 75 s keeps it: each agent that would not start
    // is a warning, and after the 202 nobody reads the answer. Logged, and
    // the audit row carries it — there is no surface for the browser to
    // read a finished background reconcile from, and the roster shows each
    // agent's container as it comes up.
    let pg = state.pg.clone();
    let actor = actor_of(&user);
    let op = async move {
        let r = reconcile_fleet(&pg, &sb).await;
        let after = match &r {
            Ok(res) => {
                tracing::info!(
                    "[fleet] reconcile: started {:?}, already running {:?}, {} warning(s)",
                    res.started,
                    res.already_running,
                    res.warnings.len()
                );
                for w in &res.warnings {
                    tracing::warn!("[fleet] reconcile: {w}");
                }
                json!({
                    "started": res.started,
                    "alreadyRunning": res.already_running,
                    "warnings": res.warnings,
                })
            }
            Err(e) => {
                tracing::warn!("[fleet] reconcile failed: {e}");
                json!({ "error": e })
            }
        };
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
                    after: Some(after),
                },
            )
            .await;
        });
        r
    };
    let reconciled = match run_detached(op, ANSWER_WITHIN).await {
        Detached::Settled(r) => r,
        Detached::StillRunning => {
            return Ok((
                StatusCode::ACCEPTED,
                Json(json!({ "pending": true, "started": [], "alreadyRunning": [] })),
            )
                .into_response());
        }
        Detached::Lost(e) => Err(e),
    };
    Ok(match reconciled {
        // json(result) — UNwrapped (unlike /render).
        Ok(result) => Json(result).into_response(),
        Err(_) => house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "reconcile failed — see server logs",
        ),
    })
}
