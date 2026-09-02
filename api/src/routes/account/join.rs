// /api/join.
//
// Public invite lookup for the /join page: token → who's invited, by whom, to
// which org. Expired/revoked/accepted tokens read as gone.
//
// Unauthenticated by design — which makes it an oracle that names the org and
// the invited address for every VALID token — so both axes are braked, the
// same dual-counter shape as the login route. (Tokens are 24 random bytes;
// guessing one is not the threat. Hammering is.)

use crate::error::house_error;
use crate::invites::invite_by_token;
use crate::ratelimit::{client_ip, rate_limit};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Page loads per window per client — generous: a NAT'd office shares one
/// counter.
const IP_LIMIT: i64 = 120;
/// Lookups against any one token.
const TOKEN_LIMIT: i64 = 20;
const WINDOW_SECONDS: i64 = 15 * 60;

#[derive(serde::Deserialize)]
pub struct JoinQuery {
    token: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<JoinQuery>,
) -> Response {
    let Some(token) = query.token.filter(|t| !t.is_empty()) else {
        return house_error(StatusCode::BAD_REQUEST, "missing token");
    };
    // Either limiter refusing answers 429; a Redis outage fails open (the
    // limiter's own rule).
    let ip_key = format!("join:ip:{}", client_ip(&headers));
    let token_key = format!("join:token:{token}");
    let mut limited = None;
    if let Ok(mut redis) = state.redis().await {
        let by_ip = rate_limit(&mut redis, &ip_key, IP_LIMIT, WINDOW_SECONDS).await;
        let by_token = rate_limit(&mut redis, &token_key, TOKEN_LIMIT, WINDOW_SECONDS).await;
        limited = if !by_ip.ok {
            Some(by_ip)
        } else {
            (!by_token.ok).then_some(by_token)
        };
    }
    if let Some(l) = limited {
        let mut resp = house_error(
            StatusCode::TOO_MANY_REQUESTS,
            "too many attempts, try again shortly",
        );
        if let Ok(v) = header::HeaderValue::from_str(&l.retry_after.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return resp;
    }
    match invite_by_token(&state.pg, &token).await {
        Ok(Some(invite)) => Json(json!({ "invite": invite })).into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "invite not found or no longer valid"),
        Err(e) => {
            tracing::error!("[join] invite lookup failed: {e}");
            crate::error::thrown_internal_error()
        }
    }
}
