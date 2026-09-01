// /api/kb/docs/{id}/backlinks — port of ui/src/routes/api/kb.docs.$id.backlinks.ts.
// Docs that link to this one ("linked from"). Editor links point at
// /knowledge/<id>, so backlinks fall out of a substring match. Gated by the
// SAME per-doc ACL as reading the doc — backlink titles leak content.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::{house_error, thrown_internal_error};
use crate::kb::{effective_doc_perms, get_backlinks, get_doc};
use crate::kb_perms::can_read;
use crate::session::{require_user, who_of};
use crate::state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let doc = match get_doc(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[kb] doc read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(doc) = doc else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let eff = match effective_doc_perms(&state.pg, &doc).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[kb] perms read failed: {e}");
            return thrown_internal_error();
        }
    };
    let who = who_of(&user);
    if !can_read(&eff.perms, Some(&user.id), who.as_deref(), &eff.grants) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match get_backlinks(&state.pg, &id).await {
        Ok(backlinks) => Json(json!({ "backlinks": backlinks })).into_response(),
        Err(e) => {
            tracing::error!("[kb] backlink scan failed: {e}");
            thrown_internal_error()
        }
    }
}
