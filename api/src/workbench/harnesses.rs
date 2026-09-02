// The harness REGISTRY — the extension point developers build against. A
// harness is a declarative WorkbenchHarnessDefinition (see @talaria/sdk
// defineWorkbenchHarness) — NOT the activity harness in harness/, which is a
// prompt plus an output schema run by `run_harness`. Two contracts: how it
// authenticates, how it's invoked (structured output first), how it
// serves/consumes MCP, and what a driving agent should understand about it.
//
// Port of the REGISTRY READ of ui/src/server/workbench-harnesses.ts: the
// builtin definitions, the merged registry (builtin < admin-custom, by slug),
// and the auth-derived fullEnv the render interpolates into containers. Two
// layers of the TS registry are out of scope by construction and named here:
//   • app-shipped (apps/<slug>/harness.ts) — a compiled TS module the Rust
//     process cannot load; no repo app ships one today, and app-published
//     surfaces carry the same boundary in mcp_registry (loud error at
//     dispatch, never a silent miss). When the app runtime crosses, its
//     harnesses join this registry.
//   • renderMcpConfig / format:'custom' — admin JSON can't carry functions in
//     TS either, so 'custom' is app-shipped-only and unreachable from the
//     layers this port serves.
// The admin upsert/delete and the effort→model chain cross here with the
// /api/workbench/harnesses routes and the workbench MCP's start_job.

use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::PgPool;

/// How a harness authenticates: Talaria's gateway (OpenAI-compatible, metered,
/// attributed), or a named provider whose key the org's endpoint registry
/// provisions into an env var the harness reads. The wire shape is the literal
/// string `"gateway"` or `{ provider, envVar }` — a union serde's untagged
/// derive cannot spell (a unit variant only matches null), so it is written
/// out by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessAuth {
    Gateway,
    Provider { provider: String, env_var: String },
}

const GATEWAY: &str = "gateway";

impl serde::Serialize for HarnessAuth {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            HarnessAuth::Gateway => s.serialize_str(GATEWAY),
            HarnessAuth::Provider { provider, env_var } => {
                let mut m = Map::new();
                m.insert("provider".into(), Value::String(provider.clone()));
                m.insert("envVar".into(), Value::String(env_var.clone()));
                Value::Object(m).serialize(s)
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for HarnessAuth {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match &v {
            Value::String(s) if s == GATEWAY => Ok(HarnessAuth::Gateway),
            Value::Object(o) => Ok(HarnessAuth::Provider {
                provider: o
                    .get("provider")
                    .and_then(Value::as_str)
                    .ok_or_else(|| serde::de::Error::missing_field("provider"))?
                    .to_string(),
                env_var: o
                    .get("envVar")
                    .and_then(Value::as_str)
                    .ok_or_else(|| serde::de::Error::missing_field("envVar"))?
                    .to_string(),
            }),
            _ => Err(serde::de::Error::custom(
                "auth must be \"gateway\" or { provider, envVar }",
            )),
        }
    }
}

/// MCP pass-through config the harness reads: written per agent in this format
/// at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpConfigFormat {
    /// .mcp.json-style (Claude Code).
    ClaudeJson,
    /// opencode config.
    OpencodeJson,
    /// The app-shipped harness's own renderer — unreachable from builtin/custom
    /// layers (functions can't ride JSON); modeled so the DB shape round-trips.
    Custom,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    pub format: McpConfigFormat,
    pub filename: String,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServe {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// The declarative harness definition — @talaria/sdk WorkbenchHarnessDefinition.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchHarnessDef {
    /// Stable id — what profiles and per-agent picks reference.
    pub slug: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    pub auth: HarnessAuth,
    /// Extra container env (compose-interpolated; merged over the auth env).
    #[serde(default)]
    pub env: Option<Map<String, Value>>,
    /// Prefix model ids need for this harness's CLI (e.g. "openai/").
    #[serde(default)]
    pub model_prefix: Option<String>,
    /// Invocation template — <model> and <task> placeholders.
    pub invoke: String,
    /// Structured-output form — REQUIRED for good drivers; agents are taught
    /// to read structured results, never scrape logs.
    #[serde(default)]
    pub json_invoke: Option<String>,
    /// How to run the harness AS an MCP server (stdio) — the preferred
    /// integration: agents drive it with tools.
    #[serde(default)]
    pub mcp_serve: Option<McpServe>,
    #[serde(default)]
    pub mcp_config: Option<McpConfig>,
    /// A cheap command that proves the harness runs in a sandbox (version
    /// check) — surfaced by the workbench doctor for agents to self-verify.
    #[serde(default)]
    pub probe: Option<String>,
    /// What a driving agent should understand: sessions, resume, results.
    pub guide: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessSource {
    Builtin,
    Custom,
}

/// A definition plus its resolved layer and full container env.
#[derive(Debug, Clone)]
pub struct ResolvedHarness {
    pub def: WorkbenchHarnessDef,
    /// The definition as THIS registry answers it — the TS literal's own key
    /// order (each builtin spells its fields differently; JSON.stringify
    /// prints the literal, not the interface), which is why it is carried
    /// rather than re-derived from the struct.
    pub wire_def: Value,
    pub source: HarnessSource,
    /// Full container env: auth-derived + definition env.
    pub full_env: Map<String, Value>,
}

impl ResolvedHarness {
    /// resolveDef's spread: the definition, then `source`, then `fullEnv`
    /// appended after the definition's own keys.
    pub fn wire(&self) -> Value {
        let source = match self.source {
            HarnessSource::Builtin => "builtin",
            HarnessSource::Custom => "custom",
        };
        let mut out = self.wire_def.as_object().cloned().unwrap_or_default();
        out.insert("source".into(), Value::String(source.into()));
        out.insert("fullEnv".into(), Value::Object(self.full_env.clone()));
        Value::Object(out)
    }
}

// Gateway-auth harnesses reach the same org gateway the personas do, but on
// their OWN credential (`workbench-gateway`, minted into the fleet .env as
// LLM_WORKBENCH_API_KEY by fleet-brain). They must: a harness run is spend NO
// Talaria flow ever sees — no chat/channel/ticket row is written for it — so
// the gateway is the only place it can enter the ledger, while the personas'
// key has to stay unmetered there because the flow that drove the turn already
// writes its row. One shared credential can only be one of those two things,
// so harness runs were either invisible or doubled every persona turn. The
// `:-` fallback keeps an operator-overridden fleet (LLM_BASE_URL pointed at a
// raw upstream, so no gateway brain is provisioned) on the key it configured.
pub(crate) fn gateway_env() -> Map<String, Value> {
    [
        ("OPENAI_BASE_URL", "${LLM_BASE_URL}"),
        ("OPENAI_API_KEY", "${LLM_WORKBENCH_API_KEY:-${LLM_API_KEY}}"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::String(v.into())))
    .collect()
}

fn resolve_def(wire: Value, source: HarnessSource) -> Option<ResolvedHarness> {
    let def: WorkbenchHarnessDef = serde_json::from_value(wire.clone()).ok()?;
    let mut full_env = match &def.auth {
        HarnessAuth::Gateway => gateway_env(),
        HarnessAuth::Provider { .. } => Map::new(),
    };
    if let Some(env) = &def.env {
        for (k, v) in env {
            full_env.insert(k.clone(), v.clone());
        }
    }
    Some(ResolvedHarness {
        def,
        wire_def: wire,
        source,
        full_env,
    })
}

/// A minimal definition wire for the tests (test-only: the real builtin
/// wires live in `builtin_wires`, and no production path constructs one).
#[cfg(test)]
fn def(slug: &str, label: &str, auth: HarnessAuth, invoke: &str, guide: &str) -> Value {
    let auth_wire = match auth {
        HarnessAuth::Gateway => serde_json::json!("gateway"),
        HarnessAuth::Provider { provider, env_var } => {
            serde_json::json!({ "provider": provider, "envVar": env_var })
        }
    };
    serde_json::json!({
        "slug": slug,
        "label": label,
        "auth": auth_wire,
        "invoke": invoke,
        "guide": guide,
    })
}

/// The merged registry — builtin < admin-custom, by slug (later wins).
pub async fn list_harness_defs(pg: &PgPool) -> Result<Vec<ResolvedHarness>, sqlx::Error> {
    let mut by_slug: Vec<(String, ResolvedHarness)> = Vec::new();
    let mut put = |r: ResolvedHarness| {
        by_slug.retain(|(slug, _)| *slug != r.def.slug);
        by_slug.push((r.def.slug.clone(), r));
    };
    for wire in builtin_wires() {
        if let Some(r) = resolve_def(wire, HarnessSource::Builtin) {
            put(r);
        }
    }
    // Admin-custom definitions (declarative only — no code runs from these):
    // invoke + guide are the contract; a row missing either is skipped, not
    // fatal, matching TS's silent `if (def.invoke && def.guide)`. The stored
    // definition rides as-is (its own key order), with the row's slug
    // overriding in place.
    let rows: Vec<(String, Value)> =
        sqlx::query_as("select slug, definition from workbench_harness_defs where enabled")
            .fetch_all(pg)
            .await?;
    for (slug, definition) in rows {
        let Ok(check) = serde_json::from_value::<WorkbenchHarnessDef>(definition.clone()) else {
            continue;
        };
        if check.invoke.is_empty() || check.guide.is_empty() {
            continue;
        }
        let Some(obj) = definition.as_object() else {
            continue;
        };
        let mut wire = obj.clone();
        wire.insert("slug".into(), Value::String(slug.clone()));
        if let Some(r) = resolve_def(Value::Object(wire), HarnessSource::Custom) {
            put(r);
        }
    }
    Ok(by_slug.into_iter().map(|(_, r)| r).collect())
}

/// Admin-custom definitions (declarative only — no code runs from these).
/// The stored JSON is the zod-parsed body, which zod emits in SCHEMA shape
/// order — the same order for every custom row, absent keys dropped.
pub async fn upsert_custom_harness(
    pg: &PgPool,
    slug: &str,
    definition: &Value,
    created_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into workbench_harness_defs (slug, definition, created_by) \
         values ($1, $2, $3) \
         on conflict (slug) do update set definition = excluded.definition, \
           enabled = true, updated_at = now()",
    )
    .bind(slug)
    .bind(definition)
    .bind(created_by)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn delete_custom_harness(pg: &PgPool, slug: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from workbench_harness_defs where slug = $1")
        .bind(slug)
        .execute(pg)
        .await?;
    Ok(())
}

/// The model id as THIS harness's CLI expects it.
pub fn harness_model_arg(h: &WorkbenchHarnessDef, model: &str) -> String {
    format!("{}{}", h.model_prefix.as_deref().unwrap_or(""), model)
}

/// The four builtin definitions, as the TS literals spell them — one json!
/// per harness because JSON.stringify prints each literal's OWN key order,
/// and they differ (opencode carries env+modelPrefix early; claude-code
/// interleaves mcpServe before probe). The struct is derived from these, so
/// there is exactly one place a builtin is spelled.
fn builtin_wires() -> Vec<Value> {
    vec![
        serde_json::json!({
            "slug": "opencode",
            "label": "opencode",
            "auth": "gateway",
            "env": { "OPENCODE_CONFIG": "/opt/workbench-config/opencode.json" },
            "modelPrefix": "openai/",
            "invoke": "npx -y opencode-ai run --model <model> \"<task>\"",
            "jsonInvoke": "npx -y opencode-ai run --model <model> --format json \"<task>\"",
            "probe": "npx -y opencode-ai --version",
            "mcpConfig": { "format": "opencode-json", "filename": "opencode.json" },
            "guide": "opencode is session-based: each run continues a project session. Use --format json and read the structured result (message, file edits) instead of scraping text. It reads OPENCODE_CONFIG for MCP servers — your Talaria tools are already wired in there.",
        }),
        serde_json::json!({
            "slug": "claude-code",
            "label": "Claude Code",
            "auth": { "provider": "anthropic", "envVar": "ANTHROPIC_API_KEY" },
            "invoke": "npx -y @anthropic-ai/claude-code -p \"<task>\" --model <model> --mcp-config /opt/workbench-config/mcp.json",
            "jsonInvoke": "npx -y @anthropic-ai/claude-code -p \"<task>\" --model <model> --output-format json --mcp-config /opt/workbench-config/mcp.json",
            "mcpServe": { "command": "npx", "args": ["-y", "@anthropic-ai/claude-code", "mcp", "serve"] },
            "probe": "npx -y @anthropic-ai/claude-code --version",
            "mcpConfig": { "format": "claude-json", "filename": "mcp.json" },
            "guide": "Claude Code returns a structured result with --output-format json (result text, cost, session_id) — resume a session with --resume <session_id> to ask follow-ups instead of restarting. Prefer its MCP tools when registered on your config; otherwise use the JSON form and read the result object, never raw logs.",
        }),
        serde_json::json!({
            "slug": "oh-my-pi",
            "label": "Oh My Pi",
            "auth": "gateway",
            "invoke": "npx -y @oh-my-pi/pi-coding-agent \"<task>\" --model <model>",
            "probe": "npx -y @oh-my-pi/pi-coding-agent --version",
            "guide": "Oh My Pi (omp) is a full terminal coding agent — hash-anchored edits, LSP, subagents, a browser. It inherits MCP/rules config from .claude/.codex-style dirs in the workspace, so your pass-through config reaches it via the repo. No first-party MCP server mode yet: drive it one-shot per task and verify its work yourself with git diff and tests.",
        }),
        serde_json::json!({
            "slug": "codex",
            "label": "Codex CLI",
            "auth": "gateway",
            "invoke": "npx -y @openai/codex exec --model <model> \"<task>\"",
            "jsonInvoke": "npx -y @openai/codex exec --model <model> --json \"<task>\"",
            "mcpServe": { "command": "npx", "args": ["-y", "@openai/codex", "mcp"] },
            "probe": "npx -y @openai/codex --version",
            "guide": "Codex exec emits JSON events with --json — read the final result event. As an MCP server (codex mcp) it exposes tool-driven sessions; prefer that when registered on your config.",
        }),
    ]
}

// ── Effort → model (the platform's call, never the agent's) ──────────────────

/// Effort → model: per-agent override first, then the global roles with a
/// fall-down chain so unset slots never strand work. A DB failure inside the
/// chain is TS's thrown error — the caller surfaces it as a thrown tool
/// error, not a resolved null.
pub async fn effort_model(
    pg: &PgPool,
    effort: &str,
    overrides: Option<&Map<String, Value>>,
) -> Result<Option<String>, String> {
    async fn resolves(pg: &PgPool, model: &str) -> Result<bool, String> {
        Ok(crate::gateway::registry::resolve_route(pg, model)
            .await
            .map_err(|e| format!("route resolve: {e}"))?
            .is_some())
    }
    let own = || {
        overrides
            .and_then(|o| o.get(effort))
            .and_then(Value::as_str)
    };
    if let Some(model) = own()
        && resolves(pg, model).await?
    {
        return Ok(Some(model.to_string()));
    }
    let order: &[&str] = match effort {
        "heavy" => &["heavy", "standard", "light"],
        "standard" => &["standard", "light"],
        _ => &["light"],
    };
    for e in order {
        if let Some(model) = overrides.and_then(|o| o.get(*e)).and_then(Value::as_str)
            && resolves(pg, model).await?
        {
            return Ok(Some(model.to_string()));
        }
        if let Some(m) = crate::model::roles::resolve_role_model(pg, &format!("code-{e}"))
            .await
            .map_err(|err| format!("role resolve: {err}"))?
        {
            return Ok(Some(m));
        }
    }
    if let Some(utility) = crate::model::roles::resolve_role_model(pg, "utility")
        .await
        .map_err(|err| format!("role resolve: {err}"))?
    {
        return Ok(Some(utility));
    }
    let env_model = std::env::var("TALARIA_COPILOT_MODEL").ok();
    for m in [env_model, Some("pl-main".to_string())] {
        let Some(m) = m else { continue };
        if resolves(pg, &m).await? {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// All three resolved at once — start_job hands the agent the full map so it
/// sees its options in effort terms, never raw catalog spelunking. Object
/// key order is the TS literal's: light, standard, heavy.
pub async fn effort_models(
    pg: &PgPool,
    overrides: Option<&Map<String, Value>>,
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for effort in ["light", "standard", "heavy"] {
        let model = effort_model(pg, effort, overrides).await?;
        out.insert(
            effort.into(),
            model.map(Value::String).unwrap_or(Value::Null),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_harnesses_carry_the_workbench_key_fallback() {
        let r = resolve_def(
            def("x", "X", HarnessAuth::Gateway, "run", "g"),
            HarnessSource::Builtin,
        )
        .unwrap();
        assert_eq!(
            r.full_env.get("OPENAI_API_KEY").and_then(Value::as_str),
            Some("${LLM_WORKBENCH_API_KEY:-${LLM_API_KEY}}")
        );
        assert_eq!(
            r.full_env.get("OPENAI_BASE_URL").and_then(Value::as_str),
            Some("${LLM_BASE_URL}")
        );
    }

    #[test]
    fn definition_env_merges_over_the_auth_env() {
        let mut d = def("x", "X", HarnessAuth::Gateway, "run", "g");
        d.as_object_mut().unwrap().insert(
            "env".into(),
            serde_json::json!({ "OPENAI_BASE_URL": "overridden" }),
        );
        let r = resolve_def(d, HarnessSource::Builtin).unwrap();
        assert_eq!(
            r.full_env.get("OPENAI_BASE_URL").and_then(Value::as_str),
            Some("overridden"),
            "the definition's env wins over the auth-derived default, like TS's spread order"
        );
        // A native-auth harness gets NO gateway env at all.
        let native = resolve_def(
            def(
                "c",
                "C",
                HarnessAuth::Provider {
                    provider: "anthropic".into(),
                    env_var: "ANTHROPIC_API_KEY".into(),
                },
                "run",
                "g",
            ),
            HarnessSource::Builtin,
        )
        .unwrap();
        assert!(native.full_env.is_empty());
    }

    #[test]
    fn the_model_arg_carries_the_cli_prefix() {
        let mut d = def("opencode", "opencode", HarnessAuth::Gateway, "run", "g");
        d.as_object_mut()
            .unwrap()
            .insert("modelPrefix".into(), serde_json::json!("openai/"));
        let prefixed: WorkbenchHarnessDef = serde_json::from_value(d).unwrap();
        assert_eq!(
            harness_model_arg(&prefixed, "qwen3:14b"),
            "openai/qwen3:14b"
        );
        let plain: WorkbenchHarnessDef =
            serde_json::from_value(def("codex", "Codex CLI", HarnessAuth::Gateway, "r", "g"))
                .unwrap();
        assert_eq!(harness_model_arg(&plain, "m"), "m");
    }

    #[test]
    fn the_builtins_parse_from_their_own_wire_shape() {
        // Round-trip the literal wires through the struct a
        // workbench_harness_defs row would be validated with, proving the
        // serde shape matches what the admin surface writes.
        for wire in builtin_wires() {
            let back: WorkbenchHarnessDef = serde_json::from_value(wire.clone())
                .unwrap_or_else(|e| panic!("builtin {} failed to parse: {e}", wire["slug"]));
            assert_eq!(&back.slug, wire["slug"].as_str().unwrap());
        }
    }

    #[test]
    fn the_builtin_wires_spell_the_ts_literals_key_order() {
        // JSON.stringify prints each TS literal's OWN key order, and the
        // harnesses route answers with these wires — so the orders are the
        // contract. One assertion per builtin, keys in the literal's order.
        let wires = builtin_wires();
        fn keys(v: &Value) -> Vec<&str> {
            v.as_object()
                .unwrap()
                .keys()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
        }
        assert_eq!(
            keys(&wires[0]),
            vec![
                "slug",
                "label",
                "auth",
                "env",
                "modelPrefix",
                "invoke",
                "jsonInvoke",
                "probe",
                "mcpConfig",
                "guide"
            ]
        );
        assert_eq!(
            keys(&wires[1]),
            vec![
                "slug",
                "label",
                "auth",
                "invoke",
                "jsonInvoke",
                "mcpServe",
                "probe",
                "mcpConfig",
                "guide"
            ]
        );
        assert_eq!(
            keys(&wires[2]),
            vec!["slug", "label", "auth", "invoke", "probe", "guide"]
        );
        assert_eq!(
            keys(&wires[3]),
            vec![
                "slug",
                "label",
                "auth",
                "invoke",
                "jsonInvoke",
                "mcpServe",
                "probe",
                "guide"
            ]
        );
    }

    #[test]
    fn the_wire_appends_source_and_fullenv_after_the_definitions_keys() {
        let r = resolve_def(
            def("x", "X", HarnessAuth::Gateway, "run", "g"),
            HarnessSource::Custom,
        )
        .unwrap();
        let wire = r.wire();
        let keys: Vec<&str> = wire
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "slug", "label", "auth", "invoke", "guide", "source", "fullEnv"
            ]
        );
        assert_eq!(wire["source"], serde_json::json!("custom"));
        assert_eq!(
            wire["fullEnv"]["OPENAI_BASE_URL"],
            serde_json::json!("${LLM_BASE_URL}")
        );
    }
}
