// POST /api/integrations/google/drive/import { fileId } — port of
// ui/src/routes/api/integrations/google.drive.import.ts. Pull a Drive file in
// as a new artifact owned by the caller (Doc→doc, Sheet→sheet, else→file).

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::artifacts::{SaveArtifactPatch, create_artifact, record_google_export, save_artifact};
use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, house_error_msg};
use crate::google::drive::import_drive_file;
use crate::google::errors::{GoogleError, reconnect_needed};
use crate::session::require_user;
use crate::state::AppState;

pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // fileId: min 1, no max.
    let file_id = match string_member(obj, "fileId", 1, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let actor = user
        .email
        .clone()
        .unwrap_or_else(|| user.name.clone().unwrap_or_else(|| "user".to_string()));
    let sb = state.secretbox().await.unwrap_or_default();

    let content = match import_drive_file(&state.pg, &sb, &user.id, &file_id, now_ms()).await {
        Ok(c) => c,
        Err(e) => return import_failed(e),
    };
    let artifact = match create_artifact(
        &state.pg,
        Some(&content.kind),
        Some(&content.title),
        &actor,
        Some(&user.id),
        None,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[drive/import] artifact create failed: {e}");
            return import_failed(GoogleError::Failed(e.to_string()));
        }
    };
    if let Err(e) = save_artifact(
        &state.pg,
        &artifact.id,
        SaveArtifactPatch {
            title: Some(Some(content.title.as_str())),
            body: Some(content.body.as_str()),
            icon: None,
            storage_ref: Some(content.storage_ref.as_deref()),
            content_type: Some(content.content_type.as_deref()),
            folder_id: None,
            visibility: None,
            edit_policy: None,
        },
        &actor,
    )
    .await
    {
        tracing::error!("[drive/import] artifact save failed: {e}");
        return import_failed(GoogleError::Failed(e.to_string()));
    }
    // Remember where it came from so "Open in Google Drive" links back.
    if let Some(source_url) = &content.source_url
        && let Err(e) = record_google_export(&state.pg, &artifact.id, &file_id, source_url).await
    {
        tracing::error!("[drive/import] export record failed: {e}");
        return import_failed(GoogleError::Failed(e.to_string()));
    }
    // `{...artifact, kind, title}` — the spread keeps its field order; the
    // two overrides land on keys the artifact already carries.
    let mut wire = serde_json::to_value(&artifact).unwrap_or(Value::Null);
    if let Some(obj) = wire.as_object_mut() {
        obj.insert("kind".into(), json!(content.kind));
        obj.insert("title".into(), json!(content.title));
    }
    Json(json!({ "artifact": wire })).into_response()
}

/// The catch ladder — the import's own noun on the 502 rung, plus the
/// too-large sentence uploads throws when the file crosses 25 MB.
fn import_failed(e: GoogleError) -> Response {
    match e {
        GoogleError::NotConnected => house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            "Connect a Google account first.",
        ),
        GoogleError::Failed(m) if reconnect_needed(&m) => house_error_msg(
            StatusCode::CONFLICT,
            "reconnect_needed",
            "Reconnect Google to grant Drive access.",
        ),
        GoogleError::Failed(m) if m.to_lowercase().contains("too large") => house_error_msg(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "That file is over the 25 MB import limit.",
        ),
        GoogleError::Failed(m) => {
            tracing::error!("[drive/import] failed: {m}");
            house_error_msg(
                StatusCode::BAD_GATEWAY,
                "import_failed",
                "Could not import that Drive file.",
            )
        }
    }
}

/// Date.now() — the one clock the import reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
