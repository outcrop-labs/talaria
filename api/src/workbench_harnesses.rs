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
// The admin upsert/delete routes and the effort→model resolution cross with
// the routes that own them.

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
    pub source: HarnessSource,
    /// Full container env: auth-derived + definition env.
    pub full_env: Map<String, Value>,
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

fn resolve_def(def: WorkbenchHarnessDef, source: HarnessSource) -> ResolvedHarness {
    let mut full_env = match &def.auth {
        HarnessAuth::Gateway => gateway_env(),
        HarnessAuth::Provider { .. } => Map::new(),
    };
    if let Some(env) = &def.env {
        for (k, v) in env {
            full_env.insert(k.clone(), v.clone());
        }
    }
    ResolvedHarness {
        def,
        source,
        full_env,
    }
}

fn def(
    slug: &str,
    label: &str,
    auth: HarnessAuth,
    invoke: &str,
    guide: &str,
) -> WorkbenchHarnessDef {
    WorkbenchHarnessDef {
        slug: slug.into(),
        label: label.into(),
        description: None,
        auth,
        env: None,
        model_prefix: None,
        invoke: invoke.into(),
        json_invoke: None,
        mcp_serve: None,
        mcp_config: None,
        probe: None,
        guide: guide.into(),
    }
}

/// The merged registry — builtin < admin-custom, by slug (later wins).
pub async fn list_harness_defs(pg: &PgPool) -> Result<Vec<ResolvedHarness>, sqlx::Error> {
    let mut by_slug: Vec<(String, ResolvedHarness)> = Vec::new();
    let mut put = |r: ResolvedHarness| {
        by_slug.retain(|(slug, _)| *slug != r.def.slug);
        by_slug.push((r.def.slug.clone(), r));
    };
    for b in builtins() {
        put(resolve_def(b, HarnessSource::Builtin));
    }
    // Admin-custom definitions (declarative only — no code runs from these):
    // invoke + guide are the contract; a row missing either is skipped, not
    // fatal, matching TS's silent `if (def.invoke && def.guide)`.
    let rows: Vec<(String, Value)> =
        sqlx::query_as("select slug, definition from workbench_harness_defs where enabled")
            .fetch_all(pg)
            .await?;
    for (slug, definition) in rows {
        let Ok(mut d) = serde_json::from_value::<WorkbenchHarnessDef>(definition) else {
            continue;
        };
        if d.invoke.is_empty() || d.guide.is_empty() {
            continue;
        }
        d.slug = slug;
        put(resolve_def(d, HarnessSource::Custom));
    }
    Ok(by_slug.into_iter().map(|(_, r)| r).collect())
}

/// The model id as THIS harness's CLI expects it.
pub fn harness_model_arg(h: &WorkbenchHarnessDef, model: &str) -> String {
    format!("{}{}", h.model_prefix.as_deref().unwrap_or(""), model)
}

fn builtins() -> Vec<WorkbenchHarnessDef> {
    let mut opencode = def(
        "opencode",
        "opencode",
        HarnessAuth::Gateway,
        "npx -y opencode-ai run --model <model> \"<task>\"",
        "opencode is session-based: each run continues a project session. Use --format json and read the structured result (message, file edits) instead of scraping text. It reads OPENCODE_CONFIG for MCP servers — your Talaria tools are already wired in there.",
    );
    opencode.env = Some(
        [("OPENCODE_CONFIG", "/opt/workbench-config/opencode.json")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.into())))
            .collect(),
    );
    opencode.model_prefix = Some("openai/".into());
    opencode.json_invoke =
        Some("npx -y opencode-ai run --model <model> --format json \"<task>\"".into());
    opencode.probe = Some("npx -y opencode-ai --version".into());
    opencode.mcp_config = Some(McpConfig {
        format: McpConfigFormat::OpencodeJson,
        filename: "opencode.json".into(),
    });

    let mut claude = def(
        "claude-code",
        "Claude Code",
        HarnessAuth::Provider {
            provider: "anthropic".into(),
            env_var: "ANTHROPIC_API_KEY".into(),
        },
        "npx -y @anthropic-ai/claude-code -p \"<task>\" --model <model> --mcp-config /opt/workbench-config/mcp.json",
        "Claude Code returns a structured result with --output-format json (result text, cost, session_id) — resume a session with --resume <session_id> to ask follow-ups instead of restarting. Prefer its MCP tools when registered on your config; otherwise use the JSON form and read the result object, never raw logs.",
    );
    claude.json_invoke = Some(
        "npx -y @anthropic-ai/claude-code -p \"<task>\" --model <model> --output-format json --mcp-config /opt/workbench-config/mcp.json"
            .into(),
    );
    claude.mcp_serve = Some(McpServe {
        command: "npx".into(),
        args: vec![
            "-y".into(),
            "@anthropic-ai/claude-code".into(),
            "mcp".into(),
            "serve".into(),
        ],
    });
    claude.probe = Some("npx -y @anthropic-ai/claude-code --version".into());
    claude.mcp_config = Some(McpConfig {
        format: McpConfigFormat::ClaudeJson,
        filename: "mcp.json".into(),
    });

    let oh_my_pi = def(
        "oh-my-pi",
        "Oh My Pi",
        HarnessAuth::Gateway,
        "npx -y @oh-my-pi/pi-coding-agent \"<task>\" --model <model>",
        "Oh My Pi (omp) is a full terminal coding agent — hash-anchored edits, LSP, subagents, a browser. It inherits MCP/rules config from .claude/.codex-style dirs in the workspace, so your pass-through config reaches it via the repo. No first-party MCP server mode yet: drive it one-shot per task and verify its work yourself with git diff and tests.",
    );
    let mut oh_my_pi = oh_my_pi;
    oh_my_pi.probe = Some("npx -y @oh-my-pi/pi-coding-agent --version".into());

    let mut codex = def(
        "codex",
        "Codex CLI",
        HarnessAuth::Gateway,
        "npx -y @openai/codex exec --model <model> \"<task>\"",
        "Codex exec emits JSON events with --json — read the final result event. As an MCP server (codex mcp) it exposes tool-driven sessions; prefer that when registered on your config.",
    );
    codex.json_invoke = Some("npx -y @openai/codex exec --model <model> --json \"<task>\"".into());
    codex.mcp_serve = Some(McpServe {
        command: "npx".into(),
        args: vec!["-y".into(), "@openai/codex".into(), "mcp".into()],
    });
    codex.probe = Some("npx -y @openai/codex --version".into());

    vec![opencode, claude, oh_my_pi, codex]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_harnesses_carry_the_workbench_key_fallback() {
        let r = resolve_def(
            def("x", "X", HarnessAuth::Gateway, "run", "g"),
            HarnessSource::Builtin,
        );
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
        d.env = Some(
            [("OPENAI_BASE_URL", "overridden")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), Value::String(v.into())))
                .collect(),
        );
        let r = resolve_def(d, HarnessSource::Builtin);
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
        );
        assert!(native.full_env.is_empty());
    }

    #[test]
    fn the_model_arg_carries_the_cli_prefix() {
        let mut d = def("opencode", "opencode", HarnessAuth::Gateway, "run", "g");
        d.model_prefix = Some("openai/".into());
        assert_eq!(harness_model_arg(&d, "qwen3:14b"), "openai/qwen3:14b");
        assert_eq!(
            harness_model_arg(
                &def("codex", "Codex CLI", HarnessAuth::Gateway, "r", "g"),
                "m"
            ),
            "m"
        );
    }

    #[test]
    fn the_builtins_parse_from_their_own_wire_shape() {
        // The defs above are built as structs; round-trip them through the
        // JSON a workbench_harness_defs row would carry, proving the serde
        // shape matches what the admin surface writes.
        for b in builtins() {
            let wire = serde_json::to_value(&b).unwrap();
            let back: WorkbenchHarnessDef = serde_json::from_value(wire).unwrap();
            assert_eq!(back.slug, b.slug);
            assert_eq!(back.auth, b.auth);
            assert_eq!(
                back.mcp_config.map(|m| m.filename),
                b.mcp_config.map(|m| m.filename)
            );
        }
    }
}
