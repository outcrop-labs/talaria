// GET /api/me/events → SSE stream of THIS person's own firehose: their runs
// changing state, their notifications landing, their brief being appended to.
//
// The per-device attach point. A run started on a laptop and parked on a
// question at 11pm has to reach the phone, and it has to reach it without the
// phone knowing which run to watch — which is the one thing `run:<id>` cannot
// do. Events here are id-shaped (see realtime's `UserEvent`); the client
// re-fetches through the ordinary routes, so this stream widens nothing.
//
// AUTHORIZATION IS THE SESSION. The topic is keyed by `user.id` taken from the
// resolved session and never from the request — no `?userId=`, no param, no
// route segment to tamper with. There is deliberately no way to ask for
// somebody else's firehose, which is why this route needs no check beyond
// being signed in: the only id it can construct is the caller's own.

use crate::realtime::{RealtimeDeps, user_event_stream};
use crate::session::require_user;
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let deps = RealtimeDeps::streams_only(&state.cfg.redis_url);
    user_event_stream(&deps, &user.id).await
}
