// /api/admin/apps/update-requests. Plugin updates waiting for an admin (the
// `app_update` approval): the maintenance pass raises one when a plugin's
// branch moves past the commit installed — a merged PR. GET → the pending
// queue; PUT → approve (talaria-apps' update: fetch, build and probe beside
// the live app, swap, roll back if the new version fails) or reject. Either
// way the outcome is a line on the plugin's ticket. Admin — an update adds
// code to the deployment.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_apps::{AppHost, PlatformCredentials};
use talaria_body::{enum_member, optional_max_string_member, parse, uuid_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_plugins::update::{UpdateDecideError, UpdateDecision, decide_update, pending_updates};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    match pending_updates(&state.pg).await {
        Ok(rows) => Ok(Json(json!({ "requests": rows })).into_response()),
        Err(e) => Ok(internal(
            "[admin/apps/update-requests] queue read failed",
            e,
        )),
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let id = match uuid_member(obj, "id") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let action = match enum_member(obj, "action", &["approve", "reject"]) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Why the admin decided; posted on the plugin's ticket with the outcome.
    let note = match optional_max_string_member(obj, "note", 500) {
        Ok(v) => v.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = secretbox_or_500(&state, "[admin/apps/update-requests] secretbox unavailable").await?;
    let actor = actor_of(&user);
    let creds = PlatformCredentials {
        pg: &state.pg,
        sb: &sb,
        actor: &actor,
    };
    let host = AppHost::new(state.cfg.secret_root.internal_token());
    let decision = if action == "approve" {
        UpdateDecision::Approve
    } else {
        UpdateDecision::Reject
    };
    Ok(
        match decide_update(
            &state.pg,
            &creds,
            &host,
            &id,
            decision,
            note.as_deref(),
            &actor,
        )
        .await
        {
            Ok(decided) => Json(json!({
                "ok": decided.status != "failed",
                "status": decided.status,
                "line": decided.line,
                "slug": decided.slug,
                "from": decided.from,
                "to": decided.to,
                "upToDate": decided.up_to_date,
                "rolledBack": decided.rolled_back,
                "error": decided.error,
            }))
            .into_response(),
            Err(UpdateDecideError::NotFound) => {
                house_error(StatusCode::NOT_FOUND, "not found or already decided")
            }
            Err(UpdateDecideError::Db(e)) => {
                internal("[admin/apps/update-requests] decision failed", e)
            }
        },
    )
}
