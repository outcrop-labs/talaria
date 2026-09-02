// /api/artifacts/public/{slug}/download.
// Public download for a public *file* artifact — NO AUTH. Serves the stored
// bytes; only resolves
// when the artifact is public and points at an upload. The inline/download
// decision lives in serve_upload — this route is UNAUTHENTICATED, so it
// especially may not widen that allowlist.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;

use crate::artifacts::get_public_artifact;
use crate::error::{house_error, thrown_internal_error};
use crate::state::AppState;
use crate::uploads::{get_upload, serve_upload};

pub async fn get(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    let a = match get_public_artifact(&state.pg, &slug).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] public read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(a) = a else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(storage_ref) = a
        .storage_ref
        .as_deref()
        .filter(|s| !s.is_empty() && a.kind == "file")
    else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let sb = state.secretbox().await.unwrap_or_default();
    let found = match get_upload(&state.pg, &sb, storage_ref).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[uploads] blob read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some((bytes, mime, filename)) = found else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    serve_upload(bytes, &mime, &filename, "public, max-age=3600")
}
