// The per-agent gateway key — the vocabulary the fleet brain (which mints
// and scopes the keys) and the chat relay (which judges calls on them) share.
//
// Every agent container presents its OWN `tlk_` credential to the gateway,
// named `agent:<slug>`, so the relay can answer three questions no shared key
// could: WHICH agent is calling (the budget subject and the attribution
// namespace are that agent's model id — the same one chat-persist ledgers a
// persona turn under, so the spend of a chat turn and the spend of anything
// else the key does sum under one name), WHAT it may ask for (the model ids
// its rendered config.yaml routes to — a container that asks for anything
// else is refused before any upstream is touched), and whether its spend is
// already in the ledger (a persona turn's is; see llm_chat.rs).
//
// The scope lives in ONE app_settings row, `gateway_agent_models`, written
// by the fleet render and read through the hot window on every relay call:
//
//   { "<slug>": { "model": "<agent model id>", "models": ["<id>", …] }, … }
//
// A setting rather than a column: the key row carries the three numeric caps
// and nothing else, and the scope is render-derived state that changes only
// when a render does — the same reason the two key-list settings next to it
// are settings.

use crate::settings::get_setting_hot;
use serde_json::{Value, json};
use sqlx::PgPool;

/// The name prefix every per-agent gateway key carries.
pub const AGENT_KEY_PREFIX: &str = "agent:";
/// The settings row holding every agent key's scope.
pub const AGENT_MODELS_SETTING: &str = "gateway_agent_models";

/// The key name for an agent: `agent:<slug>`.
pub fn agent_key_name(slug: &str) -> String {
    format!("{AGENT_KEY_PREFIX}{slug}")
}

/// The slug a key name carries, when it is an agent's — `agent:x-eng` →
/// `x-eng`. Any other key (an operator's own, `fleet-gateway`,
/// `workbench-gateway`) answers None and keeps today's behaviour.
pub fn agent_slug_of_key(key_name: &str) -> Option<&str> {
    key_name
        .strip_prefix(AGENT_KEY_PREFIX)
        .filter(|s| !s.is_empty())
}

/// What one agent key is scoped to, as the last render wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentKeyScope {
    /// The agent's model id — the budget subject and the ledger namespace.
    pub model: String,
    /// The model ids its rendered config may ask the gateway for.
    pub models: Vec<String>,
}

/// Read one agent's scope out of the settings row. Pure over the row's JSON
/// so the relay's judgement can be pinned without a database.
pub fn scope_of(setting: &Value, slug: &str) -> Option<AgentKeyScope> {
    let entry = setting.get(slug)?;
    let model = entry.get("model")?.as_str()?.to_string();
    let models = entry
        .get("models")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Some(AgentKeyScope { model, models })
}

/// The scope for an agent key, through the hot window. None when the last
/// render did not scope this slug — a disabled agent, or a render older
/// than this row. The caller decides what an unscoped key may do.
pub async fn agent_key_scope(pg: &PgPool, slug: &str) -> Option<AgentKeyScope> {
    let setting = get_setting_hot(pg, AGENT_MODELS_SETTING, json!({})).await;
    scope_of(&setting, slug)
}

/// May this scope ask for `model`? Exact match on the rendered id — the
/// config.yaml asks by that string and nothing else — and an EMPTY rendered
/// set refuses everything: a config with no model spec has no business at
/// the gateway, and refusing is how a render bug surfaces instead of
/// widening to the whole catalog.
pub fn model_in_scope(scope: &AgentKeyScope, model: &str) -> bool {
    scope.models.iter().any(|m| m == model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_round_trip_and_other_keys_are_not_agents() {
        assert_eq!(agent_key_name("x-eng"), "agent:x-eng");
        assert_eq!(agent_slug_of_key("agent:x-eng"), Some("x-eng"));
        assert_eq!(
            agent_slug_of_key("agent:"),
            None,
            "a prefix alone names nobody"
        );
        assert_eq!(agent_slug_of_key("fleet-gateway"), None);
        assert_eq!(agent_slug_of_key("workbench-gateway"), None);
        assert_eq!(
            agent_slug_of_key("my agent: key"),
            None,
            "anchored at the front"
        );
    }

    #[test]
    fn a_scope_reads_its_row_and_judges_models_exactly() {
        let setting = json!({
            "analyst": { "model": "analyst", "models": ["qwen3:14b", "z-ai/glm-5.2"] },
            "bare": { "model": "bare-model" },
            "broken": { "models": ["x"] }
        });
        let s = scope_of(&setting, "analyst").unwrap();
        assert_eq!(s.model, "analyst");
        assert!(model_in_scope(&s, "qwen3:14b"));
        assert!(model_in_scope(&s, "z-ai/glm-5.2"));
        assert!(
            !model_in_scope(&s, "qwen3"),
            "no prefix or pool matching — the exact rendered id"
        );
        assert!(!model_in_scope(&s, "openrouter/qwen3:14b"));
        // No models list: a scope with nothing in it refuses everything.
        let bare = scope_of(&setting, "bare").unwrap();
        assert_eq!(bare.models, Vec::<String>::new());
        assert!(!model_in_scope(&bare, "qwen3:14b"));
        // No model id: not a scope at all.
        assert_eq!(scope_of(&setting, "broken"), None);
        assert_eq!(scope_of(&setting, "unknown"), None);
        assert_eq!(
            scope_of(&json!([]), "analyst"),
            None,
            "a non-object row scopes nobody"
        );
    }
}
