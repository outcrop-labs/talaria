// Per-agent MCP servers, read from and written to the versioned Hermes config
// (raw.mcp_servers) — port of ui/src/server/agent-mcp.ts. Edits ride the same
// machinery as model edits: a NEW immutable version, optionally applied
// (re-render + restart). Untouched entries pass through byte-for-byte —
// headers, tool filters, timeouts all survive.

use serde_json::{Map, Value};
use sqlx::PgPool;

use crate::mcp::service::mcp_fleet_url;

/// One server entry as the roster answers it.
#[derive(Debug, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub url: String,
    pub timeout: Option<i64>,
    /// Extra config (headers, tools include/exclude) present in the raw entry.
    pub extras: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentMcp {
    pub id: String,
    pub slug: String,
    pub display_name: String,
    pub managed: bool,
    pub servers: Vec<McpServerEntry>,
}

fn entry(name: &str, raw: &Map<String, Value>) -> McpServerEntry {
    McpServerEntry {
        name: name.to_string(),
        url: raw
            .get("url")
            .map(crate::body::js_string)
            .unwrap_or_default(),
        // `typeof entry.timeout === 'number'` — a JSON number, nothing else.
        timeout: raw.get("timeout").and_then(Value::as_i64),
        extras: raw
            .keys()
            .filter(|k| k.as_str() != "url" && k.as_str() != "timeout")
            .cloned()
            .collect(),
    }
}

/// The roster read (mcp.ts + the fleet-defs console): the agent's own config
/// version's servers, with the Talaria toolkit surfaced first for managed
/// agents (it's injected at RENDER time, not stored in the version).
pub async fn list_agent_mcp(pg: &PgPool) -> Result<Vec<AgentMcp>, sqlx::Error> {
    let rows: Vec<(String, String, String, bool, Option<Value>)> = sqlx::query_as(
        "select d.id::text, d.slug, d.display_name, d.managed, v.config \
         from agent_defs d \
         left join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.enabled order by d.slug",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, slug, display_name, managed, config)| {
            let raw = config
                .as_ref()
                .and_then(|c| c.get("raw"))
                .and_then(Value::as_object)
                .and_then(|r| r.get("mcp_servers"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let mut servers = Vec::new();
            if managed {
                servers.push(McpServerEntry {
                    name: "talaria".into(),
                    url: mcp_fleet_url(),
                    timeout: None,
                    extras: vec!["built-in".into()],
                });
            }
            for (name, e) in &raw {
                if name == "talaria" {
                    continue;
                }
                if let Some(e) = e.as_object() {
                    servers.push(entry(name, e));
                }
            }
            AgentMcp {
                id,
                slug,
                display_name,
                managed,
                servers,
            }
        })
        .collect())
}

/// Merge add/remove onto the previous config's raw.mcp_servers
/// (applyMcpEdits). Added entries get the agent's X-Agent-Name header — the
/// fleet's convention. The Map is insertion-ordered like a JS object, so the
/// merged key order (removed keys dropped in place, re-added keys keeping
/// their original position, new keys appended) matches TS exactly.
pub fn apply_mcp_edits(prev: &Value, slug: &str, add: &[Value], remove: &[String]) -> Value {
    let mut servers: Map<String, Value> = prev
        .get("raw")
        .and_then(Value::as_object)
        .and_then(|r| r.get("mcp_servers"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for name in remove {
        servers.remove(name);
    }
    for s in add {
        let name = s.get("name").and_then(Value::as_str).unwrap_or_default();
        let mut next = Map::new();
        next.insert(
            "url".into(),
            s.get("url")
                .cloned()
                .unwrap_or(Value::String(String::new())),
        );
        next.insert(
            "headers".into(),
            serde_json::json!({ "X-Agent-Name": slug }),
        );
        // `...(s.timeout ? { timeout } : {})` — truthy: 0 and undefined both
        // omit the field.
        if let Some(t) = s.get("timeout").and_then(Value::as_i64)
            && t != 0
        {
            next.insert("timeout".into(), Value::from(t));
        }
        servers.insert(name.to_string(), Value::Object(next));
    }
    let mut raw = prev
        .get("raw")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    raw.insert("mcp_servers".into(), Value::Object(servers.clone()));
    let mut config = prev.as_object().cloned().unwrap_or_default();
    let names = servers.keys().cloned().collect::<Vec<_>>();
    config.insert(
        "mcpServers".into(),
        Value::Array(names.into_iter().map(Value::String).collect()),
    );
    config.insert("raw".into(), Value::Object(raw));
    Value::Object(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edits_merge_onto_the_raw_config() {
        let prev = json!({
            "model": "gpt",
            "mcpServers": ["exa", "linear"],
            "raw": {
                "mcp_servers": {
                    "exa": { "url": "https://exa", "headers": { "X": "1" } },
                    "linear": { "url": "https://linear", "timeout": 30 }
                }
            }
        });
        let next = apply_mcp_edits(
            &prev,
            "gus-research",
            &[json!({ "name": "fresh", "url": "https://f", "timeout": 5 })],
            &["linear".into()],
        );
        // Untouched entries pass through byte-for-byte.
        assert_eq!(
            next["raw"]["mcp_servers"]["exa"],
            json!({"url": "https://exa", "headers": {"X": "1"}})
        );
        assert!(next["raw"]["mcp_servers"].get("linear").is_none());
        let fresh = next["raw"]["mcp_servers"]["fresh"].as_object().unwrap();
        assert_eq!(fresh["url"], "https://f");
        assert_eq!(fresh["headers"], json!({ "X-Agent-Name": "gus-research" }));
        assert_eq!(fresh["timeout"], 5);
        // mcpServers follows the merged key order; raw's other keys survive.
        assert_eq!(
            next["mcpServers"],
            json!(["exa", "fresh"]),
            "removed in place, added appended — JS object order"
        );
        assert_eq!(next["model"], "gpt");
    }
}
