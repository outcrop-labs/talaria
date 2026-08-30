// /api/artifacts/public/{slug} — port of ui/src/routes/api/artifacts.public.$slug.ts.
// Public artifact read — NO AUTH. Only artifacts set to 'public' resolve, and
// the response is a deliberate subset: never the id, the owner, the folder,
// the routing, or anything else the full artifact carries.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::artifacts::get_public_artifact;
use crate::error::house_error;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    let a = match get_public_artifact(&state.pg, &slug).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] public read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(a) = a else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    Json(json!({
        "artifact": {
            "kind": a.kind,
            "title": a.title,
            "icon": a.icon,
            "body": a.body,
            "updatedAt": a.updated_at,
        }
    }))
    .into_response()
}
