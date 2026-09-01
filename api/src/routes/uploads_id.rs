// /api/uploads/{id} — port of ui/src/routes/api/uploads.$id.ts.
// GET → serve an attachment's bytes: signed-in users, or fleet agents (agent
// key) pulling ticket/chat attachments they were handed. The inline/download
// decision lives in serve_upload — one allowlist, no route widens it on its
// own.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;

use crate::agent_auth::agent_caller;
use crate::error::thrown_internal_error;
use crate::session::{require_user, who_of};
use crate::state::AppState;
use crate::uploads::{UploadViewer, can_access_upload, get_upload, serve_upload, upload_not_found};

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    // Bytes only for viewers who can reach this upload through something
    // they can already read — never by id alone.
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let allowed = if let Some(caller) = caller {
        can_access_upload(
            &state.pg,
            &id,
            UploadViewer::Agent {
                model: &caller.model,
            },
        )
        .await
    } else {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        can_access_upload(
            &state.pg,
            &id,
            UploadViewer::Human {
                user_id: &user.id,
                who: who_of(&user).as_deref(),
                is_admin: user.role == "admin",
            },
        )
        .await
    };
    if !allowed {
        return upload_not_found();
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let found = match get_upload(&state.pg, &sb, &id).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[uploads] blob read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some((bytes, mime, filename)) = found else {
        return upload_not_found();
    };
    serve_upload(bytes, &mime, &filename, "private, max-age=86400")
}
