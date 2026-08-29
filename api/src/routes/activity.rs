// GET /api/activity — port of ui/src/routes/api/activity.ts. The merged
// workspace activity feed, scoped to the requesting user.

use crate::activity::{KINDS, activity_feed};
use crate::error::house_error;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct ActivityBody {
    events: Vec<crate::activity::ActivityEvent>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // (new URL(request.url).searchParams.get('kinds') ?? '').split(',')
    //   .filter(k => KINDS.has(k)) — unknown kinds drop, an absent param is
    //   the empty string, whose only split product ('') also drops.
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
        Err(e) => {
            tracing::error!("[activity] feed query failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}
