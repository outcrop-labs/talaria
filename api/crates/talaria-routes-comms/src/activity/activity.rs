// GET /api/activity. The merged workspace activity feed, scoped to the
// requesting user.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use talaria_activity::{KINDS, activity_feed};
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct ActivityBody {
    events: Vec<talaria_activity::ActivityEvent>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // kinds: comma-split with unknown kinds dropped — an absent param is the
    // empty string, whose only split product ('') also drops.
    let kinds: Vec<String> = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("kinds="))
        .map(|v| {
            v.split(',')
                .filter(|k| KINDS.contains(k))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    match activity_feed(&state.pg, &user.id, &kinds, 80, user.role == "admin").await {
        Ok(events) => Json(ActivityBody { events }).into_response(),
        Err(e) => internal("[activity] feed query failed", e),
    }
}
