// GET /api/runs/{id}/events → SSE stream of one run's live transitions (state,
// phase, terminal error) — the port of ui/src/routes/api/runs.$id.events.ts.
// Auth-gated by the run's read ACL. This is what makes a long action
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
// TS's `import '@/server/runs/boot'` side effect (arming the reclaim sweep and
// the run kinds) has no line here: Rust's scheduler registration is explicit
// and lands with the scheduler slice later in this batch, at which point this
// route is what puts it in the server graph.

use crate::error::house_error;
use crate::realtime::{
    RealtimeDeps, RunWatchVerdict, may_watch_run, real_watch_deps, run_event_stream,
};
use crate::session::require_user;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

pub async fn get(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let verdict = match may_watch_run(&user.id, &run_id, &real_watch_deps(state.pg.clone())).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[runs/events] watch gate failed for {run_id}: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if verdict != RunWatchVerdict::Ok {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let deps = RealtimeDeps::streams_only(&state.cfg.redis_url);
    run_event_stream(&deps, &run_id).await
}
