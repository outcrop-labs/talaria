// Federation: bring OUTSIDE agents into Talaria as first-class citizens. Reads
// a Hermes-format directory (agents.yaml roster + agents/<department>/SOUL.md
// + config.yaml), then creates each agent NATIVELY: Talaria def + v1, fresh
// gateway key, model targets mapped into the endpoint registry, skills copied
// into the fleet dir, run on the Talaria chassis with a fresh state volume.
// Nothing keeps pointing at the source directory afterwards — it's a one-way
// door, re-runnable (existing slugs are reported and skipped). Port of
// ui/src/server/fleet-federate.ts.

use serde_json::{json, Value};
use sqlx::PgPool;

use crate::agent_defs::{
    add_version_if_changed, dept_ok, slug_ok, upsert_agent_def, NewVersion, UpsertDef,
};
use crate::fleet_create::ensure_agent_key;
use crate::fleet_layout;
use crate::gateway::registry::{add_endpoint_models, ensure_endpoint};

#[derive(Debug, serde::Serialize)]
pub struct FederatedAgent {
    pub slug: String,
    pub model: String,
    pub status: &'static str, // 'federated' | 'exists'
}

#[derive(Debug, serde::Serialize)]
pub struct FederateResult {
    pub agents: Vec<FederatedAgent>,
    pub errors: Vec<String>,
}

/// `"${LLM_API_KEY}"` → `"LLM_API_KEY"`; literal keys are never stored
/// (keyEnvOf).
fn key_env_of(api_key: Option<&str>) -> Option<String> {
    let raw = api_key?.trim();
    let inner = raw
        .strip_prefix("${")
        .and_then(|s| s.strip_suffix('}'))?;
    (!inner.is_empty() && inner.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        .then(|| inner.to_string())
}

/// Endpoint identity + class for a model target. Local = the LAN inference
/// plane; proxies to cloud models and first-party providers = cloud
/// (endpointFor).
struct EndpointFor {
    name: String,
    class: &'static str,
    provider: String,
    base_url: Option<String>,
}

fn endpoint_for(block: &Value) -> EndpointFor {
    let provider = block
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("custom")
        .to_lowercase();
    // TS strips exactly ONE trailing slash.
    let base_url = block
        .get("base_url")
        .and_then(Value::as_str)
        .map(|b| b.strip_suffix('/').unwrap_or(b).to_string());
    if let Some(url) = &base_url {
        let host = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url);
        let host = host.split(['/', ':']).next().unwrap_or(host);
        let host = if host.is_empty() { url.as_str() } else { host };
        let is_local = (host.contains("inference-router")
            || host.contains("vllm")
            || host.contains("ollama")
            || host.contains("llama")
            || host.contains("localhost")
            || host.contains("127.0.0.1"))
            && !host.contains("litellm");
        return EndpointFor {
            name: host.to_string(),
            class: if is_local { "local" } else { "cloud" },
            provider,
            base_url: Some(url.clone()),
        };
    }
    EndpointFor {
        name: provider.clone(),
        class: "cloud",
        provider,
        base_url: None,
    }
}

/// One raw model block → a config target, ensuring its endpoint exists and
/// lists the model (targetOf). Numbers stay raw: yaml hands the same JS
/// Number() the TS read did.
fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

async fn target_of(pg: &PgPool, block: &Value) -> Result<Value, sqlx::Error> {
    let ep = endpoint_for(block);
    ensure_endpoint(
        pg,
        &ep.name,
        &ep.provider,
        ep.base_url.as_deref(),
        ep.class,
        key_env_of(block.get("api_key").and_then(Value::as_str)).as_deref(),
        match block.get("context_length") {
            Some(Value::Number(n)) => n.as_i64(),
            _ => None,
        },
    )
    .await?;
    if let Some(model) = block.get("model").and_then(Value::as_str) {
        add_endpoint_models(pg, &ep.name, &[model.to_string()]).await?;
    }
    let mut t = json!({
        "endpoint": ep.name,
        "model": block.get("model").and_then(Value::as_str).unwrap_or(""),
    });
    let ctx = num(block.get("context_length"));
    if ctx.is_some_and(|c| c.is_finite() && c > 0.0) {
        t["contextLength"] = json!(ctx.unwrap());
    }
    Ok(t)
}

/// federateFromDir — the whole one-way door. Never throws: every failure is
/// collected into `errors` (an unreadable roster aborts with one line).
pub async fn federate_from_dir(pg: &PgPool, dir: &str, actor: &str) -> FederateResult {
    let mut result = FederateResult {
        agents: Vec::new(),
        errors: Vec::new(),
    };
    let root = std::path::Path::new(dir);
    if !tokio::fs::metadata(root).await.map(|m| m.is_dir()).unwrap_or(false) {
        result.errors.push(format!("{dir}: not a directory on the server"));
        return result;
    }

    let roster_raw = match tokio::fs::read_to_string(root.join("agents.yaml")).await {
        Ok(raw) => raw,
        Err(e) => {
            result.errors.push(format!("agents.yaml: {e}"));
            return result;
        }
    };
    let doc: Value = match serde_yaml_ng::from_str(&roster_raw) {
        Ok(v) => v,
        Err(e) => {
            result.errors.push(format!("agents.yaml: {e}"));
            return result;
        }
    };
    // A top-level array, or {agents: [...]}.
    let empty: Vec<Value> = Vec::new();
    let roster: Vec<Value> = match &doc {
        Value::Array(items) => items.clone(),
        Value::Object(_) => doc
            .get("agents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or(empty),
        _ => empty,
    };

    for entry in &roster {
        let field = |k: &str| entry.get(k).and_then(Value::as_str).map(str::trim);
        let (Some(slug), Some(department)) = (field("slug"), field("department")) else {
            continue;
        };
        if slug.is_empty() || department.is_empty() {
            continue;
        }
        // Same alphabet the interactive path (createAgent) enforces, checked
        // BEFORE these strings reach an .env line (ensureAgentKey), a path
        // join, or a shell — an imported roster is third-party input, not
        // operator typing.
        if !slug_ok(slug) || !dept_ok(department) {
            result.errors.push(format!(
                "{}: slug must be lowercase alphanumeric and department lowercase-kebab (e.g. \"analyst\" / \"research\")",
                if slug.is_empty() { "(unnamed)" } else { slug }
            ));
            continue;
        }
        let r = federate_one(pg, root, dir, slug, department, entry, actor).await;
        match r {
            Ok(Some(a)) => result.agents.push(a),
            Ok(None) => {} // skipped silently — malformed entry, same as TS's `continue`
            Err(e) => result.errors.push(format!("{slug}: {e}")),
        }
    }
    result
}

/// One roster entry; Err carries `${slug}: message`'s inner text. Ok(None) =
/// the entry was skipped without an error line.
async fn federate_one(
    pg: &PgPool,
    root: &std::path::Path,
    dir: &str,
    slug: &str,
    department: &str,
    entry: &Value,
    actor: &str,
) -> Result<Option<FederatedAgent>, String> {
    let exists: Option<(i32,)> = sqlx::query_as("select 1 from agent_defs where slug = $1")
        .bind(slug)
        .fetch_optional(pg)
        .await
        .map_err(|e| e.to_string())?;
    if exists.is_some() {
        return Ok(Some(FederatedAgent {
            slug: slug.to_string(),
            model: String::new(),
            status: "exists",
        }));
    }

    let agent_dir = root.join("agents").join(department);
    let soul = tokio::fs::read_to_string(agent_dir.join("SOUL.md"))
        .await
        .unwrap_or_default();
    let raw_cfg_str = tokio::fs::read_to_string(agent_dir.join("config.yaml"))
        .await
        .unwrap_or_default();
    let raw_cfg: Value = if raw_cfg_str.trim().is_empty() {
        json!({})
    } else {
        serde_yaml_ng::from_str::<Value>(&raw_cfg_str).unwrap_or(json!({}))
    };

    // Model targets: main (object form only), aliases (object or
    // "provider/model" shorthand), fallbacks (array of objects).
    let main = match raw_cfg.get("model") {
        Some(m @ Value::Object(_)) => Some(target_of(pg, m).await.map_err(|e| e.to_string())?),
        _ => None,
    };
    let mut aliases: Vec<Value> = Vec::new();
    if let Some(Value::Object(map)) = raw_cfg.get("model_aliases") {
        for (name, v) in map {
            let t = match v {
                Value::String(s) => {
                    let (provider, rest) = s.split_once('/').unwrap_or((s, ""));
                    target_of(
                        pg,
                        &json!({ "provider": provider, "model": rest }),
                    )
                    .await
                    .map_err(|e| e.to_string())?
                }
                other => target_of(pg, other).await.map_err(|e| e.to_string())?,
            };
            let mut named = json!({ "name": name });
            if let (Some(t), Some(dst)) = (t.as_object(), named.as_object_mut()) {
                for (k, v) in t {
                    dst.insert(k.clone(), v.clone());
                }
            }
            aliases.push(named);
        }
    }
    let mut fallbacks: Vec<Value> = Vec::new();
    if let Some(items) = raw_cfg.get("fallback_providers").and_then(Value::as_array) {
        for f in items {
            fallbacks.push(target_of(pg, f).await.map_err(|e| e.to_string())?);
        }
    }
    let plugins: Vec<String> = raw_cfg
        .get("plugins")
        .and_then(|p| p.get("enabled"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let mcp_servers: Vec<String> = raw_cfg
        .get("mcp_servers")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    // Native creation: fresh key, source 'created' (fresh state volume in OUR
    // orchestration — provenance lives in the version note).
    ensure_agent_key(slug).await?;
    let display_name = entry
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .unwrap_or_else(|| {
            let mut c = slug.chars();
            match c.next() {
                Some(h) => h.to_uppercase().collect::<String>() + c.as_str(),
                None => slug.to_string(),
            }
        });
    let def = upsert_agent_def(
        pg,
        &UpsertDef {
            slug,
            department,
            display_name: &display_name,
            role: None,
            source: "created",
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query("update agent_defs set managed = true, updated_at = now() where id = $1::uuid")
        .bind(&def.id)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    let config = json!({
        "main": main,
        "aliases": aliases,
        "fallbacks": fallbacks,
        "plugins": plugins,
        "mcpServers": mcp_servers,
        "raw": raw_cfg,
    });
    add_version_if_changed(
        pg,
        &def.id,
        &NewVersion {
            soul: &soul,
            config: &config,
            note: Some(&format!("federated from {dir}")),
            created_by: Some(actor),
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    // Skills come WITH the agent, into Talaria's own roots — best-effort,
    // never overwriting (cp force:false).
    let src = agent_dir.join("skills");
    let dst = fleet_layout::fleet_dir().join("agents").join(slug).join("skills");
    let _ = copy_dir_new(&src, &dst).await;

    Ok(Some(FederatedAgent {
        slug: slug.to_string(),
        model: def.model.clone(),
        status: "federated",
    }))
}

/// cp recursive force:false errorOnExist:false — copy what isn't there yet,
/// ignore everything that goes wrong. Iterative (a worklist), so no async
/// recursion.
async fn copy_dir_new(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    let mut work: Vec<(std::path::PathBuf, std::path::PathBuf)> = vec![(src.into(), dst.into())];
    while let Some((from_dir, to_dir)) = work.pop() {
        let _ = tokio::fs::create_dir_all(&to_dir).await;
        let Ok(mut entries) = tokio::fs::read_dir(&from_dir).await else {
            continue;
        };
        while let Some(entry) = entries.next_entry().await? {
            let from = entry.path();
            let to = to_dir.join(entry.file_name());
            if entry.metadata().await.map(|m| m.is_dir()).unwrap_or(false) {
                work.push((from, to));
            } else if !to.exists() {
                let _ = tokio::fs::copy(&from, &to).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_env_extracts_only_env_refs() {
        assert_eq!(key_env_of(Some("${LLM_API_KEY}")), Some("LLM_API_KEY".into()));
        assert_eq!(key_env_of(Some("sk-live-key")), None, "literal keys never stored");
        assert_eq!(key_env_of(Some("${lower_case}")), None);
        assert_eq!(key_env_of(None), None);
    }

    #[test]
    fn endpoint_for_names_the_host_and_classes_it() {
        let local = endpoint_for(&serde_yaml_ng::from_str::<Value>("base_url: http://vllm.lan:8000/v1").unwrap());
        assert_eq!(local.name, "vllm.lan");
        assert_eq!(local.class, "local");
        let cloud = endpoint_for(&serde_yaml_ng::from_str::<Value>("provider: OpenAI").unwrap());
        assert_eq!(cloud.name, "openai");
        assert_eq!(cloud.class, "cloud");
        // litellm is a proxy to cloud models even though it says "llama"
        let proxy = endpoint_for(
            &serde_yaml_ng::from_str::<Value>("base_url: http://litellm.lan:4000/v1").unwrap(),
        );
        assert_eq!(proxy.class, "cloud");
        // exactly one trailing slash stripped
        assert_eq!(
            endpoint_for(&serde_yaml_ng::from_str::<Value>("base_url: https://api.x.ai/v1/").unwrap()).base_url,
            Some("https://api.x.ai/v1".into())
        );
    }
}
