// The fleet's Talaria-toolkit MCP endpoint: talaria-mcp (mcp/dist) running in
// HTTP mode as a child of the app, one endpoint every agent's config points
// at (rendered by fleet_render). Self-supervising: ensure_mcp_service() is
// called opportunistically (renders, comms reads); it probes first so dev
// reloads and multiple entrypoints never double-spawn, and respawns on exit.
//
// The child is a plain node script (`#!/usr/bin/env node`, engines pin
// >= 20), so it spawns under `node`, overridable with TALARIA_JS_RUNTIME
// for installs that run everything under bun.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::AsyncBufReadExt;

/// The toolkit's HTTP port.
pub fn mcp_port() -> u16 {
    std::env::var("TALARIA_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(5280)
}

/// The URL agents reach the toolkit at (host-run app ⇒ host gateway).
pub fn mcp_fleet_url() -> String {
    format!("http://host.docker.internal:{}/mcp", mcp_port())
}

/// mcp/dist/index.js resolved against the repo layout (cwd = api/).
pub fn mcp_service_entry() -> PathBuf {
    std::env::current_dir()
        .map(|c| c.join("../mcp/dist/index.js"))
        .unwrap_or_else(|_| PathBuf::from("../mcp/dist/index.js"))
}

struct SpawnState {
    starting: bool,
    last_spawn_ms: u64,
}

fn state() -> &'static Mutex<SpawnState> {
    static STATE: std::sync::OnceLock<Mutex<SpawnState>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(SpawnState {
            starting: false,
            last_spawn_ms: 0,
        })
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Is the toolkit answering? 401/400 count as alive (unauthenticated, not
/// dead); a 404 or anything 5xx does not.
async fn reachable() -> bool {
    let url = format!("http://127.0.0.1:{}/mcp", mcp_port());
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
    else {
        return false;
    };
    match client
        .post(&url)
        .header("content-type", "application/json")
        .send()
        .await
    {
        Ok(r) => r.status().as_u16() != 404 && r.status().as_u16() < 500,
        Err(_) => false,
    }
}

/// Which JS runtime spawns the child (see the header note).
fn js_runtime() -> String {
    std::env::var("TALARIA_JS_RUNTIME").unwrap_or_else(|_| "node".into())
}

/// The spawn itself, already past the guards. The debounce stamps HERE, only
/// once the entry exists — a missing dist must not consume the 10s window, or
/// the not-built error would print once and then never again.
/// stderr is piped through to the log (the child's own diagnostics are the
/// only place its failures speak), and an exit is logged as the respawn
/// promise it is.
async fn spawn_child() {
    let entry = mcp_service_entry();
    if !entry.exists() {
        tracing::error!(
            "[mcp-service] not built at {} — run \"npm run build\" in mcp/",
            entry.display()
        );
        return;
    }
    if let Ok(mut st) = state().lock() {
        st.last_spawn_ms = now_ms();
    }
    let port = crate::fleet::layout::app_port();
    let mut child = tokio::process::Command::new(js_runtime())
        .arg(&entry)
        .env("MCP_HTTP_PORT", mcp_port().to_string())
        .env("TALARIA_URL", format!("http://localhost:{port}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn();
    match child.as_mut() {
        Ok(c) => {
            if let Some(stderr) = c.stderr.take() {
                tokio::spawn(async move {
                    let mut lines = tokio::io::BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if !line.trim().is_empty() {
                            tracing::error!("[talaria-mcp] {}", line.trim());
                        }
                    }
                });
            }
            tokio::spawn(async move {
                // `child` is the Result — wait through it so a spawn failure
                // logs as an exit rather than being swallowed.
                match child {
                    Ok(mut c) => match c.wait().await {
                        Ok(status) => tracing::error!(
                            "[mcp-service] exited ({}) — will respawn on next ensure",
                            status.code().unwrap_or(-1)
                        ),
                        Err(e) => tracing::error!("[mcp-service] wait failed: {e}"),
                    },
                    Err(e) => tracing::error!("[mcp-service] spawn failed: {e}"),
                }
            });
        }
        Err(e) => tracing::error!("[mcp-service] spawn failed: {e}"),
    }
}

/// Make sure the fleet MCP endpoint is up (spawn if not). Fire-and-forget —
/// the render and comms-read callers must never block on a spawn, and two
/// callers a second apart must not double-spawn: guarded by `starting` and a
/// 10s respawn debounce.
pub fn ensure_mcp_service() {
    {
        let Ok(mut st) = state().lock() else {
            return;
        };
        if st.starting || now_ms().saturating_sub(st.last_spawn_ms) < 10_000 {
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

/// Drop guard: `starting` clears on every path out of the spawned task.
struct ResetStarting;

impl Drop for ResetStarting {
    fn drop(&mut self) {
        if let Ok(mut st) = state().lock() {
            st.starting = false;
        }
    }
}

/// Bring the toolkit up and WAIT for it, for the callers that need it
/// answering right now rather than soon (the MCP refresh button: a person
/// pressing it on a freshly booted app, before anything has rendered or read
/// comms and the probe ever ran).
pub async fn await_mcp_service(timeout_ms: u64) -> bool {
    if reachable().await {
        return true;
    }
    ensure_mcp_service();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if reachable().await {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fleet_url_points_agents_at_the_host_toolkit_port() {
        // The port is env-read, so assert the URL's SHAPE against whatever
        // this process resolves — the template is the contract.
        let url = mcp_fleet_url();
        assert!(url.starts_with("http://host.docker.internal:"), "{url}");
        assert!(url.ends_with("/mcp"), "{url}");
        assert_eq!(
            url,
            format!("http://host.docker.internal:{}/mcp", mcp_port())
        );
    }

    #[test]
    fn the_entry_is_the_built_mcp_dist() {
        let e = mcp_service_entry();
        assert!(e.ends_with("mcp/dist/index.js"), "{}", e.display());
    }
}
