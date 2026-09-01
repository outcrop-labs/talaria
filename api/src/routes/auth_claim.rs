// POST /api/auth/claim { email, password, name? } — port of
// ui/src/routes/api/auth/claim.ts. The FIRST admin. Offered only while the
// instance has zero admins (GET /api/auth/providers → claimable); claim's
// advisory lock closes the race, so a lost race is a 409, never a second
// admin. Reachable by whoever gets there first on a fresh install — by
// design: whoever deploys, owns (same trust model as the Google claim, which
// needs no form at all).

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, optional_string_member, parse, preprocessed_email_member, string_member,
};
use crate::claim::claim_admin;
use crate::error::{house_error, thrown_internal_error};
use crate::password::hash_password;
use crate::ratelimit::{client_ip, rate_limit, rate_limit_reset};
use crate::session::{SessionUser, WireUser, create_session, json_with_cookies, session_cookie};
use crate::state::AppState;
use crate::users::Identity;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct ClaimBody {
    ok: bool,
    user: WireUser,
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let email = match preprocessed_email_member(obj, "email", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let password = match string_member(obj, "password", 8, 1000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match optional_string_member(obj, "name", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let ip_key = format!("claim:ip:{}", client_ip(&headers));
    if let Ok(mut redis) = state.redis().await {
        let by_ip = rate_limit(&mut redis, &ip_key, 10, 15 * 60).await;
        if !by_ip.ok {
            let mut resp = house_error(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many attempts, try again shortly",
            );
            if let Ok(v) = header::HeaderValue::from_str(&by_ip.retry_after.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
            return resp;
        }
    }

    // scrypt is ~100ms of CPU; keeping it off the async workers is the point
    // of doing this login plane in Rust.
    let pw = password.clone();
    let hash = match tokio::task::spawn_blocking(move || hash_password(&pw)).await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("[auth/claim] hash task panicked: {e}");
            return thrown_internal_error();
        }
    };

    // `parsed.name?.trim() || parsed.email` — a name that trims to empty falls
    // back to the email.
    let display_name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| email.clone());
    let identity = Identity {
        sub: format!("password:{email}"),
        email: Some(email.clone()),
        name: Some(display_name),
        picture: None,
    };
    let claimed = match claim_admin(&state.pg, &identity, Some(&hash)).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return house_error(
                StatusCode::CONFLICT,
                "This instance already has an admin — sign in instead.",
            );
        }
        Err(e) => {
            tracing::error!("[auth/claim] claim failed: {e}");
            return thrown_internal_error();
        }
    };

    if let Ok(mut redis) = state.redis().await {
        rate_limit_reset(&mut redis, &ip_key).await;
    }
    // No session exists yet, so the actor is the claimed email itself. Audit
    // failures never break the claim (logAudit ends in .catch(() => {})).
    let pg = state.pg.clone();
    let (cid, role) = (claimed.0.clone(), claimed.5.clone());
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &email,
                action: "auth.claim",
                target_type: "user",
                target_id: Some(&cid),
                target_label: None,
                before: None,
                after: Some(serde_json::json!({
                    "email": email,
                    "role": role,
                    "provider": "password",
                })),
            },
        )
        .await;
    });
    let user = SessionUser {
        id: claimed.0,
        sub: claimed.1,
        email: claimed.2,
        name: claimed.3,
        picture: claimed.4,
        role: claimed.5,
        provider: "password".into(),
    };
    let sid = match create_session(&state, &user).await {
        Ok(sid) => sid,
        Err(e) => {
            tracing::error!("[auth/claim] session create failed: {e}");
            return thrown_internal_error();
        }
    };
    json_with_cookies(
        Json(ClaimBody {
            ok: true,
            user: WireUser::from(&user),
        }),
        &[session_cookie(&sid)],
    )
    .into_response()
}
