// OAuth 2.1 for remote MCP servers — port of ui/src/server/mcp-oauth.ts. The
// auth most first-party servers (Linear, Stripe, Vercel, GitHub) actually
// speak: no header to paste, the flow is negotiated per the MCP authorization
// spec — 401 + WWW-Authenticate → protected-resource metadata → authorization
// server metadata → DYNAMIC CLIENT REGISTRATION → authorization-code + PKCE →
// sealed token store → silent refresh. Tokens are held per SUBJECT: 'org'
// (one shared connection) or a user id (per-user connected accounts). The
// gateway injects the right bearer at call time; nothing OAuth-shaped ever
// reaches an agent config.
//
// SSRF note — the sharpest edge in the codebase, because almost every URL it
// fetches is chosen by the SERVER BEING TALKED TO, not by us: the
// resource-metadata URL arrives in the upstream's own WWW-Authenticate header,
// the authorization server comes out of that document, and the
// registration/token endpoints come out of the AS metadata. The token POST
// then carries our client_secret. Two defences, both required and both
// carried from TS verbatim:
//   1. every request goes through safe_fetch (no loopback/link-local/private
//      targets, and each redirect hop re-validated), and
//   2. every discovered URL is PINNED to the registrable domain of the server
//      URL an admin configured — so a server can send us to its own auth
//      infrastructure and nowhere else.
//
// The config lives in mcp_servers.oauth as the raw jsonb Value: pg owns its
// key order (canonical on write), both runtimes must read and re-store the
// same row, and a typed struct re-serializing in declaration order would
// diverge from the TS spread for nothing.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{Map, Value};
use sqlx::PgPool;

use crate::mcp_registry::MCP_PROTOCOL_VERSION;
use crate::safe_fetch::{SafeFetch, safe_fetch};
use crate::secretbox::SecretBox;

// ── Domain pinning ──────────────────────────────────────────────────────────

// An approximation of the public suffix list: enough multi-label suffixes
// that "co.uk" doesn't collapse two unrelated companies into one domain.
// Shipping the real PSL for this would be a dependency and a refresh problem;
// the cost of the approximation is that an exotic suffix pins one label too
// loosely.
const MULTI_LABEL_SUFFIXES: [&str; 20] = [
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "com.au", "net.au", "org.au", "co.nz",
    "co.jp", "co.kr", "co.za", "com.br", "com.mx", "com.sg", "com.hk", "com.tr", "co.in",
    "com.cn", "com.tw",
];

fn registrable_domain(host: &str) -> String {
    let lowered = host.to_lowercase();
    let labels: Vec<&str> = lowered.trim_end_matches('.').split('.').collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let last2 = labels[labels.len() - 2..].join(".");
    if MULTI_LABEL_SUFFIXES.contains(&last2.as_str()) {
        labels[labels.len() - 3..].join(".")
    } else {
        last2
    }
}

// Providers that legitimately authorize on a different registrable domain than
// they serve MCP from — GitHub's official server is api.githubcopilot.com but
// its OAuth issuer is github.com. Additions belong in
// TALARIA_MCP_OAUTH_TRUSTED_DOMAINS (comma-separated), which an operator sets
// deliberately, not in this map.
const CROSS_DOMAIN_ISSUERS: [(&str, &[&str]); 1] = [("githubcopilot.com", &["github.com"])];

/// A checker that accepts only URLs on the configured server's own registrable
/// domain (plus documented/operator-trusted exceptions). Everything discovered
/// from an untrusted response passes through this before it is fetched,
/// stored, or handed a client_secret.
struct Pinned {
    allowed: HashSet<String>,
    base: String,
}

impl Pinned {
    /// TS's `check(url, what)` — Err carries the same sentence the route
    /// answers with.
    fn check(&self, url: &str, what: &str) -> Result<String, String> {
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return Err(format!("{what} is not a valid URL"));
        };
        let host = parsed.host_str().unwrap_or_default().to_string();
        if !self.allowed.contains(&registrable_domain(&host)) {
            return Err(format!(
                "{what} points at {host}, which is outside this server's own domain ({}), so it is refused",
                self.base
            ));
        }
        Ok(url.to_string())
    }
}

fn pin_to(server_url: &str) -> Result<Pinned, String> {
    let host = reqwest::Url::parse(server_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .ok_or_else(|| "the server URL is not a valid URL".to_string())?;
    let base = registrable_domain(&host);
    let mut allowed = HashSet::from([base.clone()]);
    if let Some((_, extra)) = CROSS_DOMAIN_ISSUERS.iter().find(|(d, _)| *d == base) {
        allowed.extend(extra.iter().map(|d| d.to_string()));
    }
    for d in std::env::var("TALARIA_MCP_OAUTH_TRUSTED_DOMAINS")
        .unwrap_or_default()
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
    {
        allowed.insert(registrable_domain(d));
    }
    Ok(Pinned { allowed, base })
}

// ── Small helpers ───────────────────────────────────────────────────────────

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_b64url(n: usize) -> Result<String, String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|e| format!("key material unavailable: {e}"))?;
    Ok(b64url(&buf))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// One safe_fetch POST of a JSON body.
async fn post_json(
    url: &str,
    body: &Value,
    timeout_ms: u64,
) -> Result<crate::safe_fetch::SafeResponse, crate::safe_fetch::SafeError> {
    let payload = serde_json::to_string(body).expect("the handshake body is plain data");
    safe_fetch(
        url,
        SafeFetch {
            method: Some("POST"),
            headers: vec![
                ("content-type", "application/json"),
                ("accept", "application/json, text/event-stream"),
            ],
            body: Some(payload.as_bytes()),
            timeout_ms: Some(timeout_ms),
            ..SafeFetch::default()
        },
    )
    .await
}

// ── Discovery ───────────────────────────────────────────────────────────────

/// Probe a server unauthenticated; a 401 with resource metadata (or the
/// well-known fallback) means OAuth. Returns the resolved config, or None for
/// servers that don't advertise it. Any failure anywhere in the dance is a
/// "not OAuth" answer — discovery never throws at its caller.
pub async fn discover_oauth(server_url: &str) -> Option<Value> {
    let run = async {
        let pin = pin_to(server_url).ok()?;
        let probe = post_json(
            server_url,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "talaria", "version": "1.0" },
                },
            }),
            10_000,
        )
        .await
        .ok()?;
        if probe.status != 401 && probe.status != 403 {
            return None;
        }
        let www = probe
            .headers
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        // The server names its own metadata document. Pinned: a 401 challenge
        // is not permission to send us anywhere on the network.
        let meta_url = match regex::Regex::new(r#"resource_metadata="?([^",\s]+)"?"#)
            .expect("resource_metadata pattern")
            .captures(&www)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
        {
            Some(u) => u,
            None => reqwest::Url::parse(server_url)
                .ok()?
                .join("/.well-known/oauth-protected-resource")
                .ok()?
                .to_string(),
        };
        let meta_url = pin.check(&meta_url, "the resource metadata URL").ok()?;

        let meta_r = safe_fetch(&meta_url, SafeFetch { timeout_ms: Some(8_000), ..Default::default() })
            .await
            .ok()?;
        if !(200..300).contains(&meta_r.status) {
            return None;
        }
        let meta: Value = serde_json::from_slice(&meta_r.body).ok()?;
        let as_url = meta
            .get("authorization_servers")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)?
            .to_string();
        pin.check(&as_url, "the advertised authorization server").ok()?;

        // RFC 8414: for an issuer WITH a path (github.com/login/oauth), the
        // well-known segment goes BETWEEN host and path.
        let as_parsed = reqwest::Url::parse(&as_url).ok()?;
        let as_path = as_parsed.path().trim_end_matches('/').to_string();
        let bare = format!("{as_url}/.well-known/openid-configuration");
        let mut candidates = vec![
            format!("{}://{}{as_path}/.well-known/oauth-authorization-server", as_parsed.scheme(), as_parsed.host_str()?),
            format!("{}://{}/.well-known/oauth-authorization-server", as_parsed.scheme(), as_parsed.host_str()?),
            format!("{}://{}/.well-known/openid-configuration{as_path}", as_parsed.scheme(), as_parsed.host_str()?),
            bare,
        ];
        candidates.dedup();
        let mut as_meta: Option<Value> = None;
        for c in &candidates {
            let Ok(r) = safe_fetch(c, SafeFetch { timeout_ms: Some(8_000), ..Default::default() }).await
            else {
                continue;
            };
            if !(200..300).contains(&r.status) {
                continue;
            }
            let Ok(j) = serde_json::from_slice::<Value>(&r.body) else {
                continue;
            };
            if j.get("authorization_endpoint").is_some() {
                as_meta = Some(j);
                break;
            }
        }
        let as_meta = as_meta?;
        if str_field(&as_meta, "authorization_endpoint").is_none()
            || str_field(&as_meta, "token_endpoint").is_none()
        {
            return None;
        }
        // The endpoints that matter most: the browser is sent to one of them
        // and the client_secret is POSTed to another. Both pinned to the
        // server's domain.
        let scopes = meta
            .get("scopes_supported")
            .cloned()
            .filter(|v| v.is_array())
            .unwrap_or_else(|| {
                as_meta
                    .get("scopes_supported")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new()))
            });
        let mut cfg = Map::new();
        cfg.insert(
            "resource".into(),
            meta.get("resource").cloned().unwrap_or(Value::String(server_url.into())),
        );
        cfg.insert(
            "authorizationEndpoint".into(),
            Value::String(pin.check(&str_field(&as_meta, "authorization_endpoint")?, "the authorization endpoint").ok()?),
        );
        cfg.insert(
            "tokenEndpoint".into(),
            Value::String(pin.check(&str_field(&as_meta, "token_endpoint")?, "the token endpoint").ok()?),
        );
        cfg.insert(
            "registrationEndpoint".into(),
            match str_field(&as_meta, "registration_endpoint") {
                Some(u) => Value::String(pin.check(&u, "the client registration endpoint").ok()?),
                None => Value::Null,
            },
        );
        cfg.insert("scopes".into(), scopes);
        cfg.insert(
            "documentation".into(),
            match str_field(&as_meta, "service_documentation") {
                Some(d) => Value::String(d),
                None => Value::Null,
            },
        );
        Some(Value::Object(cfg))
    };
    run.await
}

/// Read the stored config, or None when the row has none.
async fn stored_oauth(pg: &PgPool, server_id: &str) -> Result<Option<Value>, sqlx::Error> {
    let row: Option<(Option<Value>,)> =
        sqlx::query_as("select oauth from mcp_servers where id::text = $1")
            .bind(server_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.and_then(|(v,)| v).filter(|v| v.is_object()))
}

async fn store_oauth(pg: &PgPool, server_id: &str, cfg: &Value) -> Result<(), sqlx::Error> {
    sqlx::query("update mcp_servers set oauth = $2, updated_at = now() where id::text = $1")
        .bind(server_id)
        .bind(cfg)
        .execute(pg)
        .await?;
    Ok(())
}

/// Detect + persist a server's OAuth config (idempotent; cheap no-op when
/// already discovered). Returns whether the server is OAuth-shaped.
pub async fn ensure_oauth_config(
    pg: &PgPool,
    server_id: &str,
    server_url: &str,
) -> Result<Option<Value>, String> {
    let existing = stored_oauth(pg, server_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(oauth) = existing {
        // Backfill fields added since this config was discovered (docs link),
        // preserving the registered client.
        if oauth.get("documentation").is_none()
            && let Some(fresh) = discover_oauth(server_url).await
        {
            let mut merged = fresh;
            if let Some(client) = oauth.get("client") {
                merged
                    .as_object_mut()
                    .expect("discover_oauth answers an object")
                    .insert("client".into(), client.clone());
            }
            store_oauth(pg, server_id, &merged)
                .await
                .map_err(|e| e.to_string())?;
            return Ok(Some(merged));
        }
        return Ok(Some(oauth));
    }
    if let Some(config) = discover_oauth(server_url).await {
        store_oauth(pg, server_id, &config)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(Some(config));
    }
    Ok(None)
}

// ── Client registration + the authorize/callback dance ──────────────────────

/// The registered (or manually pinned) client: id + PLAINTEXT secret. The
/// secret rides along because registration produced one (TS's shape); the
/// only consumer that POSTs it is the callback, which reads the same pair
/// back through `client_of` — start_oauth itself only ever needs the id.
struct ClientCreds {
    id: String,
    #[allow(dead_code)]
    secret: Option<String>,
}

fn client_of(cfg: &Value, sb: &SecretBox) -> Option<(String, Option<String>, String)> {
    // (id, secret, redirect_uri) — secretEnc opens here, once, for every asker.
    let client = cfg.get("client")?.as_object()?;
    let id = client.get("id")?.as_str()?.to_string();
    let secret = client
        .get("secretEnc")
        .and_then(Value::as_str)
        .and_then(|enc| sb.open(enc).ok());
    let redirect = client.get("redirectUri")?.as_str()?.to_string();
    Some((id, secret, redirect))
}

async fn ensure_client(
    pg: &PgPool,
    sb: &SecretBox,
    server_id: &str,
    server_url: &str,
    config: &Value,
    redirect_uri: &str,
) -> Result<ClientCreds, String> {
    if let Some((id, secret, stored_redirect)) = client_of(config, sb) {
        if stored_redirect == redirect_uri {
            return Ok(ClientCreds { id, secret });
        }
        // A manually-configured client (DCR-less providers like GitHub) is
        // pinned to whatever redirect the admin registered — reuse as-is.
        if str_field(config, "registrationEndpoint").is_none() {
            return Ok(ClientCreds { id, secret });
        }
    }
    let Some(registration_endpoint) = str_field(config, "registrationEndpoint") else {
        return Err(
            "this provider requires a pre-registered OAuth app. Add its client credentials on the server card."
                .into(),
        );
    };
    // Re-pinned at use time: the stored config may predate the pinning rule,
    // or the server row may have been repointed since discovery.
    let registration =
        pin_to(server_url)?.check(&registration_endpoint, "the client registration endpoint")?;
    let r = post_json(
        &registration,
        &serde_json::json!({
            "client_name": "Talaria",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }),
        10_000,
    )
    .await
    .map_err(|e| e.to_string())?;
    if !(200..300).contains(&r.status) {
        return Err(format!("client registration failed ({})", r.status));
    }
    let reg: Value = serde_json::from_slice(&r.body)
        .map_err(|_| "client registration failed (unparseable answer)".to_string())?;
    let client_id = str_field(&reg, "client_id")
        .ok_or_else(|| "client registration failed (no client id)".to_string())?;
    let client_secret = str_field(&reg, "client_secret");
    let secret_enc = client_secret
        .as_ref()
        .map(|s| sb.seal(s).map_err(|e| e.to_string()))
        .transpose()?;
    let mut next = config.clone();
    next.as_object_mut()
        .expect("the config is an object")
        .insert(
            "client".into(),
            serde_json::json!({
                "id": client_id,
                "secretEnc": secret_enc,
                "redirectUri": redirect_uri,
            }),
        );
    store_oauth(pg, server_id, &next)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ClientCreds {
        id: client_id,
        secret: client_secret,
    })
}

/// `URLSearchParams.set` semantics over an ordered pair list: replace the
/// first occurrence in place (keeping its position), drop duplicates, append
/// when absent.
fn set_param(pairs: &mut Vec<(String, String)>, key: &str, value: &str) {
    let mut kept = false;
    let mut out: Vec<(String, String)> = Vec::with_capacity(pairs.len() + 1);
    for (k, v) in pairs.drain(..) {
        if k == key {
            if !kept {
                out.push((k, value.to_string()));
                kept = true;
            }
            // every later duplicate drops
        } else {
            out.push((k, v));
        }
    }
    if !kept {
        out.push((key.to_string(), value.to_string()));
    }
    *pairs = out;
}

/// Build the authorization URL and park state+verifier for the callback.
/// `subject` is 'org' or a user id. Errors carry the route's 400 sentence.
pub async fn start_oauth(
    pg: &PgPool,
    sb: &SecretBox,
    server_id: &str,
    server_url: &str,
    subject: &str,
    origin: &str,
) -> Result<String, String> {
    let Some(config) = ensure_oauth_config(pg, server_id, server_url).await? else {
        return Err("this server doesn't advertise OAuth".into());
    };
    let mut redirect_uri = format!("{origin}/api/mcp/oauth/callback");
    if config.get("client").is_some() && str_field(&config, "registrationEndpoint").is_none() {
        redirect_uri = client_of(&config, sb)
            .map(|(_, _, r)| r)
            .unwrap_or(redirect_uri);
    }
    let client = ensure_client(pg, sb, server_id, server_url, &config, &redirect_uri).await?;
    let state = random_b64url(24)?;
    let verifier = random_b64url(48)?;
    sqlx::query("delete from mcp_oauth_states where created_at < now() - interval '30 minutes'")
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query(
        "insert into mcp_oauth_states (state, server_id, subject, verifier, redirect_uri) \
         values ($1, $2::uuid, $3, $4, $5)",
    )
    .bind(&state)
    .bind(server_id)
    .bind(subject)
    .bind(&verifier)
    .bind(&redirect_uri)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;

    let authorize = pin_to(server_url)?
        .check(&str_field(&config, "authorizationEndpoint").unwrap_or_default(), "the authorization endpoint")?;
    let mut url = reqwest::Url::parse(&authorize).map_err(|_| "the authorization endpoint is not a valid URL".to_string())?;
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let challenge = {
        use sha2::{Digest, Sha256};
        b64url(&Sha256::digest(verifier.as_bytes()))
    };
    set_param(&mut pairs, "response_type", "code");
    set_param(&mut pairs, "client_id", &client.id);
    set_param(&mut pairs, "redirect_uri", &redirect_uri);
    set_param(&mut pairs, "state", &state);
    set_param(&mut pairs, "code_challenge", &challenge);
    set_param(&mut pairs, "code_challenge_method", "S256");
    set_param(&mut pairs, "resource", &str_field(&config, "resource").unwrap_or_default());
    let scopes: Vec<String> = config
        .get("scopes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !scopes.is_empty() {
        set_param(&mut pairs, "scope", &scopes.join(" "));
    }
    let mut q = url.query_pairs_mut();
    q.clear();
    for (k, v) in &pairs {
        q.append_pair(k, v);
    }
    drop(q);
    Ok(url.to_string())
}

async fn store_tokens(
    pg: &PgPool,
    sb: &SecretBox,
    server_id: &str,
    subject: &str,
    t: &Value,
) -> Result<(), String> {
    let sealed = sb
        .seal(&t.to_string())
        .map_err(|e| format!("token seal failed: {e}"))?;
    sqlx::query(
        "insert into mcp_oauth_tokens (server_id, subject, tokens_enc) values ($1::uuid, $2, $3) \
         on conflict (server_id, subject) do update set tokens_enc = $3, updated_at = now()",
    )
    .bind(server_id)
    .bind(subject)
    .bind(&sealed)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn parse_token_response(j: &Value) -> Value {
    serde_json::json!({
        "accessToken": js_string_lossy(j.get("access_token")),
        "refreshToken": j.get("refresh_token").filter(|v| !v.is_null()).map(|v| js_string_lossy(Some(v))),
        // typeof expires_in === 'number' — a JSON number, never bool/string.
        "expiresAt": j.get("expires_in").and_then(Value::as_f64)
            .map(|s| now_ms() + (s * 1000.0) as i64).unwrap_or(0),
        "tokenType": j.get("token_type").filter(|v| !v.is_null()).map(|v| js_string_lossy(Some(v))).unwrap_or_else(|| "Bearer".into()),
    })
}

/// `String(j.x)` — null/objects stringify too; for the token POST only
/// strings ever arrive, and anything else stays a string the upstream rejects.
fn js_string_lossy(v: Option<&Value>) -> String {
    match v {
        None => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

/// The token POST body — shared by the exchange and the refresh leg.
fn token_body(pairs: Vec<(&str, String)>) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                k,
                url_enc(v)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// x-www-form-urlencoded with the JS set: space → '+', everything unreserved
/// passed through, '~' kept (URLSearchParams does not escape it).
fn url_enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn token_post(
    url: &str,
    body: String,
) -> Option<crate::safe_fetch::SafeResponse> {
    safe_fetch(
        url,
        SafeFetch {
            method: Some("POST"),
            headers: vec![("content-type", "application/x-www-form-urlencoded")],
            body: Some(body.as_bytes()),
            timeout_ms: Some(15_000),
            ..SafeFetch::default()
        },
    )
    .await
    .ok()
}

/// Exchange the authorization code. Returns where the browser should land.
pub async fn handle_oauth_callback(
    pg: &PgPool,
    sb: &SecretBox,
    state: &str,
    code: &str,
) -> Result<(String, String), String> {
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "delete from mcp_oauth_states where state = $1 \
         returning server_id::text, subject, verifier, redirect_uri",
    )
    .bind(state)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    let Some((server_id, subject, verifier, redirect_uri)) = row else {
        return Err("unknown or expired authorization state".into());
    };
    let srv: Option<(Option<Value>, String)> =
        sqlx::query_as("select oauth, url from mcp_servers where id::text = $1")
            .bind(&server_id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    let (Some(oauth), server_url) = srv.unwrap() else {
        return Err("server lost its OAuth config mid-flow".into());
    };
    let Some((client_id, client_secret, _)) = client_of(&oauth, sb) else {
        return Err("server lost its OAuth config mid-flow".into());
    };
    let mut pairs = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
        (
            "resource",
            str_field(&oauth, "resource").unwrap_or_default(),
        ),
    ];
    if let Some(secret) = client_secret {
        pairs.push(("client_secret", secret));
    }
    // This request carries the client_secret — pin the endpoint against the
    // server's own URL again before sending it anywhere.
    let token_url = pin_to(&server_url)?.check(
        &str_field(&oauth, "tokenEndpoint").unwrap_or_default(),
        "the token endpoint",
    )?;
    let Some(r) = token_post(&token_url, token_body(pairs)).await else {
        return Err("token exchange failed (network)".into());
    };
    let j: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    if !(200..300).contains(&r.status) || j.get("access_token").is_none() {
        let msg = str_field(&j, "error_description")
            .or_else(|| str_field(&j, "error"))
            .unwrap_or_else(|| format!("token exchange failed ({})", r.status));
        return Err(msg);
    }
    store_tokens(pg, sb, &server_id, &subject, &parse_token_response(&j)).await?;
    Ok((subject, server_id))
}

// ── Token use ───────────────────────────────────────────────────────────────

pub async fn has_oauth_tokens(
    pg: &PgPool,
    server_id: &str,
    subject: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from mcp_oauth_tokens where server_id::text = $1 and subject = $2",
    )
    .bind(server_id)
    .bind(subject)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

pub async fn drop_oauth_tokens(pg: &PgPool, server_id: &str, subject: &str) -> Result<(), String> {
    sqlx::query("delete from mcp_oauth_tokens where server_id::text = $1 and subject = $2")
        .bind(server_id)
        .bind(subject)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn stored_tokens(
    pg: &PgPool,
    sb: &SecretBox,
    server_id: &str,
    subject: &str,
) -> Result<Option<Value>, String> {
    let row: Option<(String,)> = sqlx::query_as(
        "select tokens_enc from mcp_oauth_tokens where server_id::text = $1 and subject = $2",
    )
    .bind(server_id)
    .bind(subject)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    let Some((enc,)) = row else {
        return Ok(None);
    };
    let opened = sb.open(&enc).map_err(|e| e.to_string())?;
    serde_json::from_str(&opened)
        .map(Some)
        .map_err(|_| "token store unreadable".to_string())
}

/// A live access token for the subject — silently refreshed when expiring.
/// None = not connected (or refresh rejected → tokens dropped, reconnect).
pub async fn oauth_token_for(
    pg: &PgPool,
    sb: &SecretBox,
    server_id: &str,
    subject: &str,
) -> Result<Option<String>, String> {
    let Some(t) = stored_tokens(pg, sb, server_id, subject).await? else {
        return Ok(None);
    };
    let access = str_field(&t, "accessToken").unwrap_or_default();
    let expires_at = t.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
    let refresh = str_field(&t, "refreshToken");
    let stale = expires_at != 0 && expires_at - now_ms() < 60_000;
    if !stale {
        return Ok(Some(access));
    }
    let Some(refresh_token) = refresh else {
        return Ok(Some(access)); // long-shot: let the upstream judge
    };
    let srv: Option<(Option<Value>, String)> =
        sqlx::query_as("select oauth, url from mcp_servers where id::text = $1")
            .bind(server_id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    let Some((Some(oauth), server_url)) = srv else {
        return Ok(Some(access));
    };
    let Some((client_id, client_secret, _)) = client_of(&oauth, sb) else {
        return Ok(Some(access));
    };
    let mut pairs = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.clone()),
        ("client_id", client_id),
        (
            "resource",
            str_field(&oauth, "resource").unwrap_or_default(),
        ),
    ];
    if let Some(secret) = client_secret {
        pairs.push(("client_secret", secret));
    }
    // Off-domain token endpoint: never send the secret, let the upstream
    // judge the old token.
    let refresh_url = match pin_to(&server_url).and_then(|p| {
        p.check(
            &str_field(&oauth, "tokenEndpoint").unwrap_or_default(),
            "the token endpoint",
        )
    }) {
        Ok(u) => u,
        Err(_) => return Ok(Some(access)),
    };
    let r = token_post(&refresh_url, token_body(pairs)).await;
    let j = r
        .as_ref()
        .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok());
    let ok = r.as_ref().is_some_and(|r| (200..300).contains(&r.status))
        && j.as_ref().is_some_and(|j| j.get("access_token").is_some());
    if !ok {
        if let Some(r) = &r
            && (r.status == 400 || r.status == 401)
        {
            let _ = drop_oauth_tokens(pg, server_id, subject).await; // revoked — force reconnect
        }
        return Ok(None);
    }
    let mut next = parse_token_response(j.as_ref().expect("ok implies j"));
    // Rotation is optional — keep the old refresh token when the new answer
    // carries none.
    if next.get("refreshToken").map(Value::is_null).unwrap_or(true) {
        next.as_object_mut()
            .expect("parse_token_response answers an object")
            .insert("refreshToken".into(), Value::String(refresh_token));
    }
    store_tokens(pg, sb, server_id, subject, &next).await?;
    Ok(Some(
        next.get("accessToken").and_then(Value::as_str).unwrap_or_default().to_string(),
    ))
}

/// Store admin-provided OAuth app credentials (providers without dynamic
/// registration — GitHub). The redirect URI must be registered with the
/// provider exactly as shown in the UI.
pub async fn set_manual_oauth_client(
    pg: &PgPool,
    sb: &SecretBox,
    server_id: &str,
    server_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
) -> Result<(), String> {
    let Some(config) = ensure_oauth_config(pg, server_id, server_url).await? else {
        return Err("this server doesn't advertise OAuth".into());
    };
    let secret_enc = client_secret
        .map(|s| sb.seal(s).map_err(|e| e.to_string()))
        .transpose()?;
    let mut next = config;
    next.as_object_mut()
        .expect("the config is an object")
        .insert(
            "client".into(),
            serde_json::json!({
                "id": client_id,
                "secretEnc": secret_enc,
                "redirectUri": redirect_uri,
            }),
        );
    store_oauth(pg, server_id, &next)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Surface the flags the UI needs: is this OAuth, does it self-register, is a
/// client configured.
pub async fn oauth_meta(pg: &PgPool, server_id: &str) -> Result<Option<Value>, sqlx::Error> {
    let oauth = stored_oauth(pg, server_id).await?;
    let Some(oauth) = oauth else {
        return Ok(None);
    };
    Ok(Some(serde_json::json!({
        "dcr": !oauth.get("registrationEndpoint").map(Value::is_null).unwrap_or(true),
        "clientSet": oauth.get("client").is_some(),
        // `documentation ?? null` — the field is null when discovery found
        // none, and the TS interface says null, never undefined.
        "documentation": oauth.get("documentation").and_then(Value::as_str),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrable_domains_collapse_to_the_company() {
        assert_eq!(registrable_domain("api.linear.app"), "linear.app");
        assert_eq!(registrable_domain("linear.app"), "linear.app");
        assert_eq!(registrable_domain("a.b.co.uk"), "b.co.uk");
        assert_eq!(registrable_domain("x.a.b.co.uk"), "b.co.uk");
        assert_eq!(registrable_domain("API.Linear.APP."), "linear.app");
    }

    #[test]
    fn pinning_refuses_off_domain_and_accepts_trusted() {
        let pin = pin_to("https://mcp.linear.app/").unwrap();
        assert_eq!(
            pin.check("https://linear.app/oauth/authorize", "the authorization endpoint")
                .unwrap(),
            "https://linear.app/oauth/authorize"
        );
        // A different company on a sibling subdomain: refused, and the
        // sentence names both domains.
        let err = pin
            .check("https://evil.app/oauth", "the token endpoint")
            .unwrap_err();
        assert_eq!(
            err,
            "the token endpoint points at evil.app, which is outside this server's own domain (linear.app), so it is refused"
        );
        let err = pin.check("not a url", "the resource metadata URL").unwrap_err();
        assert_eq!(err, "the resource metadata URL is not a valid URL");
        // The documented cross-domain issuer: GitHub's MCP server.
        let gh = pin_to("https://api.githubcopilot.com/mcp/").unwrap();
        assert!(gh.check("https://github.com/login/oauth/authorize", "x").is_ok());
    }

    #[test]
    fn set_param_matches_urlsearchparams_semantics() {
        let mut pairs = vec![
            ("a".into(), "1".into()),
            ("b".into(), "2".into()),
            ("a".into(), "3".into()),
        ];
        set_param(&mut pairs, "a", "9");
        assert_eq!(pairs, vec![("a".into(), "9".into()), ("b".into(), "2".into())]);
        set_param(&mut pairs, "c", "4");
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[2], ("c".into(), "4".into()));
    }

    #[test]
    fn form_encoding_matches_urlsearchparams() {
        assert_eq!(url_enc("a b~c-d"), "a+b~c-d");
        assert_eq!(url_enc("x&y=z"), "x%26y%3Dz");
    }
}
