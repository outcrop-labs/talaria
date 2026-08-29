// Sessions — port of ui/src/server/auth/session.ts. Redis-backed, cookie-carried:
// the cookie holds only an opaque id; the user record lives under `sess:<sid>`
// with a TTL. Both runtimes share the store during the port, so a session
// created by TS reads back here field-for-field and vice versa — the JSON shape
// below is the contract, and field ORDER is part of it (TS createSession stores
// `{...upsertUserRow, provider}` — id, sub, email, name, picture, role,
// provider — and V8 re-emits parsed JSON in stored order; serde re-emits in
// struct order, so the struct is declared in the stored order).

use crate::state::AppState;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

pub const SESSION_COOKIE: &str = "talaria_session";
pub const STATE_COOKIE: &str = "talaria_oauth_state";

const SESSION_TTL_SECONDS: u64 = 60 * 60 * 24 * 7; // 7 days

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUser {
    pub id: String,
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
    // 'admin' | 'member' — a bare string, not an enum: the value comes from
    // the DB via TS and round-trips through Redis JSON untouched.
    pub role: String,
    // 'google' | 'password'
    pub provider: String,
}

/// The user object the auth routes put in their JSON bodies
/// (`{ok: true, user}` on login/claim) — the session user minus `provider`,
/// in the TS select order. Login spreads `provider` into the SESSION record
/// only; the response user is the bare row.
#[derive(Debug, Serialize)]
pub struct WireUser {
    pub id: String,
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
    pub role: String,
}

impl From<&SessionUser> for WireUser {
    fn from(u: &SessionUser) -> Self {
        WireUser {
            id: u.id.clone(),
            sub: u.sub.clone(),
            email: u.email.clone(),
            name: u.name.clone(),
            picture: u.picture.clone(),
            role: u.role.clone(),
        }
    }
}

fn key(sid: &str) -> String {
    format!("sess:{sid}")
}

/// Create a session in Redis; returns the opaque id for the cookie.
pub async fn create_session(
    state: &AppState,
    user: &SessionUser,
) -> Result<String, redis::RedisError> {
    let sid = random_token();
    let mut conn = state.redis().await?;
    redis::cmd("SET")
        .arg(key(&sid))
        .arg(serde_json::to_string(user).expect("SessionUser serializes"))
        .arg("EX")
        .arg(SESSION_TTL_SECONDS)
        .query_async::<()>(&mut conn)
        .await?;
    Ok(sid)
}

/// Resolve the request's session cookie to its user, or None.
pub async fn get_session_user(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<SessionUser>, redis::RedisError> {
    let Some(sid) = parse_cookies(headers).and_then(|c| c.get(SESSION_COOKIE).cloned()) else {
        return Ok(None);
    };
    let mut conn = state.redis().await?;
    let raw: Option<String> = redis::cmd("GET")
        .arg(key(&sid))
        .query_async(&mut conn)
        .await?;
    // A value that fails to parse is a missing session (TS JSON.parse → catch
    // → null), never a 500.
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

/// Merge changes into the live session (a display-name edit), keeping the
/// existing TTL (SET KEEPTTL).
pub async fn update_session_user(
    state: &AppState,
    headers: &HeaderMap,
    patch: &serde_json::Value,
) -> Result<Option<SessionUser>, redis::RedisError> {
    let Some(sid) = parse_cookies(headers).and_then(|c| c.get(SESSION_COOKIE).cloned()) else {
        return Ok(None);
    };
    let mut conn = state.redis().await?;
    let raw: Option<String> = redis::cmd("GET")
        .arg(key(&sid))
        .query_async(&mut conn)
        .await?;
    let Some(raw) = raw else { return Ok(None) };
    let mut next: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| invalid("session json"))?;
    if let Some(a) = next.as_object_mut()
        && let Some(b) = patch.as_object()
    {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
    redis::cmd("SET")
        .arg(key(&sid))
        .arg(next.to_string())
        .arg("KEEPTTL")
        .query_async::<()>(&mut conn)
        .await?;
    Ok(Some(
        serde_json::from_value(next).map_err(|_| invalid("session json"))?,
    ))
}

/// Patch EVERY live session belonging to a user (an admin role change must not
/// wait for re-login). SCAN-based — session counts are tiny here. Returns how
/// many sessions were patched.
pub async fn update_sessions_for_user(
    state: &AppState,
    user_id: &str,
    patch: &serde_json::Value,
) -> Result<u64, redis::RedisError> {
    let mut conn = state.redis().await?;
    let mut cursor: u64 = 0;
    let mut updated: u64 = 0;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .cursor_arg(cursor)
            .arg("MATCH")
            .arg("sess:*")
            .arg("COUNT")
            .arg(100)
            .query_async(&mut conn)
            .await?;
        for k in keys {
            // MATCH alone is not a guarantee — verify the shape before writing.
            let Some(raw): Option<String> =
                redis::cmd("GET").arg(&k).query_async(&mut conn).await?
            else {
                continue;
            };
            let Ok(mut u) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue; // skip malformed
            };
            if u.get("id").and_then(|v| v.as_str()) == Some(user_id) {
                if let Some(obj) = u.as_object_mut()
                    && let Some(p) = patch.as_object()
                {
                    for (pk, pv) in p {
                        obj.insert(pk.clone(), pv.clone());
                    }
                }
                redis::cmd("SET")
                    .arg(&k)
                    .arg(u.to_string())
                    .arg("KEEPTTL")
                    .query_async::<()>(&mut conn)
                    .await?;
                updated += 1;
            }
        }
        if next == 0 {
            return Ok(updated);
        }
        cursor = next;
    }
}

/// Delete the session behind the request's cookie (logout).
pub async fn destroy_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), redis::RedisError> {
    if let Some(sid) = parse_cookies(headers).and_then(|c| c.get(SESSION_COOKIE).cloned()) {
        let mut conn = state.redis().await?;
        redis::cmd("DEL")
            .arg(key(&sid))
            .query_async::<()>(&mut conn)
            .await?;
    }
    Ok(())
}

fn invalid(what: &'static str) -> redis::RedisError {
    // UnexpectedReturnType is redis-rs's kind for "the value wasn't the shape
    // the caller needed" — the closest fit for an unparseable session body.
    redis::RedisError::from((redis::ErrorKind::UnexpectedReturnType, what))
}

// ── Cookie helpers ───────────────────────────────────────────────────────────

/// Parse the request's Cookie header into name → value (last wins, like the
/// TS object assignment). One decode failure skips that cookie — TS's
/// decodeURIComponent would THROW and 500 the whole route over one malformed
/// value; a skipped cookie degrades to "not present" instead, which is what
/// the attacker-crafted case deserves anyway.
pub fn parse_cookies(headers: &HeaderMap) -> Option<std::collections::HashMap<String, String>> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    let mut out = std::collections::HashMap::new();
    for part in header.split(';') {
        let Some(idx) = part.find('=') else { continue };
        let k = part[..idx].trim();
        if k.is_empty() {
            continue;
        }
        if let Some(v) = decode_uri_component(part[idx + 1..].trim()) {
            out.insert(k.to_string(), v);
        }
    }
    Some(out)
}

/// decodeURIComponent, minus the throw: percent-decoded UTF-8, `+` untouched.
/// None when the decodes don't land in valid UTF-8.
fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = (hex[0] as char).to_digit(16)?;
            let lo = (hex[1] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// encodeURIComponent: everything but the unreserved set escapes, as %XX of
/// the UTF-8 bytes.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn cookie_string(name: &str, value: &str, max_age: u64) -> String {
    // Secure by default in production, unless COOKIE_SECURE opts out (browsers
    // drop Secure cookies over plain http://, which breaks LAN deployments).
    let override_ = std::env::var("COOKIE_SECURE")
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let insecure = matches!(override_.as_str(), "0" | "false" | "no");
    let node_env = std::env::var("NODE_ENV").ok();
    cookie_string_parts(name, value, max_age, insecure, node_env.as_deref())
}

/// The pure core of the cookie format, so the format itself is testable
/// without racing other tests over the process environment.
fn cookie_string_parts(
    name: &str,
    value: &str,
    max_age: u64,
    insecure_optout: bool,
    node_env: Option<&str>,
) -> String {
    let secure = if !insecure_optout && node_env == Some("production") {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{name}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        encode_uri_component(value)
    )
}

pub fn session_cookie(sid: &str) -> String {
    cookie_string(SESSION_COOKIE, sid, SESSION_TTL_SECONDS)
}

pub fn clear_session_cookie() -> String {
    cookie_string(SESSION_COOKIE, "", 0)
}

pub fn state_cookie(value: &str) -> String {
    cookie_string(STATE_COOKIE, value, 600) // 10 min
}

pub fn clear_state_cookie() -> String {
    cookie_string(STATE_COOKIE, "", 0)
}

/// Constant-time compare for the OAuth state cookie — no early exit on the
/// first differing byte. (Length first; that leaks only the length, which the
/// attacker already carried here.)
pub fn state_matches(a: &str, b: &str) -> bool {
    constant_time_eq(a.as_bytes(), b.as_bytes())
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Random URL-safe token for session ids / OAuth state (base64url, no pad —
/// Node's randomBytes().toString('base64url') shape).
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("system rng");
    URL_SAFE_NO_PAD.encode(buf)
}

/// The house 401 — requireUser's exact body.
pub fn unauthorized() -> Response {
    crate::error::house_error(StatusCode::UNAUTHORIZED, "unauthorized")
}

/// Signed-in user or the 401 Response (the api-guard contract: return the
/// gate when it is a Response).
pub async fn require_user(state: &AppState, headers: &HeaderMap) -> Result<SessionUser, Response> {
    match get_session_user(state, headers).await {
        Ok(Some(user)) => Ok(user),
        Ok(None) => Err(unauthorized()),
        Err(e) => {
            tracing::error!("[session] redis read failed: {e}");
            Err(crate::error::house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            ))
        }
    }
}

/// Admin or 401/403 (api-guard.ts requireAdmin).
pub async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<SessionUser, Response> {
    let user = require_user(state, headers).await?;
    if user.role != "admin" {
        return Err(crate::error::house_error(
            StatusCode::FORBIDDEN,
            "forbidden",
        ));
    }
    Ok(user)
}

/// Signed-in user whose view is NOT DENIED (api-guard.ts requireView).
/// Denial-based, so a member sees a view by default and only a deniedViews
/// entry (or a prefix of one — 'x' denies 'x/anything') takes it away;
/// admins are exempt. The same resolution the nav and route gates use.
pub async fn require_view(
    state: &AppState,
    headers: &HeaderMap,
    view: &str,
) -> Result<SessionUser, Response> {
    let user = require_user(state, headers).await?;
    if user.role != "admin" {
        let denied = crate::users::denied_views(&state.pg, &user.id, &user.role)
            .await
            .map_err(|e| {
                tracing::error!("[session] view-denial read failed: {e}");
                crate::error::house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            })?;
        if denied
            .iter()
            .any(|v| v == view || view.starts_with(&format!("{v}/")))
        {
            return Err(crate::error::house_error(
                StatusCode::FORBIDDEN,
                "forbidden",
            ));
        }
    }
    Ok(user)
}

/// Route gate: session + permission in one call (api-guard.ts requirePerm,
/// via permissions.ts). Returns the user, or a ready-to-return 401/403.
pub async fn require_perm(
    state: &AppState,
    headers: &HeaderMap,
    perm: &str,
) -> Result<SessionUser, Response> {
    let user = require_user(state, headers).await?;
    if !crate::users::has_perm(&state.pg, &user.id, &user.role, perm)
        .await
        .map_err(|e| {
            tracing::error!("[session] permission read failed: {e}");
            crate::error::house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        })?
    {
        return Err(crate::error::house_error(
            StatusCode::FORBIDDEN,
            &format!("you don't have permission to do that ({perm})"),
        ));
    }
    Ok(user)
}

/// Attach a Set-Cookie (or several) to a JSON body — the login/logout shape.
pub fn json_with_cookies(body: impl IntoResponse, cookies: &[String]) -> Response {
    let mut res = body.into_response();
    let headers = res.headers_mut();
    for c in cookies {
        if let Ok(v) = axum::http::HeaderValue::from_str(c) {
            headers.append(header::SET_COOKIE, v);
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_component_round_trips_the_js_way() {
        // The unreserved set survives; everything else escapes as %XX.
        assert_eq!(encode_uri_component("aZ9-_.!~*'()"), "aZ9-_.!~*'()");
        assert_eq!(encode_uri_component("a b/c"), "a%20b%2Fc");
        assert_eq!(encode_uri_component("ü"), "%C3%BC");
        // decode is its inverse, and leaves '+' alone (decodeURIComponent,
        // not decodeURIComponent-minus-query-semantics)
        assert_eq!(decode_uri_component("a%20b%2Fc").as_deref(), Some("a b/c"));
        assert_eq!(decode_uri_component("a+b").as_deref(), Some("a+b"));
        assert_eq!(decode_uri_component("%C3%BC").as_deref(), Some("ü"));
        // Invalid escapes: JS throws; we decline.
        assert_eq!(decode_uri_component("100%"), None);
        assert_eq!(decode_uri_component("%ZZ"), None);
        // Broken UTF-8 after decode: JS throws; we decline.
        assert_eq!(decode_uri_component("%FF"), None);
    }

    #[test]
    fn cookies_parse_like_the_ts_split() {
        let headers = |val: &str| {
            let mut h = HeaderMap::new();
            h.insert(
                header::COOKIE,
                axum::http::HeaderValue::from_str(val).unwrap(),
            );
            h
        };
        let c =
            parse_cookies(&headers("a=1; talaria_session=abc%2Fdef; b=x=y; ; nosplit")).unwrap();
        assert_eq!(c.get("a").map(String::as_str), Some("1"));
        assert_eq!(
            c.get("talaria_session").map(String::as_str),
            Some("abc/def")
        );
        assert_eq!(c.get("b").map(String::as_str), Some("x=y")); // first '=' splits
        assert!(!c.contains_key("nosplit"));
        assert!(parse_cookies(&HeaderMap::new()).is_none());
    }

    #[test]
    fn cookie_strings_match_the_ts_format() {
        // The pure core, so the format test doesn't race the env.
        let c =
            |name: &str, value: &str, age: u64| cookie_string_parts(name, value, age, false, None);
        assert_eq!(
            c("talaria_session", "abc/def", 604800),
            "talaria_session=abc%2Fdef; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800"
        );
        assert_eq!(
            c("talaria_oauth_state", "", 0),
            "talaria_oauth_state=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
        );
        // Production adds Secure; the COOKIE_SECURE opt-outs (any casing) remove it.
        let prod = |optout: bool| cookie_string_parts("s", "v", 1, optout, Some("production"));
        assert!(prod(false).ends_with("; Secure"));
        assert!(!prod(true).ends_with("; Secure"));
        // Outside production there is never a Secure suffix.
        assert!(!cookie_string_parts("s", "v", 1, false, None).contains("Secure"));
    }

    #[test]
    fn session_user_serializes_in_stored_order() {
        let u = SessionUser {
            id: "1".into(),
            sub: "google:2".into(),
            email: Some("a@b.c".into()),
            name: None,
            picture: None,
            role: "member".into(),
            provider: "google".into(),
        };
        let j = serde_json::to_string(&u).unwrap();
        // role BEFORE provider — the order TS's `{...user, provider}` spread
        // stores, and therefore the order the SPA's parsed object carries.
        assert!(j.starts_with(r#"{"id":"1","sub":"google:2","email":"a@b.c","name":null,"picture":null,"role":"member","provider":"google"}"#));
    }

    #[test]
    fn tokens_are_43_chars_of_base64url() {
        let t = random_token();
        assert_eq!(t.len(), 43); // 32 bytes → ceil(32/3)*4 without padding
        assert!(
            t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(t, random_token());
    }
}
