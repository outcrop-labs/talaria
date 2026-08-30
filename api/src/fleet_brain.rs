// The fleet's brain is Talaria's own gateway. Rather than pointing agents at a
// raw LLM endpoint, the whole fleet routes its tool loop through Talaria's org
// gateway (`/api/llm/v1`) — so every agent call is guarded (confab checks see
// the full tool trace), metered in one ledger, and observable in one place.
//
// Port of the ROUTING half of ui/src/server/fleet-brain.ts: the model-set
// question and the config transform. The provisioning half (ensureGatewayBrain
// — minting the fleet-gateway/workbench-gateway keys into the fleet .env and
// the two gateway settings) is key-plane work and lands with the env writers.

use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashSet;

use crate::gateway::registry::list_endpoints;

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
}
