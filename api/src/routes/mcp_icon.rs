// /api/mcp/icon — port of ui/src/routes/api/mcp.icon.ts.
// FALLBACK marketplace icons: the publisher's favicon, proxied + cached
// server-side (warmed in bulk when library pages are served). Registry-
// declared icons hotlink directly from the client and never come through
// here.

use crate::mcp_icons::{IconKey, icons};
use crate::session::require_user;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

#[derive(serde::Deserialize)]
pub struct IconQuery {
    domain: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IconQuery>,
) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    let Some(domain) = query.domain else {
        return crate::error::house_error(StatusCode::BAD_REQUEST, "bad request");
    };
    // `/^[a-z0-9.-]+$/i` — a bare host shape, nothing else.
    let shape_ok = !domain.is_empty()
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !shape_ok {
        return crate::error::house_error(StatusCode::BAD_REQUEST, "bad request");
    }
    let Some(icon) = icons()
        .resolve_icon(&IconKey {
            src: None,
            domain: Some(domain),
        })
        .await
    else {
        // `new Response(null, { status: 404 })` — an empty body and NO
        // content-type; axum's `(status, "")` pair would stamp text/plain on it.
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(axum::body::Body::empty())
            .expect("a bare 404 builds");
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, icon.content_type.clone()),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        icon.buf.clone(),
    )
        .into_response()
}
