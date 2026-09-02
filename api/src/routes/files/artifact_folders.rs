// /api/artifact-folders — port of ui/src/routes/api/artifact-folders.ts.
// Artifact folders. GET → the ones you can read. POST → create one you own.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::artifacts::{create_folder, guarded_folder, list_folders};
use crate::body::{
    as_object, optional_enum_member, parse, present_nullable_uuid_member, string_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{can_read, granted_item_ids};
use crate::session::{require_perm, require_user, who_of};
use crate::state::AppState;

/// "artifact-folder" — the kb_editors item_type folders file under. Not a
/// kb_perms const: only this family uses it.
const ITEM_FOLDER: &str = "artifact-folder";

struct Body {
    name: String,
    parent_id: Option<Option<String>>,
    visibility: Option<String>,
}

fn parse_body(obj: &serde_json::Map<String, Value>) -> Result<Body, String> {
    Ok(Body {
        name: string_member(obj, "name", 1, 80)?,
        parent_id: present_nullable_uuid_member(obj, "parentId")?,
        visibility: optional_enum_member(obj, "visibility", &["private", "org", "public"])?,
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let who = who_of(&user);
    // Folders used to be returned wholesale, which was fine while they had no
    // access of their own. Now they do, so this read is gated exactly like the
    // artifact list beside it — same canRead, same grant escape hatch.
    let granted = match granted_item_ids(&state.pg, ITEM_FOLDER, &user.id).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[folders] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    let folders = match list_folders(&state.pg).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[folders] list failed: {e}");
            return thrown_internal_error();
        }
    };
    let folders: Vec<_> = folders
        .iter()
        .filter(|f| {
            granted.contains(&f.id)
                || can_read(&guarded_folder(f), Some(&user.id), who.as_deref(), &[])
        })
        .collect();
    Json(json!({ "folders": folders })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Folders shape the artifact tree — same perm as creating artifacts.
    let user = match require_perm(&state, &headers, "artifacts.create").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match parse_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let parent = match body.parent_id {
        Some(Some(id)) => Some(id),
        _ => None, // `body.parentId ?? null` — absent and explicit-null both
    };
    let folder = match create_folder(
        &state.pg,
        &body.name,
        parent.as_deref(),
        &who_of(&user).unwrap_or_else(|| "user".into()),
        Some(&user.id),
        // Omitted = private: a folder you make in My Files is yours until you
        // share it. Callers wanting the old org-wide default pass it explicitly.
        Some(body.visibility.as_deref().unwrap_or("private")),
    )
    .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[folders] create failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "folder": folder })).into_response()
}
