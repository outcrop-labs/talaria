// GET /api/runs/{id}/events → SSE stream of one run's live transitions (state,
// phase, terminal error). Auth-gated by the run's read ACL. This is what makes
// a long action
// attachable: a tab that was closed, a view that was navigated away from, or a
// second device can re-attach to the SAME server-owned record and see where it
// actually is.
//
// THE ORDER OF THE TWO STEPS IN THE BODY IS LOAD-BEARING. The gate runs to
// completion BEFORE `run_event_stream`, which is the only arrangement that gets
// both halves right at once:
//
//   the ACL half      — obvious, and the same shape as boards/channels.
//   the RESOURCE half — `run_event_stream` opens a DEDICATED Redis subscriber
//                       per client. Creating one and then returning 403 would
//                       leak a subscriber per rejected request, and a rejected
//                       request is exactly the kind a caller retries in a loop.
//                       Nothing downstream disconnects a stream nobody was
//                       handed: the subscriber's forwarder exits when its
//                       receiver drops, and a receiver that was never returned
//                       into a response never drops on the client's say-so.
//
// Every refusal answers 403, including "no such run" — see `RunWatchVerdict`
// about not turning a guessable id into an existence oracle.
//
// The reclaim sweep and run kinds are armed by the scheduler's explicit job
// declarations, not by anything in this route.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use talaria_error::{house_error, internal};
use talaria_realtime_watch::{
    RealtimeDeps, RunWatchVerdict, may_watch_run, real_watch_deps, run_event_stream,
};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let verdict = match may_watch_run(&user.id, &run_id, &real_watch_deps(state.pg.clone())).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(internal(
                &format!("[runs/events] watch gate failed for {run_id}"),
                e,
            ));
        }
    };
    if verdict != RunWatchVerdict::Ok {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let deps = RealtimeDeps::streams_only(&state.cfg.redis_url);
    Ok(run_event_stream(&deps, &run_id).await)
}
