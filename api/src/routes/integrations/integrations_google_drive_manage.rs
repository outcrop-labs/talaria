// /api/integrations/google/drive/{rename,move,trash,create-folder}. The
// Drive place's management verbs — all POST, all audited (a Drive write is a
// cross-boundary mutation: Google's Drive is not ours, and the audit log is
// where "who moved this" answers from).
//
// GATING: the `d` key's connection resolves the token (org never falls back
// to personal and the reverse), and ORG writes additionally require an admin
// — the org connection is admin-granted, so acting with it is an admin act;
// browse/import from org drives stay member-readable.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::google::connections::{RequireError, require_token};
use crate::google::drive::{
    create_drive_folder_with_token, move_drive_file_with_token, rename_drive_file_with_token,
    trash_drive_file_with_token,
};
use crate::google::errors::{GoogleError, google_fail_with};
use crate::session::{require_admin, require_user};
use crate::state::AppState;

/// Resolve `d` to (connection, root-parent). The root parent is what a move
/// to the drive's top or a folder creation with no parent means: 'root' for
/// a personal My Drive, the shared drive id itself otherwise.
fn drive_of(d: &str) -> Option<(String, String)> {
    let (connection, id) = d.split_once(':')?;
    match (connection, id) {
        ("personal", "my") => Some((connection.into(), "root".into())),
        ("org", "my") => Some((connection.into(), "root".into())),
        (_, _) if id.len() > 4 => Some((connection.into(), id.into())),
        _ => None,
    }
}

struct Ctx {
    token: String,
    connection: String,
    root_parent: String,
    is_admin: bool,
    actor: String,
}

/// The shared door: parse the body, resolve the token, gate org writes.
async fn gate(
    state: &AppState,
    headers: &HeaderMap,
    obj: &serde_json::Map<String, Value>,
) -> Result<Ctx, Response> {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return Err(gate),
    };
    let d = match string_member(obj, "d", 3, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return Err(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let Some((connection, root_parent)) = drive_of(&d) else {
        return Err(house_error(StatusCode::BAD_REQUEST, "malformed drive key"));
    };
    // Org writes are admin acts (the org connection is admin-granted).
    let mut is_admin = false;
    if connection == "org" {
        match require_admin(state, headers).await {
            Ok(_admin) => is_admin = true,
            Err(gate) => return Err(gate),
        }
        let _ = &user;
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default();
    let token = match connection.as_str() {
        "personal" => match require_token(&state.pg, &sb, &user.id, now).await {
            Ok(t) => t,
            Err(RequireError::NotConnected) => {
                return Err(google_fail_with(
                    GoogleError::NotConnected,
                    "Drive",
                    "drive_error",
                ));
            }
            Err(e) => {
                return Err(google_fail_with(
                    GoogleError::Failed(format!("{e:?}")),
                    "Drive",
                    "drive_error",
                ));
            }
        },
        _ => match crate::google::org::get_org_access_token(&state.pg, &sb, now).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                return Err(google_fail_with(
                    GoogleError::NotConnected,
                    "Drive",
                    "drive_error",
                ));
            }
            Err(e) => {
                return Err(google_fail_with(
                    GoogleError::Failed(format!("{e:?}")),
                    "Drive",
                    "drive_error",
                ));
            }
        },
    };
    Ok(Ctx {
        token,
        connection,
        root_parent,
        is_admin,
        actor: user.email.clone().unwrap_or_else(|| "user".into()),
    })
}

fn parse_body(body: Value) -> Result<serde_json::Map<String, Value>, Response> {
    match as_object(&body) {
        Ok(obj) => Ok(obj.clone()),
        Err(msg) => Err(house_error(StatusCode::BAD_REQUEST, &msg)),
    }
}

pub async fn rename(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Json<Value>,
) -> Response {
    let obj = match parse_body(body.0) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let ctx = match gate(&state, &headers, &obj).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let file_id = match string_member(&obj, "fileId", 1, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(&obj, "name", 1, 512) {
        Ok(v) => v.trim().to_string(),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if name.is_empty() {
        return house_error(StatusCode::BAD_REQUEST, "name cannot be empty");
    }
    match rename_drive_file_with_token(&ctx.token, &file_id, &name).await {
        Ok(file) => {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &ctx.actor,
                    action: "drive.rename",
                    target_type: "google_drive_file",
                    target_id: Some(&file_id),
                    target_label: Some(&name),
                    before: None,
                    after: Some(json!({ "name": name })),
                },
            )
            .await;
            Json(json!({ "file": file })).into_response()
        }
        Err(e) => google_fail_with(e, "Drive", "drive_error"),
    }
}

pub async fn drive_move(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Json<Value>,
) -> Response {
    let obj = match parse_body(body.0) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let ctx = match gate(&state, &headers, &obj).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let file_id = match string_member(&obj, "fileId", 1, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let remove = obj
        .get("removeParent")
        .and_then(|v| v.as_str())
        .map(String::from);
    // An absent add means the drive root; the caller states the remove end.
    let add = obj
        .get("addParent")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| ctx.root_parent.clone());
    let token = ctx.token.clone();
    let actor = ctx.actor.clone();
    let before = remove.as_deref().map(|r| json!({ "parent": r }));
    let after = json!({ "parent": add });
    let res = move_drive_file_with_token(&token, &file_id, Some(&add), remove.as_deref()).await;
    match res {
        Ok(()) => {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "drive.move",
                    target_type: "google_drive_file",
                    target_id: Some(&file_id),
                    target_label: None,
                    before,
                    after: Some(after),
                },
            )
            .await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => google_fail_with(e, "Drive", "drive_error"),
    }
}

pub async fn trash(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Json<Value>,
) -> Response {
    let obj = match parse_body(body.0) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let ctx = match gate(&state, &headers, &obj).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let file_id = match string_member(&obj, "fileId", 1, usize::MAX) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match trash_drive_file_with_token(&ctx.token, &file_id).await {
        Ok(()) => {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &ctx.actor,
                    action: "drive.trash",
                    target_type: "google_drive_file",
                    target_id: Some(&file_id),
                    target_label: None,
                    before: Some(json!({ "trashed": false })),
                    after: Some(json!({ "trashed": true })),
                },
            )
            .await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => google_fail_with(e, "Drive", "drive_error"),
    }
}

pub async fn create_folder(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Json<Value>,
) -> Response {
    let obj = match parse_body(body.0) {
        Ok(o) => o,
        Err(r) => return r,
    };
    let ctx = match gate(&state, &headers, &obj).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let name = match string_member(&obj, "name", 1, 512) {
        Ok(v) => v.trim().to_string(),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if name.is_empty() {
        return house_error(StatusCode::BAD_REQUEST, "name cannot be empty");
    }
    let parent = obj.get("parent").and_then(|v| v.as_str()).map(String::from);
    let parent_ref = parent.as_deref().unwrap_or(ctx.root_parent.as_str());
    match create_drive_folder_with_token(&ctx.token, &name, Some(parent_ref)).await {
        Ok(file) => {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &ctx.actor,
                    action: "drive.create_folder",
                    target_type: "google_drive_file",
                    target_id: Some(&file.id),
                    target_label: Some(&name),
                    before: None,
                    after: Some(json!({ "name": name, "parent": parent_ref })),
                },
            )
            .await;
            Json(json!({ "file": file })).into_response()
        }
        Err(e) => google_fail_with(e, "Drive", "drive_error"),
    }
}

// keep thrown_internal_error linked even if a future arm drops it
#[allow(dead_code)]
fn _unused() -> Response {
    thrown_internal_error()
}
