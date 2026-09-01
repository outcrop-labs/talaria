// /api/agent-media/{model} — port of ui/src/routes/api/agent-media.$model.ts.
// GET ?path=/opt/data/ → stream an image out of the agent's container, so
// media agents produce ("MEDIA:<path>" in replies) renders inline in chat.
// Access + path/type guardrails live in agent_media.rs.

use crate::agent_media::read_agent_image;
use crate::error::house_error;
use crate::fleet::usable_agent_gate;
use crate::session::require_user;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct MediaQuery {
    path: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model): Path<String>,
    Query(query): Query<MediaQuery>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // Owner-aware: a personal assistant is only ever visible to its owner.
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[agent-media] gate read failed: {e}");
            return crate::error::thrown_internal_error();
        }
    };
    if !gate(&model) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let path = query.path.unwrap_or_default();
    match read_agent_image(&state.pg, &model, &path).await {
        Err(media) => {
            let status =
                StatusCode::from_u16(media.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            house_error(status, media.error)
        }
        Ok(media) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, media.mime.to_string()),
                (header::CACHE_CONTROL, "private, max-age=300".to_string()),
                (
                    header::X_CONTENT_TYPE_OPTIONS,
                    "nosniff".to_string(),
                ),
            ],
            media.bytes,
        )
            .into_response(),
    }
}
