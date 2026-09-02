// POST /api/auth/password { username, password }. Sets the session cookie.
// Credentials live in user_password_credentials (Admin → People); the provider
// exists while any account does. No allow-list applies here — an account was
// admitted by an admin when it was created; login checks the stored hash only.

use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::password_accounts::{has_password_accounts, verify_password_login};
use crate::ratelimit::{client_ip, rate_limit, rate_limit_reset};
use crate::session::{SessionUser, WireUser, create_session, json_with_cookies, session_cookie};
use crate::state::AppState;
use crate::users::upsert_user;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::time::Duration;

// Brute-force brake. Two counters, because either one alone has a hole:
//   • per USERNAME — the one that actually bounds guessing at a given account,
//     and the only one an attacker can't sidestep by changing headers or hosts.
//   • per IP — catches spraying across many usernames, but only counts when a
//     trusted proxy is configured (see client_ip); otherwise every request
//     looks like the same 'direct' client, which is the safe direction.
// Both live in Redis, so they survive a restart and hold across instances.
const USER_LIMIT: i64 = 10;
const IP_LIMIT: i64 = 30;
const WINDOW_SECONDS: i64 = 15 * 60;

#[derive(serde::Serialize)]
struct LoginBody {
    ok: bool,
    user: WireUser,
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    match has_password_accounts(&state.pg).await {
        Ok(true) => {}
        Ok(false) => return house_error(StatusCode::BAD_REQUEST, "Password login is disabled"),
        Err(e) => {
            tracing::error!("[auth/password] account probe failed: {e}");
            return thrown_internal_error();
        }
    }

    // Parse first: the username is what the primary counter keys on.
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let username = match string_member(obj, "username", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let password = match string_member(obj, "password", 1, 1000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // Either limiter refusing answers 429; a Redis outage fails open (the
    // limiter's own rule).
    let user_key = format!("login:user:{}", username.trim().to_lowercase());
    let ip_key = format!("login:ip:{}", client_ip(&headers));
    let mut limited = None;
    if let Ok(mut redis) = state.redis().await {
        let by_user = rate_limit(&mut redis, &user_key, USER_LIMIT, WINDOW_SECONDS).await;
        let by_ip = rate_limit(&mut redis, &ip_key, IP_LIMIT, WINDOW_SECONDS).await;
        limited = if !by_user.ok {
            Some(by_user)
        } else {
            (!by_ip.ok).then_some(by_ip)
        };
    }
    if let Some(l) = limited {
        let mut resp = house_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts, try again shortly",
        );
        if let Ok(v) = header::HeaderValue::from_str(&l.retry_after.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return resp;
    }

    let identity = match verify_password_login(&state.pg, &username, &password).await {
        Ok(Some(i)) => i,
        Ok(None) => {
            // Slow the failure path a touch to blunt brute force.
            tokio::time::sleep(Duration::from_millis(400)).await;
            return house_error(StatusCode::UNAUTHORIZED, "Invalid credentials");
        }
        Err(e) => {
            tracing::error!("[auth/password] credential lookup failed: {e}");
            return thrown_internal_error();
        }
    };

    // A real login clears the budget so a fat-fingered morning doesn't lock
    // someone out for the rest of the window.
    if let Ok(mut redis) = state.redis().await {
        rate_limit_reset(&mut redis, &user_key).await;
    }
    let row = match upsert_user(
        &state.pg,
        &crate::users::Identity {
            sub: identity.sub,
            email: Some(identity.email),
            name: identity.name,
            picture: identity.picture,
        },
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[auth/password] upsert failed: {e}");
            return thrown_internal_error();
        }
    };
    let user = SessionUser {
        id: row.0,
        sub: row.1,
        email: row.2,
        name: row.3,
        picture: row.4,
        role: row.5,
        provider: "password".into(),
    };
    let sid = match create_session(&state, &user).await {
        Ok(sid) => sid,
        Err(e) => {
            tracing::error!("[auth/password] session create failed: {e}");
            return thrown_internal_error();
        }
    };
    json_with_cookies(
        Json(LoginBody {
            ok: true,
            user: WireUser::from(&user),
        }),
        &[session_cookie(&sid)],
    )
    .into_response()
}
