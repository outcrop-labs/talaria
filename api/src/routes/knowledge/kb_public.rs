// /api/kb/public/{slug} — port of ui/src/routes/api/kb.public.$slug.ts.
// Public doc read — no auth. Only docs with visibility 'public' resolve; the
// body of the response is title/body/updatedAt only (routing and every other
// internal column stay off the public wire).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::{house_error, thrown_internal_error};
use crate::kb::get_public_doc;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    let doc = match get_public_doc(&state.pg, &slug).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[kb] public doc read failed: {e}");
            return thrown_internal_error();
        }
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
