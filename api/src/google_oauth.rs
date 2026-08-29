// Google LOGIN — port of the policy half (ui/src/server/auth/google.ts) and
// the OAuth mechanics it leans on (ui/src/server/google/oauth.ts, the leaf
// both flows build on). Tokens hand-rolled over TLS, no SDK — the profile
// comes from Google's userinfo endpoint, never a locally-verified JWT. The
// CONNECT flow (per-user/org workspace tokens, batch 5) shares the endpoints
// and exchange shape defined here.
//
// The endpoints, in full:
pub const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const USERINFO_ENDPOINT: &str = "https://openidconnect.googleapis.com/v1/userinfo";

use crate::gateway::provider::http;
use crate::google_client::{GoogleClient, resolve_google_client};
use crate::secretbox::SecretBox;
use crate::users::Identity;
use axum::http::{HeaderMap, Uri, header};
use sqlx::PgPool;
use std::collections::HashMap;

/// The login identity's provider tag.
pub const PROVIDER: &str = "google";

/// External origin for redirect URIs (oauth.ts resolveOrigin):
/// AUTH_PUBLIC_URL, else the forwarded proto/host a proxy stated, else the
/// request's own scheme/host. The coexistence proxy forwards the caller's
/// origin, so proxied and direct requests derive the same URI.
pub fn resolve_origin(cfg_public_url: Option<&str>, headers: &HeaderMap, uri: &Uri) -> String {
    if let Some(configured) = cfg_public_url.filter(|s| !s.is_empty()) {
        return configured.to_string();
    }
    let hdr = |name: &'static str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let proto = hdr("x-forwarded-proto")
        .map(String::from)
        .or_else(|| uri.scheme().map(|s| s.to_string()))
        .unwrap_or_else(|| "http".into());
    let host = hdr("x-forwarded-host")
        .or_else(|| hdr("host"))
        .map(String::from)
        .or_else(|| {
            uri.host().map(|h| {
                // Re-attach the port when the URI carried one.
                match uri.port_u16() {
                    Some(p) => format!("{h}:{p}"),
                    None => h.to_string(),
                }
            })
        })
        .unwrap_or_else(|| "127.0.0.1".into());
    format!("{proto}://{host}")
}

/// The login flow's redirect URI (auth/google.ts googleRedirectUri).
pub fn google_redirect_uri(cfg_public_url: Option<&str>, headers: &HeaderMap, uri: &Uri) -> String {
    format!(
        "{}/api/auth/google/callback",
        resolve_origin(cfg_public_url, headers, uri)
    )
}

/// application/x-www-form-urlencoded serialization with URLSearchParams's
/// exact byte behavior (WHATWG url spec: space → '+', unreserved passthrough)
/// — what TS's `new URLSearchParams(...).toString()` produces, parameter
/// order included, so the consent URL matches byte-for-byte.
fn form_serialize(pairs: &[(&str, &str)]) -> String {
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        out.append_pair(k, v);
    }
    out.finish()
}

/// The consent URL for the login flow (auth/google.ts googleAuthUrl):
/// access_type=online (proves who you are; nothing is stored), account
/// chooser forced, the Workspace hd restriction as a HINT (the guarantee is
/// re-checked on the resolved identity at exchange time).
pub fn google_auth_url(cfg: &GoogleClient, redirect_uri: &str, state: &str) -> String {
    let mut pairs: Vec<(&str, &str)> = vec![
        ("client_id", cfg.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", "openid email profile"),
        ("state", state),
        ("access_type", "online"),
        ("prompt", "select_account"),
    ];
    if let Some(hd) = cfg.hd.as_deref().filter(|h| !h.is_empty()) {
        pairs.push(("hd", hd));
    }
    format!("{AUTH_ENDPOINT}?{}", form_serialize(&pairs))
}

/// The userinfo shape the login flow reads (oauth.ts GoogleUserInfo — optionals
/// exactly as Google may omit them).
struct GoogleUserInfo {
    sub: String,
    email: Option<String>,
    email_verified: bool,
    name: Option<String>,
    picture: Option<String>,
    hd: Option<String>,
}

/// Exchange the auth code for tokens + the Google identity (oauth.ts
/// exchangeGoogleTokens, then auth/google.ts's shaping). Errors are strings —
/// the callback maps every one to the same `exchange_failed` bounce, so the
/// only place the detail is allowed to live is the log.
pub async fn exchange_google_code(
    pg: &PgPool,
    sb: &SecretBox,
    code: &str,
    redirect_uri: &str,
) -> Result<Identity, String> {
    let cfg = resolve_google_client(pg, sb)
        .await
        .ok_or_else(|| "no google client configured".to_string())?;

    let token_body = form_serialize(&[
        ("code", code),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
    ]);
    let token_res = http()
        .post(TOKEN_ENDPOINT)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(token_body)
        .send()
        .await
        .map_err(|e| format!("token request: {e}"))?;
    if !token_res.status().is_success() {
        let status = token_res.status();
        // The body names the client or the code; both die at this boundary
        // (the log keeps ≤500 chars of it, the caller sees a fixed sentence).
        let text = token_res.text().await.unwrap_or_default();
        tracing::error!(
            "[auth/google] token exchange failed: {status} {}",
            truncate(&text)
        );
        return Err(format!("token exchange failed: {status}"));
    }
    let tokens: serde_json::Value = serde_json::from_str(
        &token_res
            .text()
            .await
            .map_err(|e| format!("token body: {e}"))?,
    )
    .map_err(|e| format!("token body not json: {e}"))?;
    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "token exchange: no access_token".to_string())?
        .to_string();

    let info_res = http()
        .get(USERINFO_ENDPOINT)
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| format!("userinfo request: {e}"))?;
    if !info_res.status().is_success() {
        return Err(format!("userinfo failed: {}", info_res.status()));
    }
    let raw: serde_json::Value = serde_json::from_str(
        &info_res
            .text()
            .await
            .map_err(|e| format!("userinfo body: {e}"))?,
    )
    .map_err(|e| format!("userinfo not json: {e}"))?;
    let str_field = |key: &str| {
        raw.get(key)
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty())
    };
    let info = GoogleUserInfo {
        sub: str_field("sub").ok_or_else(|| "userinfo: no sub".to_string())?,
        email: str_field("email"),
        email_verified: raw.get("email_verified").and_then(|v| v.as_bool()) == Some(true),
        name: str_field("name"),
        picture: str_field("picture"),
        hd: str_field("hd"),
    };

    // An email claim is only an identity when Google has verified it. Google's
    // userinfo always carries the claim alongside the email, so an absent
    // claim is treated as unverified too — an IdP (or client config) handing
    // back unverified addresses must not mint accounts on them (#269).
    if let Some(email) = info.email.as_deref().filter(|_| !info.email_verified) {
        return Err(format!("google email not verified: {email}"));
    }
    // Enforce the Workspace hosted-domain restriction on the resolved identity
    // too (the `hd` auth param is a hint, not a guarantee).
    if let Some(hd) = cfg.hd.as_deref().filter(|h| !h.is_empty()) {
        let in_domain = info.hd.as_deref() == Some(hd);
        if !in_domain {
            return Err(format!("google account not in required domain {hd}"));
        }
    }

    // `info.name ?? info.email ?? null` — take the fallback before email moves.
    let name = info.name.clone().or_else(|| info.email.clone());
    Ok(Identity {
        sub: format!("{}:{}", PROVIDER, info.sub),
        email: info.email,
        name,
        picture: info.picture,
    })
}

/// The domain of an email address, lowercased (aliasing.ts emailDomainOf):
/// null when there is no usable one (no @, nothing before it, nothing after
/// it, null input). lastIndexOf — a plus-addressed org account gates on its
/// REAL domain.
pub fn email_domain_of(email: Option<&str>) -> Option<String> {
    let email = email?;
    let at = email.rfind('@')?;
    if at < 1 || at == email.len() - 1 {
        return None;
    }
    Some(email[at + 1..].trim().to_lowercase())
}

/// May this Google identity sign in, given the connected org account's email?
/// A Talaria wired to a Google Workspace is FOR that workspace's people: once
/// the org account is connected, Google sign-in is domain-members-only — even
/// an invited outside address is refused (invite them a password instead).
/// No org connection gates nothing: a fresh install's logins are governed by
/// the allow-list/invite doors alone, as before.
pub fn org_login_allowed(org_email: Option<&str>, login_email: Option<&str>) -> bool {
    let Some(org_email) = org_email else {
        return true;
    };
    let Some(domain) = email_domain_of(Some(org_email)) else {
        return true; // a connected row with no usable email cannot gate anything
    };
    email_domain_of(login_email).as_deref() == Some(domain.as_str())
}

/// The connected org account's email, or None when there is no live (refresh-
/// token-carrying) connection — org-connection.ts getOrgEmail, the read the
/// login gate needs. The full connection status (targets, scopes) is batch 5.
pub async fn org_login_email(pg: &PgPool) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("select email, refresh_token_enc from google_org_connection where id = 1")
            .fetch_optional(pg)
            .await?;
    Ok(match row {
        // Truthy refresh = connected; a dead leftover row gates nothing.
        Some((email, Some(refresh))) if !refresh.is_empty() => email,
        _ => None,
    })
}

/// Parse a query string with URLSearchParams's semantics (percent-decode,
/// '+' → space). Later duplicates win, like the TS `.get()`.
pub fn query_pairs(query: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(q) = query {
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            out.insert(k.into_owned(), v.into_owned());
        }
    }
    out
}

fn truncate(s: &str) -> &str {
    match s.char_indices().nth(500) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_url_matches_the_ts_serialization() {
        // Order and encoding exactly as URLSearchParams.toString() in
        // auth/google.ts: spaces as '+', hd appended only when set.
        let mut cfg = GoogleClient {
            client_id: "cid-1".into(),
            client_secret: "sec".into(),
            hd: None,
            source: "env",
        };
        assert_eq!(
            google_auth_url(
                &cfg,
                "http://127.0.0.1:5273/api/auth/google/callback",
                "st%ate+ x"
            ),
            "https://accounts.google.com/o/oauth2/v2/auth?client_id=cid-1&redirect_uri=http%3A%2F%2F127.0.0.1%3A5273%2Fapi%2Fauth%2Fgoogle%2Fcallback&response_type=code&scope=openid+email+profile&state=st%25ate%2B+x&access_type=online&prompt=select_account"
        );
        cfg.hd = Some("getboxie.com".into());
        assert!(
            google_auth_url(&cfg, "http://x/cb", "s")
                .ends_with("&prompt=select_account&hd=getboxie.com")
        );
    }

    #[test]
    fn domain_rules_are_aliasings() {
        assert_eq!(
            email_domain_of(Some("Jon@GetBoxie.COM")),
            Some("getboxie.com".into())
        );
        assert_eq!(
            email_domain_of(Some("org+triage@x.io")),
            Some("x.io".into())
        ); // last @
        assert_eq!(email_domain_of(Some("@x.io")), None);
        assert_eq!(email_domain_of(Some("a@")), None);
        assert_eq!(email_domain_of(Some("plain")), None);
        assert_eq!(email_domain_of(None), None);
    }

    #[test]
    fn org_gate_admits_only_the_workspaces_domain() {
        assert!(org_login_allowed(None, None)); // no connection gates nothing
        assert!(org_login_allowed(None, Some("anyone@anywhere.io")));
        assert!(org_login_allowed(
            Some("org@getboxie.com"),
            Some("jon@getboxie.com")
        ));
        assert!(!org_login_allowed(
            Some("org@getboxie.com"),
            Some("jon@gmail.com")
        ));
        assert!(!org_login_allowed(Some("org@getboxie.com"), None));
        // A connected row with no usable email cannot gate anything.
        assert!(org_login_allowed(Some("not-an-email"), Some("x@y.io")));
    }

    #[test]
    fn query_pairs_decode_like_urlsearchparams() {
        let q = query_pairs(Some("code=a%2Bb&state=x+y&error"));
        assert_eq!(q.get("code").map(String::as_str), Some("a+b"));
        assert_eq!(q.get("state").map(String::as_str), Some("x y"));
        assert_eq!(q.get("error").map(String::as_str), Some(""));
        assert!(query_pairs(None).is_empty());
    }
}
