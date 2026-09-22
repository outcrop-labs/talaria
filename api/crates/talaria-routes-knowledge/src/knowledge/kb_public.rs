// /api/kb/public/{slug}. Public doc read — no auth. Only docs with visibility
// 'public' resolve; the response body is title/body/updatedAt only (routing
// and every other internal column stay off the public wire).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::kb::get_public_doc;
use talaria_error::{house_error, internal};
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    let doc = match get_public_doc(&state.pg, &slug).await {
        Ok(d) => d,
        Err(e) => return internal("[kb] public doc read failed", e),
    };
    let Some(doc) = doc else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    Json(json!({
        "doc": {
            "title": doc.title,
            "body": doc.body,
            "updatedAt": doc.updated_at,
        }
    }))
    .into_response()
}
