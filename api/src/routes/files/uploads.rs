// /api/uploads.
// POST (multipart/form-data, field "file") → store an attachment, return its
// metadata. Any signed-in user with the upload perm may upload; the file is
// served back from /api/uploads/{id}. The body is read through
// read_upload_form — an oversized upload is refused before it is buffered
// (413), not discovered after.

use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::error::house_error;
use crate::session::require_perm;
use crate::state::AppState;
use crate::uploads::{FormRead, read_upload_form, save_upload};

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    // Result, not Multipart: the bare extractor would reject a body without
    // a multipart boundary BEFORE this handler runs — answering its own 400
    // text ahead of the permission gate. Capturing the rejection here keeps
    // the gate first and folds it into the `no file` answer every read
    // failure takes.
    multipart: Result<Multipart, axum::extract::multipart::MultipartRejection>,
) -> Response {
    let user = match require_perm(&state, &headers, "files.upload").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let multipart = match multipart {
        Ok(m) => m,
        Err(_) => return house_error(StatusCode::BAD_REQUEST, "no file"),
    };
    let read = read_upload_form(&headers, multipart).await;
    let (filename, mime, bytes) = match read {
        FormRead::TooLarge => {
            return house_error(StatusCode::PAYLOAD_TOO_LARGE, "file too large (max 25 MB)");
        }
        // Malformed and NoFile both answer `no file` — the distinction is for
        // the log line, not the wire.
        FormRead::Malformed | FormRead::NoFile => {
            return house_error(StatusCode::BAD_REQUEST, "no file");
        }
        FormRead::File(name, mime, bytes) => (name, mime, bytes),
    };
    let filename = if filename.is_empty() {
        "file".to_string()
    } else {
        filename
    };
    let mime = if mime.is_empty() {
        "application/octet-stream".to_string()
    } else {
        mime
    };
    let sb = state.secretbox().await.unwrap_or_default();
    match save_upload(&state.pg, &sb, &filename, &mime, &bytes, Some(&user.id)).await {
        // The attachment object itself, not wrapped.
        Ok(att) => Json(att).into_response(),
        // Storage faults and row faults are the server's, not the request's:
        // a blob that cannot be written or recorded is a 500, never a 400
        // blaming a body that was fine.
        Err(msg) => {
            tracing::error!("[uploads] save failed: {msg}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "upload failed")
        }
    }
}
