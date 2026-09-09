// /api/integrations/google/drive/browse?d=<rosterKey>&parent=&q=&pageSize=&pageToken=&sort=
// One folder of one Drive, folders included, paginated, with the walked path
// for breadcrumbs. `d` names the connection AND the drive
// (`personal:my`, `personal:<sharedDriveId>`, `org:<sharedDriveId>`,
// `org:my`) — the org token is never resolved from a personal key or the
// reverse, the same discipline the agent routes hold.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::google::connections::{RequireError, require_token};
use crate::google::drive::browse_drive_with_token;
use crate::google::errors::{GoogleError, google_fail_with};
use crate::google::oauth::query_pairs;
use crate::session::require_user;
use crate::state::AppState;

/// Resolve `d` to (connection, kind, drive id). None = malformed key.
fn parse_drive_key(d: &str) -> Option<(&str, &str, &str)> {
    let (connection, id) = d.split_once(':')?;
    match (connection, id) {
        ("personal", "my") => Some(("personal", "my", "my")),
        ("org", "my") => Some(("org", "my", "my")),
        ("personal", _) | ("org", _) if !id.is_empty() => Some((connection, "shared", id)),
        _ => None,
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let qp = query_pairs(uri.query());
    let Some(d) = qp.get("d").cloned() else {
        return crate::error::house_error(axum::http::StatusCode::BAD_REQUEST, "missing drive");
    };
    let Some((connection, kind, drive_id)) = parse_drive_key(&d) else {
        return crate::error::house_error(
            axum::http::StatusCode::BAD_REQUEST,
            "malformed drive key",
        );
    };
    let parent = qp.get("parent").cloned().filter(|p| !p.is_empty());
    let q = qp.get("q").cloned().filter(|q| !q.is_empty());
    let page_size = qp
        .get("pageSize")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    let page_token = qp.get("pageToken").cloned().filter(|t| !t.is_empty());
    // Drive's orderBy speaks its own names; map the browser's columns.
    let sort = match qp.get("sort").map(String::as_str) {
        Some("name") => "name",
        Some("kind") => "folder,name",
        Some("owner") => "folder,name",
        Some("modified") => "folder,modifiedTime desc",
        _ => "folder,name",
    }
    .to_string();

    let sb = state.secretbox().await.unwrap_or_default();
    let now = now_ms();
    let token = match connection {
        "personal" => match require_token(&state.pg, &sb, &user.id, now).await {
            Ok(t) => Ok(t),
            Err(RequireError::NotConnected) => Err(GoogleError::NotConnected),
            Err(e) => Err(GoogleError::Failed(format!("{e:?}"))),
        },
        _ => crate::google::org::get_org_access_token(&state.pg, &sb, now)
            .await
            .map_err(|e| GoogleError::Failed(e.to_string()))
            .and_then(|t| t.ok_or(GoogleError::NotConnected)),
    };
    let token = match token {
        Ok(t) => t,
        Err(e) => return google_fail_with(e, "Drive", "drive_error"),
    };

    // None browses a personal My Drive; the shared-drive id browses it.
    let shared_drive_id = (kind == "shared").then_some(drive_id);
    match browse_drive_with_token(
        &token,
        parent.as_deref(),
        shared_drive_id,
        q.as_deref(),
        page_size,
        page_token.as_deref(),
        &sort,
    )
    .await
    {
        Ok(page) => Json(json!({ "files": page.files, "nextPageToken": page.next_page_token, "path": page.path })).into_response(),
        Err(e) => google_fail_with(e, "Drive", "drive_error"),
    }
}

/// Epoch-ms clock — the one time the Drive surface reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
