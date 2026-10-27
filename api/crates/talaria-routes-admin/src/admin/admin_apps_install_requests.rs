// /api/admin/apps/install-requests. Agents' requests to install the app they
// built (the workbench's `request_app_install`). GET → the pending queue;
// PUT → approve (talaria-apps' credentialed install from the requested repo
// and ref, then the plugin record: the requesting agent maintains it, the
// ticket's person owns it, the ticket's board takes its maintenance tickets)
// or reject (the note goes on the requesting ticket). Admin — approval adds
// code to the deployment. Approval installs only: the app lands disabled,
// and enabling it and granting its surfaces stay admin steps.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_apps::{AppHost, KeptDataChoice, PlatformCredentials};
use talaria_body::{
    enum_member, optional_enum_member, optional_max_string_member, parse, uuid_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_plugins::install::{DecideError, Decision, decide, pending_requests};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    match pending_requests(&state.pg).await {
        Ok(rows) => Ok(Json(json!({ "requests": rows })).into_response()),
        Err(e) => Ok(internal(
            "[admin/apps/install-requests] queue read failed",
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
    // Why the admin decided; posted on the requesting ticket with the outcome.
    let note = match optional_max_string_member(obj, "note", 500) {
        Ok(v) => v.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // keptData: the answer to a 409 — data an earlier install of the slug
    // kept from another repository is adopted, or wiped first.
    let kept_data = match optional_enum_member(obj, "keptData", &["adopt", "wipe"]) {
        Ok(k) => k.map(|k| {
            if k == "wipe" {
                KeptDataChoice::Wipe
            } else {
                KeptDataChoice::Adopt
            }
        }),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = secretbox_or_500(
        &state,
        "[admin/apps/install-requests] secretbox unavailable",
    )
    .await?;
    let actor = actor_of(&user);
    let creds = PlatformCredentials {
        pg: &state.pg,
        sb: &sb,
        actor: &actor,
    };
    let host = AppHost::new(state.cfg.secret_root.internal_token());
    let decision = if action == "approve" {
        Decision::Approve { kept_data }
    } else {
        Decision::Reject
    };
    Ok(
        match decide(
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
                "sha": decided.sha,
                "build": decided.build,
                "buildError": decided.build_error,
                "warning": decided.warning,
            }))
            .into_response(),
            Err(DecideError::NotFound) => {
                house_error(StatusCode::NOT_FOUND, "not found or already decided")
            }
            // Kept data from another repository stands in the way: the
            // request is still pending, and a second approve with keptData
            // answers it.
            Err(DecideError::KeptData { error, conflict }) => (
                StatusCode::CONFLICT,
                Json(json!({ "error": error, "conflict": conflict })),
            )
                .into_response(),
            Err(DecideError::Db(e)) => internal("[admin/apps/install-requests] decision failed", e),
        },
    )
}
