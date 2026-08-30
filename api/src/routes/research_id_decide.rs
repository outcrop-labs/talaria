// /api/research/{id}/decide — port of ui/src/routes/api/research.$id.decide.ts.
//
// THE EXIT FROM 'awaiting', on the run's own surface. A parked run is an
// approval (runs/decide.ts files it with the approvals machinery), and the
// research view is where the person it asked is already looking — the
// question renders in place via the projection's `awaiting` field, and this
// is the button under it. No second inbox; the run's own page IS the approval
// surface.
//
// THE AUTHORITY IS decide()'s, not this route's. `decide` checks the run's
// declared audience (the owner; admins for an org run) through the same
// `mayDecide` the digest and the SLA use, and this file adds only the
// visibility gate every research route starts with — a stranger probing ids
// learns nothing: no role is a 404, before decide says anything at all.
//
// THE ANSWER IS DATA: an `optionId` the step offered, plus an optional note.
// Both are clamped here and re-validated against the question the run is
// parked on RIGHT NOW inside `decide` — a stale tab answering last week's
// question is a 409, not a resume.

use crate::body::{as_object, optional_string_member, parse, string_member};
use crate::error::house_error;
use crate::realtime::RealtimeDeps;
use crate::research::research_role;
use crate::runs::decide::{DecideArgs, DecideRefusal, DecideResult, decide};
use crate::runs::real_decide_deps;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("research", "POST decide", &id) {
        return gate;
    }
    match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[research] role read on decide failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    }

    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let option_id = match string_member(obj, "optionId", 1, 200) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let note = match optional_string_member(obj, "note", 2000) {
        Ok(n) => n,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // TS spreads `note` in only when present AND non-blank — a whitespace
    // note is an absent note.
    let note = note.filter(|n| !n.trim().is_empty());

    let redis = match state.redis().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[research] redis for decide deps unavailable: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let deps = real_decide_deps(
        state.pg.clone(),
        redis.clone(),
        RealtimeDeps::publish_only(Some(redis)),
    );
    let res = match decide(
        DecideArgs {
            run_id: id.clone(),
            option_id,
            note,
            by: user.id,
            start: None,
        },
        &deps,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[research] decide on {id} failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    match res {
        DecideResult::Decided { run, .. } => {
            Json(json!({ "ok": true, "status": run.state, "phase": run.phase })).into_response()
        }
        DecideResult::Refused { reason, .. } => match reason {
            // No run under this id (or none since before the runs port). Same
            // answer as the visibility gate, for the same reason.
            DecideRefusal::Missing => house_error(StatusCode::NOT_FOUND, "not found"),
            // Answered, cancelled, or never parked. Two people racing one
            // question is the common cause and is not an error worth showing:
            // somebody answered.
            DecideRefusal::NotAwaiting | DecideRefusal::StaleKey => (
                StatusCode::CONFLICT,
                Json(json!({ "error": "this run is not waiting on a decision" })),
            )
                .into_response(),
            // Deliberately one sentence for "not in the audience" and "nobody
            // is": a route that distinguished them would say whose run this is.
            DecideRefusal::Forbidden => {
                house_error(StatusCode::FORBIDDEN, "you cannot decide this run")
            }
            DecideRefusal::UnknownOption => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "not one of the options this run offered" })),
            )
                .into_response(),
        },
    }
}
