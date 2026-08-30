// The fleet's brain is Talaria's own gateway. Rather than pointing agents at a
// raw LLM endpoint, the whole fleet routes its tool loop through Talaria's org
// gateway (`/api/llm/v1`) — so every agent call is guarded (confab checks see
// the full tool trace), metered in one ledger, and observable in one place.
//
// Port of ui/src/server/fleet-brain.ts. Two halves: the ROUTING half (the
// model-set question and the config transform) and the PROVISIONING half
// (ensure_gateway_brain — the two gateway keys minted into the Talaria-owned
// fleet .env, the two key-list settings, and the LLM_* lines the containers
// interpolate).

use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashSet;

use crate::auth::sha256_hex;
use crate::fleet_layout;
use crate::gateway::registry::list_endpoints;
use crate::gateway::settings::{get_setting, set_setting};
use crate::llm_keys::mint_key;

// What a rendered agent config points its model specs at: the gateway, on the
// PERSONA key — the one the gateway leaves unmetered, because the flow that
// drove the turn writes the row. Sandbox harnesses get the workbench key
// instead (workbench-harnesses).
const GATEWAY_BASE_URL: &str = "${LLM_BASE_URL}";
const GATEWAY_API_KEY: &str = "${LLM_API_KEY}";

// Imported agents carry legacy litellm-router model names. Map them to the real
// provider model ids Talaria's gateway serves. As agents are redesigned they use
// real ids directly and this bridge shrinks to nothing.
fn legacy_model(name: &str) -> Option<&'static str> {
    match name {
        // legacy litellm "glm" → OpenRouter's Z.AI GLM
        "glm" => Some("z-ai/glm-5.2"),
        _ => None,
    }
}

pub struct GatewayModels {
    /// Every model name Talaria's gateway can resolve (across all endpoints).
    pub served: HashSet<String>,
    /// Default model to fall back to for names the gateway doesn't serve yet.
    pub fallback: Option<String>,
}

/// The set of models Talaria's gateway serves, for the config transform below.
/// Never fails — an unreadable registry answers the empty set a fresh install
/// would, and a render routes everything to the fallback in that world.
pub async fn gateway_model_set(pg: &PgPool) -> GatewayModels {
    let eps = list_endpoints(pg).await.unwrap_or_default();
    let mut served = HashSet::new();
    for e in &eps {
        for m in &e.models {
            served.insert(m.clone());
        }
    }
    let local = eps
        .iter()
        .find(|e| e.class == "local" && !e.models.is_empty())
        .or_else(|| eps.iter().find(|e| !e.models.is_empty()));
    GatewayModels {
        served,
        fallback: local.and_then(|e| e.models.first().cloned()),
    }
}

/// An LLM model spec inside an agent config: has a model name and some endpoint
/// marker (base_url / provider / key_env / api_key). MCP servers (url + headers,
/// no `model`) and other blocks are left untouched.
fn is_model_spec(o: &Value) -> bool {
    let Some(obj) = o.as_object() else {
        return false;
    };
    obj.get("model").is_some_and(Value::is_string)
        && ["base_url", "provider", "key_env", "api_key"]
            .iter()
            .any(|k| obj.contains_key(*k))
}

/// Route every LLM model spec in a raw agent config through Talaria's gateway:
/// base_url/api_key point at the gateway, provider becomes openai-compatible, and
/// the model name is kept (or falls back to the default if the gateway doesn't
/// serve it yet). Un-interweaves agents from litellm/inference-router/anthropic —
/// they have exactly one upstream, and Talaria routes to the real providers.
pub fn route_config_through_gateway(
    raw: &Value,
    models: &GatewayModels,
    mut warn: impl FnMut(String),
) -> Value {
    fn walk(v: &Value, models: &GatewayModels, warn: &mut impl FnMut(String)) -> Value {
        match v {
            Value::Array(items) => {
                Value::Array(items.iter().map(|i| walk(i, models, warn)).collect())
            }
            Value::Object(map) => {
                if is_model_spec(v) {
                    let name = map["model"].as_str().unwrap_or_default().to_string();
                    let canonical = legacy_model(&name).unwrap_or(&name).to_string();
                    let model = if models.served.contains(&canonical) {
                        canonical.clone()
                    } else {
                        models.fallback.clone().unwrap_or(canonical.clone())
                    };
                    // Warn only when we couldn't route to the intended model
                    // (unserved → fallback); a known legacy remap (glm →
                    // z-ai/glm-5.2) is intentional.
                    if !models.served.contains(&canonical) {
                        warn(format!(
                            "model \"{name}\" not served by the gateway — routing to \"{model}\""
                        ));
                    }
                    let mut out = map.clone();
                    out.remove("key_env");
                    out.insert("model".into(), Value::String(model));
                    out.insert("base_url".into(), Value::String(GATEWAY_BASE_URL.into()));
                    out.insert("api_key".into(), Value::String(GATEWAY_API_KEY.into()));
                    out.insert("provider".into(), Value::String("custom".into()));
                    Value::Object(out)
                } else {
                    Value::Object(
                        map.iter()
                            .map(|(k, val)| (k.clone(), walk(val, models, warn)))
                            .collect(),
                    )
                }
            }
            leaf => leaf.clone(),
        }
    }
    walk(raw, models, &mut warn)
}

// ── The provisioning half ────────────────────────────────────────────────────

const GATEWAY_KEY_NAME: &str = "fleet-gateway";
// The workbench's own gateway credential. Harness runs reach the gateway with
// this key, never the personas' — metering has to treat the two oppositely
// (see ensure_gateway_brain), and a key is the only thing the gateway can tell
// its callers apart by. Env name matches workbench-harnesses' gateway env.
const WORKBENCH_KEY_NAME: &str = "workbench-gateway";
const WORKBENCH_KEY_ENV: &str = "LLM_WORKBENCH_API_KEY";
// Where the fleet reaches Talaria's gateway. In dev the app runs on the host,
// so agents reach it via the docker host-gateway (`extra_hosts` in the
// chassis) on :5273. In a containerized stack, set TALARIA_GATEWAY_SELF_URL to
// the app's service DNS (e.g. http://talaria-ui:3000/api/llm/v1).
const DEFAULT_SELF_URL: &str = "http://host.docker.internal:5273/api/llm/v1";

fn self_url() -> String {
    match std::env::var("TALARIA_GATEWAY_SELF_URL") {
        Ok(v) if !v.is_empty() => v,
        _ => DEFAULT_SELF_URL.into(),
    }
}

/// A gateway URL is one that ends in the gateway path — safe for Talaria to own
/// and migrate. Any other value means the operator deliberately pointed the
/// fleet at a raw upstream, so we leave it alone.
pub(crate) fn is_gateway_url(u: &str) -> bool {
    regex::Regex::new(r"/api/llm/v1/?$")
        .expect("gateway url pattern")
        .is_match(u.trim())
}

/// Read a KEY=value line from env-file text. The captured value is trimmed,
/// exactly the TS regex's `m[1].trim()`.
pub(crate) fn read_env_line(content: &str, key: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r"(?m)^{}=(.*)$", regex::escape(key)))
        .expect("env line pattern");
    re.captures(content).map(|c| c[1].trim().to_string())
}

/// Set or replace a KEY=value line; append if absent. The append shape is
/// TS-exact, including the leading newline when the file is empty.
pub(crate) fn upsert_env_line(content: &str, key: &str, value: &str) -> String {
    let re =
        regex::Regex::new(&format!(r"(?m)^{}=.*$", regex::escape(key))).expect("env line pattern");
    let line = format!("{key}={value}");
    if re.is_match(content) {
        return re.replace(content, line.as_str()).into_owned();
    }
    format!("{}\n{line}\n", content.trim_end_matches('\n'))
}

struct EnsuredKey {
    secret: String,
    rotated: bool,
}

/// Keep the current key if it's a live key of that name; otherwise mint a
/// fresh one (revoking any stale rows) under an admin owner. Returns the
/// plaintext secret to write into the fleet .env, or None if there's no user
/// to own it yet (nothing to own the key — try again next render).
async fn ensure_gateway_key(
    pg: &PgPool,
    name: &str,
    current_key: Option<&str>,
) -> Result<Option<EnsuredKey>, String> {
    if let Some(cur) = current_key.filter(|k| k.starts_with("tlk_")) {
        let live: Option<(i32,)> = sqlx::query_as(
            "select 1 from llm_api_keys where name = $1 and revoked_at is null and key_hash = $2",
        )
        .bind(name)
        .bind(sha256_hex(cur))
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("gateway key live-check failed: {e}"))?;
        if live.is_some() {
            return Ok(Some(EnsuredKey {
                secret: cur.to_string(),
                rotated: false,
            }));
        }
    }
    let owner: Option<(String,)> = sqlx::query_as(
        "select id::text from users order by (role = 'admin') desc, created_at asc limit 1",
    )
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("gateway key owner lookup failed: {e}"))?;
    let Some((owner_id,)) = owner else {
        return Ok(None);
    };
    sqlx::query(
        "update llm_api_keys set revoked_at = now() where name = $1 and revoked_at is null",
    )
    .bind(name)
    .execute(pg)
    .await
    .map_err(|e| format!("stale gateway key revoke failed: {e}"))?;
    let (_, secret) = mint_key(pg, &owner_id, name)
        .await
        .map_err(|e| format!("gateway key mint failed: {e}"))?;
    Ok(Some(EnsuredKey {
        secret,
        rotated: true,
    }))
}

/// First gateway-resolvable model — a sane default LLM_MODEL so
/// freshly-added endpoints light up the fleet without hand-editing. None if
/// nothing is configured. (Same local-first pick the routing half's fallback
/// makes — TS spells the identical body twice.)
async fn default_gateway_model(pg: &PgPool) -> Option<String> {
    gateway_model_set(pg).await.fallback
}

#[derive(Debug, Clone)]
pub struct GatewayBrain {
    pub url: Option<String>,
    pub model: Option<String>,
    pub key_rotated: bool,
    /// False when the operator pointed the fleet at a non-gateway upstream.
    pub managed: bool,
}

/// Provision (or refresh) the fleet's gateway brain in the Talaria-owned .env.
/// Idempotent; a failure here is the caller's to treat as best-effort — it
/// never blocks a fleet render (TS's renderFleet wraps the call in try/catch;
/// the Rust render loop does the same when it lands).
pub async fn ensure_gateway_brain(pg: &PgPool) -> Result<GatewayBrain, String> {
    let env_path = fleet_layout::fleet_env();
    if let Some(parent) = env_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("fleet dir unwritable ({}): {e}", parent.display()))?;
    }
    let content = tokio::fs::read_to_string(&env_path)
        .await
        .unwrap_or_default();

    // Two settings, two different questions — one list answering both is what
    // corrupted the ledger, so keep them apart:
    //
    //   gateway_unmetered_keys — keys the gateway must NOT write a usage row
    //     for, because their spend already reaches the ledger from another
    //     writer. ONLY the personas' key qualifies: a chat/channel/ticket turn
    //     writes one row from the persona gateway's reported usage for the
    //     whole turn, so metering the inner-loop calls behind it would count
    //     every turn twice. `workbench-gateway` is deliberately NOT here —
    //     nothing else records a harness run, so the gateway is where that
    //     spend lands.
    //
    //   gateway_agent_loop_keys — keys whose replies must never be rewritten
    //     with a guard caveat, because the caller is an agent's own tool loop
    //     and a caveat would contaminate its context. BOTH gateway keys
    //     qualify (findings still record either way).
    //
    // A stored setting that isn't a string array is the same failure TS would
    // throw on — the whole brain step fails and the render carries on without
    // it, rather than silently overwriting a corrupt value.
    let unmetered: Vec<String> = serde_json::from_value(
        get_setting(pg, "gateway_unmetered_keys", json!([GATEWAY_KEY_NAME])).await,
    )
    .map_err(|e| format!("gateway_unmetered_keys is unreadable: {e}"))?;
    if !unmetered.iter().any(|k| k == GATEWAY_KEY_NAME) {
        let mut next = unmetered;
        next.push(GATEWAY_KEY_NAME.into());
        set_setting(pg, "gateway_unmetered_keys", &json!(next))
            .await
            .map_err(|e| format!("gateway_unmetered_keys write failed: {e}"))?;
    }
    let loop_keys: Vec<String> = serde_json::from_value(
        get_setting(
            pg,
            "gateway_agent_loop_keys",
            json!([GATEWAY_KEY_NAME, WORKBENCH_KEY_NAME]),
        )
        .await,
    )
    .map_err(|e| format!("gateway_agent_loop_keys is unreadable: {e}"))?;
    let missing = [GATEWAY_KEY_NAME, WORKBENCH_KEY_NAME]
        .into_iter()
        .filter(|n| !loop_keys.contains(&n.to_string()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let mut next = loop_keys;
        next.extend(missing.iter().map(|n| n.to_string()));
        set_setting(pg, "gateway_agent_loop_keys", &json!(next))
            .await
            .map_err(|e| format!("gateway_agent_loop_keys write failed: {e}"))?;
    }

    if let Some(cur) = read_env_line(&content, "LLM_BASE_URL").filter(|u| !is_gateway_url(u)) {
        // Operator override: a raw upstream. Respect it — no gateway brain.
        return Ok(GatewayBrain {
            url: Some(cur),
            model: read_env_line(&content, "LLM_MODEL").filter(|m| !m.is_empty()),
            key_rotated: false,
            managed: false,
        });
    }

    let url = self_url();
    let key = ensure_gateway_key(
        pg,
        GATEWAY_KEY_NAME,
        read_env_line(&content, "LLM_API_KEY").as_deref(),
    )
    .await?;
    let wb_key = ensure_gateway_key(
        pg,
        WORKBENCH_KEY_NAME,
        read_env_line(&content, WORKBENCH_KEY_ENV).as_deref(),
    )
    .await?;

    let mut next = content.clone();
    if !next.contains("# talaria-managed (gateway brain)") {
        next = format!(
            "{}\n\n# talaria-managed (gateway brain) — the fleet's default LLM is Talaria's org\n\
             # gateway, so every agent call is guarded, metered, and observable. Configure\n\
             # the real upstream model in-app on /models; don't hand-edit these lines.\n\
             # LLM_API_KEY is the personas' loop; LLM_WORKBENCH_API_KEY is the sandbox\n\
             # harnesses' — separate credentials so the ledger can tell them apart.\n",
            next.trim_end_matches('\n')
        );
    }
    next = upsert_env_line(&next, "LLM_BASE_URL", &url);
    if let Some(k) = &key {
        next = upsert_env_line(&next, "LLM_API_KEY", &k.secret);
    }
    if let Some(k) = &wb_key {
        next = upsert_env_line(&next, WORKBENCH_KEY_ENV, &k.secret);
    }

    let mut model = read_env_line(&next, "LLM_MODEL").filter(|m| !m.is_empty());
    if model.is_none()
        && let Some(m) = default_gateway_model(pg).await
    {
        next = upsert_env_line(&next, "LLM_MODEL", &m);
        model = Some(m);
    }

    if next != content {
        tokio::fs::write(&env_path, &next)
            .await
            .map_err(|e| format!("fleet .env unwritable ({}): {e}", env_path.display()))?;
    }
    Ok(GatewayBrain {
        url: Some(url),
        model,
        key_rotated: key.as_ref().is_some_and(|k| k.rotated)
            || wb_key.as_ref().is_some_and(|k| k.rotated),
        managed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn models(served: &[&str], fallback: Option<&str>) -> GatewayModels {
        GatewayModels {
            served: served.iter().map(|s| s.to_string()).collect(),
            fallback: fallback.map(|f| f.to_string()),
        }
    }

    #[test]
    fn a_model_spec_is_a_model_plus_an_endpoint_marker() {
        assert!(is_model_spec(&json!({"model": "m", "base_url": "x"})));
        assert!(is_model_spec(&json!({"model": "m", "provider": "p"})));
        assert!(is_model_spec(&json!({"model": "m", "key_env": "K"})));
        assert!(is_model_spec(&json!({"model": "m", "api_key": "k"})));
        // MCP servers, bare names, arrays, scalars: untouched shapes.
        assert!(!is_model_spec(&json!({"url": "u", "headers": {}})));
        assert!(!is_model_spec(&json!({"model": "m"})));
        assert!(!is_model_spec(&json!(["model", "m"])));
        assert!(!is_model_spec(&json!("model")));
    }

    #[test]
    fn specs_route_to_the_gateway_and_drop_key_env() {
        let raw = json!({
            "specs": {
                "main": {"model": "qwen3:14b", "provider": "ollama", "key_env": "OLD_KEY"},
                "mcp_servers": {"talaria": {"url": "http://x/talaria", "headers": {"h": "v"}}}
            },
            "nested": [{"model": "qwen3:14b", "base_url": "http://litellm:4000"}]
        });
        let mut warned = Vec::new();
        let out =
            route_config_through_gateway(&raw, &models(&["qwen3:14b"], None), |m| warned.push(m));
        let main = &out["specs"]["main"];
        assert_eq!(main["model"], json!("qwen3:14b"));
        assert_eq!(main["base_url"], json!("${LLM_BASE_URL}"));
        assert_eq!(main["api_key"], json!("${LLM_API_KEY}"));
        assert_eq!(main["provider"], json!("custom"));
        assert!(
            main.get("key_env").is_none(),
            "the raw upstream's env name must not survive"
        );
        assert_eq!(out["nested"][0]["base_url"], json!("${LLM_BASE_URL}"));
        // MCP servers ride through byte-for-byte
        assert_eq!(
            out["specs"]["mcp_servers"]["talaria"]["url"],
            json!("http://x/talaria")
        );
        assert!(warned.is_empty(), "served models do not warn");
    }

    #[test]
    fn a_spec_object_is_terminal_its_own_children_never_walk() {
        // TS's walk returns at the spec branch — a spec carries no nested
        // specs, so anything nested inside one rides through untouched. This
        // pins that branch semantics, not an accident of the walk order.
        let raw = json!({
            "model": "qwen3:14b",
            "provider": "ollama",
            "weird": {"model": "gpt:old", "base_url": "http://litellm:4000"}
        });
        let out = route_config_through_gateway(&raw, &models(&["qwen3:14b"], None), |_| {});
        assert_eq!(
            out["weird"]["base_url"],
            json!("http://litellm:4000"),
            "inside a spec, nothing re-routes"
        );
    }

    #[test]
    fn unserved_names_fall_back_and_legacy_names_remap_without_warning() {
        let raw = json!({"main": {"model": "glm", "base_url": "x"}, "deep": {"model": "gpt:old", "base_url": "x"}});
        let mut warned = Vec::new();
        let out = route_config_through_gateway(
            &raw,
            &models(&["z-ai/glm-5.2"], Some("qwen3:14b")),
            |m| warned.push(m),
        );
        // glm is a known remap landing on a SERVED id — intentional, no warning.
        assert_eq!(out["main"]["model"], json!("z-ai/glm-5.2"));
        // gpt:old is served by nothing — falls back, and says so.
        assert_eq!(out["deep"]["model"], json!("qwen3:14b"));
        assert_eq!(
            warned,
            vec![
                "model \"gpt:old\" not served by the gateway — routing to \"qwen3:14b\""
                    .to_string()
            ]
        );
    }

    #[test]
    fn with_no_fallback_an_unserved_name_keeps_itself() {
        let raw = json!({"model": "gpt:old", "base_url": "x"});
        let out = route_config_through_gateway(&raw, &models(&[], None), |_| {});
        assert_eq!(out["model"], json!("gpt:old"), "fallback ?? canonical");
    }

    #[test]
    fn gateway_urls_are_the_path_with_or_without_the_slash() {
        assert!(is_gateway_url(
            "http://host.docker.internal:5273/api/llm/v1"
        ));
        assert!(is_gateway_url(
            "http://host.docker.internal:5273/api/llm/v1/"
        ));
        assert!(is_gateway_url("  http://talaria-ui:3000/api/llm/v1 \n"));
        // A raw upstream the operator chose: not ours to migrate.
        assert!(!is_gateway_url("https://openrouter.ai/api/v1"));
        assert!(!is_gateway_url("http://litellm:4000/v1"));
    }

    #[test]
    fn env_lines_read_trimmed_and_upsert_in_place() {
        let f = "A=1\nLLM_BASE_URL = http://x/api/llm/v1  \n# comment\nB=2\n";
        assert_eq!(
            read_env_line(f, "LLM_BASE_URL"),
            None,
            "the regex is anchored on the bare key — 'LLM_BASE_URL =' (spaced) is not a line it owns"
        );
        assert_eq!(read_env_line(f, "A"), Some("1".into()));
        assert_eq!(read_env_line(f, "B"), Some("2".into()));
        assert_eq!(read_env_line(f, "C"), None);

        let out = upsert_env_line(f, "A", "9");
        assert_eq!(
            out,
            "A=9\nLLM_BASE_URL = http://x/api/llm/v1  \n# comment\nB=2\n"
        );

        // Appends after stripping trailing newlines — and an empty file gains a
        // LEADING newline, the exact TS template shape.
        assert_eq!(upsert_env_line("", "K", "v"), "\nK=v\n");
        assert_eq!(upsert_env_line("A=1\n\n\n", "K", "v"), "A=1\nK=v\n");
    }
}
