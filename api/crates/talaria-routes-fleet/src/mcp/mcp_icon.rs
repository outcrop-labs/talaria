// /api/mcp/icon.
// FALLBACK marketplace icons: the publisher's favicon, proxied + cached
// server-side (warmed in bulk when library pages are served). Registry-
// declared icons hotlink directly from the client and never come through
// here.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use talaria_api_facades::mcp::icons::{IconKey, icons};
use talaria_session::require_user;
use talaria_state::AppState;

#[derive(serde::Deserialize)]
pub struct IconQuery {
    domain: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IconQuery>,
) -> Result<Response, Response> {
    require_user(&state, &headers).await?;
    let Some(domain) = query.domain else {
        return Ok(talaria_error::house_error(
            StatusCode::BAD_REQUEST,
            "bad request",
        ));
    };
    // a bare host shape — alphanumerics, dots, hyphens; nothing else.
    let shape_ok = !domain.is_empty()
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !shape_ok {
        return Ok(talaria_error::house_error(
            StatusCode::BAD_REQUEST,
            "bad request",
        ));
    }
    let Some(icon) = icons()
        .resolve_icon(&IconKey {
            src: None,
            domain: Some(domain),
        })
        .await
    else {
        // an empty body and NO content-type; axum's `(status, "")` pair would
        // stamp text/plain on it.
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(axum::body::Body::empty())
            .expect("a bare 404 builds"));
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, icon.content_type.clone()),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        icon.buf.clone(),
    )
        .into_response())
}
