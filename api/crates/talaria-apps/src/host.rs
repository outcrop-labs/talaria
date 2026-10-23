// The api's calls INTO the app host. App code is TypeScript and the host
// builds and loads it (rule 10); the api owns the lifecycle around it — the
// fetch, the swap, the record. So an install asks the host to build the app
// it just placed, and an update asks it to build and probe the new tree
// before the swap and to switch to it after.
//
// The host answers these on `/api/app-host/<slug>/…` to the same root-derived
// service credential the api's own `/api/internal/*` routes take
// (`x-talaria-internal`, `SecretRoot::internal_token`), so the call needs no
// session and nothing else can make it.

use std::time::Duration;

use serde_json::Value;

/// Where the api reaches the app host: `TALARIA_APP_HOST_URL`, else this
/// machine's loopback on the host's `PORT` (the host spawns the api in
/// production and `talaria dev` lifts PORT into its environment).
pub fn app_host_url() -> String {
    match std::env::var("TALARIA_APP_HOST_URL") {
        Ok(u) if !u.trim().is_empty() => u.trim().trim_end_matches('/').to_string(),
        _ => format!("http://127.0.0.1:{}", host_port()),
    }
}

fn host_port() -> String {
    std::env::var("PORT")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "5273".into())
}

/// A build can take minutes on a loaded machine; a call that has not answered
/// by then has failed.
pub const BUILD_DEADLINE: Duration = Duration::from_secs(600);

/// The app host, as the api calls it.
#[derive(Clone)]
pub struct AppHost {
    base: String,
    token: String,
}

impl AppHost {
    /// `token` is the root-derived service credential
    /// (`state.cfg.secret_root.internal_token()`).
    pub fn new(token: String) -> Self {
        Self {
            base: app_host_url(),
            token,
        }
    }

    pub fn at(base: impl Into<String>, token: String) -> Self {
        Self {
            base: base.into(),
            token,
        }
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(BUILD_DEADLINE)
            .build()
            .map_err(|e| format!("app host client: {e}"))?;
        let res = client
            .post(format!("{}{path}", self.base))
            .header("x-talaria-internal", &self.token)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| format!("the app host did not answer: {e}"))?;
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let why = parsed
                .get("error")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| format!("status {status}"));
            return Err(format!("the app host refused: {why}"));
        }
        Ok(parsed)
    }

    /// Compile and load-probe the app now, and start its database: what
    /// Manage → Apps' Rebuild does. `{ ok, build, error? }`.
    pub async fn rebuild(&self, slug: &str) -> Result<Value, String> {
        self.post(
            &format!("/api/app-host/{slug}/rebuild"),
            serde_json::json!({}),
        )
        .await
    }

    /// One step of an update or uninstall, on the host's side
    /// (`/api/app-host/<slug>/lifecycle`): `stage` builds and probes the tree
    /// at `apps/.<slug>.next` without touching the live app, `activate`
    /// switches the live app to that build after the swap, `release` lets go
    /// of the app without switching, and `removed` closes what the host still
    /// holds for an uninstalled app. `{ ok, build?, error? }`.
    pub async fn lifecycle(&self, slug: &str, step: &str) -> Result<Value, String> {
        self.post(
            &format!("/api/app-host/{slug}/lifecycle"),
            serde_json::json!({ "step": step }),
        )
        .await
    }
}

/// `{ ok: true, … }` from the host.
pub fn host_ok(v: &Value) -> bool {
    v.get("ok").and_then(Value::as_bool) == Some(true)
}

/// The host's own sentence for a failed step.
pub fn host_error(v: &Value) -> String {
    v.get("error")
        .and_then(Value::as_str)
        .unwrap_or("the app failed to compile or load")
        .to_string()
}
