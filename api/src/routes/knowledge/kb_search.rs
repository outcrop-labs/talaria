// /api/kb/search. Full-text search across the knowledgebase (docs the caller
// can read). The engine (ranked union of docs + space overviews,
// effective-visibility ACL filter, sentinel highlighting) lives in
// kb::search_docs.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::thrown_internal_error;
use crate::kb::{SearchViewer, search_docs};
use crate::session::{require_user, who_of};
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // a missing q is the empty query, which the engine answers with no hits
    // (never an error).
    let q = uri
        .query()
        .and_then(|qs| {
            url::form_urlencoded::parse(qs.as_bytes())
                .find(|(k, _)| k == "q")
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_default();
    let who = who_of(&user);
    match search_docs(
        &state.pg,
        &q,
        SearchViewer {
            user_id: &user.id,
            who: who.as_deref(),
        },
    )
    .await
    {
        Ok(hits) => Json(json!({ "hits": hits })).into_response(),
        Err(e) => {
            tracing::error!("[kb] search failed: {e}");
            thrown_internal_error()
        }
    }
}
