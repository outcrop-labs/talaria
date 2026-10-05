// The omp auth bridge, from the api's side: a supervised child process that
// performs Oh My Pi's OAuth flows and token refreshes on our behalf.
//
// WHY A CHILD AND NOT RUST. Talaria holds the coding-account credentials and
// serves them over omp's auth-broker protocol, which makes this instance the
// canonical refresher. Doing that means knowing 23 providers' client ids, PKCE
// quirks, device-code endpoints, token exchanges and per-provider refresh
// hooks — all of which omp already implements declaratively and republishes
// several times a week. Reimplementing them here would be a mirror of an
// upstream that moves faster than we could follow, so instead `omp-auth/`
// bundles upstream's own engine and this crate drives it. Nothing in Talaria
// encodes a provider parameter.
//
// SUPERVISION mirrors talaria-mcp-service's: probe first so dev reloads and
// several entrypoints never double-spawn, debounce respawns, and never block a
// caller on a spawn. Unlike that one, the bridge is reached ONLY from this
// process over loopback with a bearer minted at boot — it hands out
// credentials, so a second local process must not be able to ask it for one.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use talaria_agent_auth::now_ms;
use tokio::io::AsyncBufReadExt;

/// The bridge's loopback port. Next to the api's own (5274) and the toolkit's
/// (5280), with the same env override shape.
pub fn bridge_port() -> u16 {
    std::env::var("TALARIA_OMP_AUTH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .filter(|p| *p > 0)
        .unwrap_or(5276)
}

/// The bearer the bridge demands, minted once per process. Not configurable
/// and never logged: it is an in-memory capability, and an operator who could
/// set it could also read every credential the bridge touches.
fn bridge_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| {
        let mut raw = [0u8; 24];
        // A failed draw must not degrade to a guessable token: an empty
        // token is one the bridge refuses every request with, which fails
        // the feature loudly instead of opening it.
        if getrandom::fill(&mut raw).is_err() {
            return String::new();
        }
        raw.iter().map(|b| format!("{b:02x}")).collect()
    })
}

/// omp-auth/dist/server.js resolved against the repo layout (cwd = api/).
pub fn bridge_entry() -> PathBuf {
    std::env::current_dir()
        .map(|c| c.join("../omp-auth/dist/server.js"))
        .unwrap_or_else(|_| PathBuf::from("../omp-auth/dist/server.js"))
}

/// Which JS runtime spawns the child. Same override as the toolkit's, for the
/// same reason: the container image ships bun and no node.
fn js_runtime() -> String {
    std::env::var("TALARIA_JS_RUNTIME").unwrap_or_else(|_| "node".into())
}

fn base_url() -> String {
    format!("http://127.0.0.1:{}", bridge_port())
}

struct SpawnState {
    starting: bool,
    last_spawn_ms: u64,
}

fn state() -> &'static Mutex<SpawnState> {
    static STATE: OnceLock<Mutex<SpawnState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(SpawnState {
            starting: false,
            last_spawn_ms: 0,
        })
    })
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("omp-auth client: {e}"))
}

async fn reachable() -> bool {
    let Ok(c) = client(Duration::from_millis(1500)) else {
        return false;
    };
    match c.get(format!("{}/healthz", base_url())).send().await {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

async fn spawn_child() {
    let entry = bridge_entry();
    if !entry.exists() {
        tracing::error!(
            "[omp-auth] not built at {} — run \"bun run build\" in omp-auth/",
            entry.display()
        );
        return;
    }
    if let Ok(mut st) = state().lock() {
        st.last_spawn_ms = now_ms() as u64;
    }
    let child = tokio::process::Command::new(js_runtime())
        .arg(&entry)
        .env("OMP_AUTH_BRIDGE_PORT", bridge_port().to_string())
        .env("OMP_AUTH_BRIDGE_TOKEN", bridge_token())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn();
    match child {
        Ok(mut c) => {
            if let Some(stderr) = c.stderr.take() {
                tokio::spawn(async move {
                    let mut lines = tokio::io::BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if !line.trim().is_empty() {
                            tracing::error!("[omp-auth] {}", line.trim());
                        }
                    }
                });
            }
            tokio::spawn(async move {
                match c.wait().await {
                    Ok(status) => tracing::warn!(
                        "[omp-auth] exited ({}) — will respawn on next ensure",
                        status.code().unwrap_or(-1)
                    ),
                    Err(e) => tracing::error!("[omp-auth] wait failed: {e}"),
                }
            });
        }
        Err(e) => tracing::error!("[omp-auth] spawn failed: {e}"),
    }
}

/// Drop guard: `starting` clears on every path out of the spawned task.
struct ResetStarting;

impl Drop for ResetStarting {
    fn drop(&mut self) {
        if let Ok(mut st) = state().lock() {
            st.starting = false;
        }
    }
}

/// Make sure the bridge is up — fire-and-forget, debounced 10s.
pub fn ensure_bridge() {
    {
        let Ok(mut st) = state().lock() else { return };
        if st.starting || (now_ms() as u64).saturating_sub(st.last_spawn_ms) < 10_000 {
            return;
        }
        st.starting = true;
    }
    tokio::spawn(async move {
        let _guard = ResetStarting;
        if reachable().await {
            return;
        }
        spawn_child().await;
    });
}

/// Bring the bridge up and WAIT for it. Every route here needs an answer now —
/// a person pressing "Sign in" is not going to be told to try again — so this,
/// not `ensure_bridge`, is what the routes call.
pub async fn await_bridge(timeout_ms: u64) -> bool {
    if reachable().await {
        return true;
    }
    ensure_bridge();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if reachable().await {
            return true;
        }
    }
    false
}

// ── Calls ───────────────────────────────────────────────────────────────────

/// A refresh can take as long as a provider's token endpoint does, and it is
/// on the path of a harness waiting to make a call. Generous, but bounded.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

async fn call(method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    if !await_bridge(15_000).await {
        return Err("the omp auth bridge is not running".into());
    }
    let c = client(CALL_TIMEOUT)?;
    let mut req = c
        .request(method, format!("{}{path}", base_url()))
        .bearer_auth(bridge_token());
    if let Some(body) = body {
        req = req.json(&body);
    }
    let res = req
        .send()
        .await
        .map_err(|e| format!("omp auth bridge unreachable: {e}"))?;
    let status = res.status();
    let value: Value = res.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(value);
    }
    // The bridge's own message is the useful one — a provider's refusal text
    // ("device code expired", "needs an interactive browser") says more than a
    // status code, and it is what the UI shows.
    Err(value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("omp auth bridge: {status}")))
}

/// Every OAuth provider omp can sign in to, with its flow shape.
pub async fn providers() -> Result<Vec<Value>, String> {
    let v = call(reqwest::Method::GET, "/v1/providers", None).await?;
    Ok(v.get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// The models omp's catalog knows for one provider — what the role pickers
/// offer. Empty for a provider omp has no catalog entry for, which is not an
/// error: the pickers then offer nothing rather than inventing ids.
pub async fn models(provider: &str) -> Result<Vec<Value>, String> {
    let path = format!("/v1/models?provider={}", urlencode(provider));
    let v = call(reqwest::Method::GET, &path, None).await?;
    Ok(v.get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

pub async fn login_start(provider: &str) -> Result<Value, String> {
    call(
        reqwest::Method::POST,
        "/v1/login",
        Some(json!({ "provider": provider })),
    )
    .await
}

/// Poll one login. A finished session is consumed by this read — the bridge
/// serves a credential exactly once — so a caller that sees `phase: "done"`
/// must store it then and there.
pub async fn login_poll(id: &str) -> Result<Value, String> {
    call(
        reqwest::Method::GET,
        &format!("/v1/login/{}", urlencode(id)),
        None,
    )
    .await
}

/// Answer the provider's pending question, or paste the authorization code.
pub async fn login_input(id: &str, value: &str) -> Result<Value, String> {
    call(
        reqwest::Method::POST,
        &format!("/v1/login/{}/input", urlencode(id)),
        Some(json!({ "value": value })),
    )
    .await
}

pub async fn login_cancel(id: &str) -> Result<(), String> {
    call(
        reqwest::Method::DELETE,
        &format!("/v1/login/{}", urlencode(id)),
        None,
    )
    .await
    .map(|_| ())
}

/// Refresh one credential through the provider's own refresher.
pub async fn refresh(provider: &str, credential: &Value) -> Result<Value, String> {
    let v = call(
        reqwest::Method::POST,
        "/v1/refresh",
        Some(json!({ "provider": provider, "credential": credential })),
    )
    .await?;
    v.get("credential")
        .cloned()
        .ok_or_else(|| "the omp auth bridge returned no credential".to_string())
}

/// Percent-encode a path or query segment. Provider ids and session uuids are
/// tame, but neither is ours to trust into a URL unescaped.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_token_is_minted_once_and_is_not_trivial() {
        let a = bridge_token();
        let b = bridge_token();
        assert_eq!(a, b, "one token per process");
        assert_eq!(a.len(), 48, "24 random bytes, hex");
    }

    #[test]
    fn urlencode_escapes_what_a_path_cannot_carry() {
        assert_eq!(urlencode("openai-codex"), "openai-codex");
        assert_eq!(urlencode("a/b?c=d"), "a%2Fb%3Fc%3Dd");
        assert_eq!(urlencode("llama.cpp"), "llama.cpp");
    }

    #[test]
    fn the_port_has_a_default_beside_the_api_and_the_toolkit() {
        // Only meaningful with the env unset, which is the test environment.
        if std::env::var("TALARIA_OMP_AUTH_PORT").is_err() {
            assert_eq!(bridge_port(), 5276);
        }
    }
}
