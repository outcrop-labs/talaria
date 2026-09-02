// GET /api/auth/google/callback. Verify the state cookie, exchange the code,
// mint the session, and land on the cockpit. Every failure bounces to /login
// with a machine-readable reason (the SPA renders each), extra params riding
// along where a door can name what to change.

use crate::audit::{AuditEntry, log_audit};
use crate::auth_config::{get_auth_config, is_email_allowed};
use crate::claim::{claim_admin, instance_claimable};
use crate::error::thrown_internal_error;
use crate::google::client::google_login_enabled;
use crate::google::oauth::{
    PROVIDER, email_domain_of, exchange_google_code, google_redirect_uri, org_login_allowed,
    org_login_email, query_pairs,
};
use crate::invites::{invite_allowed, mark_invite_accepted};
use crate::org_domains::self_join_allowed;
use crate::session::{
    STATE_COOKIE, SessionUser, clear_state_cookie, create_session, parse_cookies, session_cookie,
    state_matches,
};
use crate::state::AppState;
use crate::users::upsert_user;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

/// Bounce back to the login screen with a machine-readable reason (the
/// callback's loginError). Extra params ride along — the org-domain refusal
/// carries WHICH domain to name.
fn login_error(reason: &str, extra: &[(&str, &str)]) -> Response {
    let mut pairs: Vec<(&str, &str)> = vec![("error", reason)];
    pairs.extend_from_slice(extra);
    let qs = {
        let mut out = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in pairs {
            out.append_pair(k, v);
        }
        out.finish()
    };
    redirect(&format!("/login?{qs}"), &[clear_state_cookie()])
}

/// 302 with cookies — the callback's only success/failure shape.
fn redirect(location: &str, cookies: &[String]) -> Response {
    let mut res = (StatusCode::FOUND, [(header::LOCATION, location)]).into_response();
    for c in cookies {
        if let Ok(v) = axum::http::HeaderValue::from_str(c) {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    res
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let sb = state.secretbox().await.unwrap_or_default();
    if !google_login_enabled(&state.pg, &sb).await {
        return login_error("google_disabled", &[]);
    }

    let qp = query_pairs(uri.query());
    let code = qp.get("code");
    let state_param = qp.get("state");
    let cookie_state = parse_cookies(&headers).and_then(|c| c.get(STATE_COOKIE).cloned());

    // A non-empty error param from Google = the human said no.
    if qp.get("error").is_some_and(|e| !e.is_empty()) {
        return login_error("google_denied", &[]);
    }
    let (Some(code), Some(state_param)) = (code, state_param) else {
        return login_error("bad_state", &[]);
    };
    if cookie_state
        .as_deref()
        .is_none_or(|c| !state_matches(state_param, c))
    {
        return login_error("bad_state", &[]);
    }

    let cfg = get_auth_config();
    let redirect_uri = google_redirect_uri(cfg.public_url.as_deref(), &headers, &uri);
    let identity = match exchange_google_code(&state.pg, &sb, code, &redirect_uri).await {
        Ok(i) => i,
        Err(e) => {
            // The error names the client or the code; the user sees a fixed
            // sentence and the log keeps the detail.
            tracing::error!("[auth/google] callback failed: {e}");
            return login_error("exchange_failed", &[]);
        }
    };

    // An unclaimed instance (zero admins) hands the FIRST Google identity the
    // keys — no doors apply, by design: nobody exists yet to have invited
    // them. A lost race (someone claimed between the check and the lock) falls
    // through to the normal doors for this sign-in. The org-domain gate below
    // cannot bite here in practice — org connect is admin-only, so an
    // unclaimed instance has no connection — but the claim precedes it anyway.
    match instance_claimable(&state.pg).await {
        Ok(true) => match claim_admin(&state.pg, &identity, None).await {
            Ok(Some(claimed)) => {
                let (cid, role) = (claimed.0.clone(), claimed.5.clone());
                let pg = state.pg.clone();
                let actor = identity.email.clone().unwrap_or_else(|| cid.clone());
                let after_email = identity.email.clone();
                tokio::spawn(async move {
                    log_audit(
                        &pg,
                        AuditEntry {
                            actor: &actor,
                            action: "auth.claim",
                            target_type: "user",
                            target_id: Some(&cid),
                            target_label: None,
                            before: None,
                            after: Some(serde_json::json!({
                                "email": after_email,
                                "role": role,
                                "provider": PROVIDER,
                            })),
                        },
                    )
                    .await;
                });
                return finish_login(&state, &claimed, PROVIDER).await;
            }
            Ok(None) => {} // race lost — fall through to the doors
            Err(e) => return internal(&e),
        },
        Ok(false) => {}
        Err(e) => return internal(&e),
    }

    // The org account's domain is the outer gate. Once a Talaria is wired to
    // a Google Workspace, Google sign-in is for that workspace's people —
    // checked BEFORE the doors below because "only this org" is a property of
    // the install, not a membership policy an invite can override. The
    // connected flag (a live refresh token) is part of the anchor: a dead
    // leftover row must not lock every human out.
    let org_email = match org_login_email(&state.pg).await {
        Ok(e) => e,
        Err(e) => return internal(&e),
    };
    if !org_login_allowed(org_email.as_deref(), identity.email.as_deref()) {
        return login_error(
            "org_domain",
            &[(
                "domain",
                email_domain_of(org_email.as_deref())
                    .unwrap_or_default()
                    .as_str(),
            )],
        );
    }

    // Three doors in: the env allow-list, SELF-JOIN (verified email on a
    // DNS-verified org domain), or a live INVITE for this address.
    let invited = match invite_allowed(&state.pg, identity.email.as_deref()).await {
        Ok(v) => v,
        Err(e) => return internal(&e),
    };
    if !is_email_allowed(identity.email.as_deref(), &cfg) {
        let self_join = match self_join_allowed(&state.pg, identity.email.as_deref()).await {
            Ok(v) => v,
            Err(e) => return internal(&e),
        };
        if !self_join && !invited {
            return login_error("not_allowed", &[]);
        }
    }

    let row = match upsert_user(&state.pg, &identity).await {
        Ok(r) => r,
        Err(e) => return internal(&e),
    };
    if invited && identity.email.is_some() {
        // fire-and-forget — a failed stamp must not fail the sign-in (the
        // invite just stays "pending" and the next login re-stamps it)
        let pg = state.pg.clone();
        let email = identity.email.clone().unwrap();
        let uid = row.0.clone();
        tokio::spawn(async move {
            if let Err(e) = mark_invite_accepted(&pg, &email, &uid).await {
                tracing::error!("[auth/google] could not stamp invite accepted: {e}");
            }
        });
    }
    finish_login(&state, &row, PROVIDER).await
}

/// Mint the session and land on the cockpit — the callback's one success
/// shape (session cookie on, one-shot state cookie off), shared by the claim
/// and the doors paths.
async fn finish_login(
    state: &AppState,
    row: &crate::claim::ClaimedAdmin,
    provider: &str,
) -> Response {
    let user = SessionUser {
        id: row.0.clone(),
        sub: row.1.clone(),
        email: row.2.clone(),
        name: row.3.clone(),
        picture: row.4.clone(),
        role: row.5.clone(),
        provider: provider.into(),
    };
    match create_session(state, &user).await {
        Ok(sid) => redirect("/", &[session_cookie(&sid), clear_state_cookie()]),
        Err(e) => {
            tracing::error!("[auth/google] session create failed: {e}");
            thrown_internal_error()
        }
    }
}

fn internal(e: &sqlx::Error) -> Response {
    tracing::error!("[auth/google] database read failed: {e}");
    thrown_internal_error()
}
