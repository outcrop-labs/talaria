// The harness REGISTRY — the extension point developers build against. A
// harness is a declarative WorkbenchHarnessDefinition (see @talaria/sdk
// defineWorkbenchHarness) — NOT the activity harness in harness/, which is a
// prompt plus an output schema run by `run_harness`. Two contracts: how it
// authenticates, how it's invoked (structured output first), how it
// serves/consumes MCP, and what a driving agent should understand about it.
//
// This registry answers the builtin definitions, the merged registry
// (builtin < admin-custom, by slug), and the auth-derived fullEnv the render
// interpolates into containers. Two layers stay out of scope by construction
// and are named here:
//   • app-shipped (apps/<slug>/harness.ts) — app modules stay TS/node, a
//     compiled module this process cannot load; no repo app ships one today,
//     and app-published surfaces carry the same boundary in mcp_registry
//     (loud error at dispatch, never a silent miss). When the app runtime
//     crosses, its harnesses join this registry.
//   • renderMcpConfig / format:'custom' — admin JSON can't carry functions,
//     so 'custom' is app-shipped-only and unreachable from the layers this
//     registry serves.
// The admin upsert/delete and the effort→model chain serve the
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
    ClaudeJson,
    OpencodeJson,
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
    /// Invocation template — `<model>`, `<task>`, `<sessionDir>` placeholders.
    pub invoke: String,
    /// Structured-output form — REQUIRED for good drivers; agents are taught
    /// to read structured results, never scrape logs.
    #[serde(default)]
    pub json_invoke: Option<String>,
    /// Follow-up on the same session as a prior invoke (same placeholders).
    /// Hermes steers; the harness keeps context. Omit when every run in the
    /// workdir continues the project session on its own (opencode).
    #[serde(default)]
    pub continue_invoke: Option<String>,
    #[serde(default)]
    pub continue_json_invoke: Option<String>,
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
    /// The definition as THIS registry answers it — the wire's own key order
    /// (each builtin spells its fields differently), which is why it is
    /// carried rather than re-derived from the struct.
    pub wire_def: Value,
    pub source: HarnessSource,
    /// Full container env: auth-derived + definition env.
    pub full_env: Map<String, Value>,
}

impl ResolvedHarness {
    /// The definition, then `source`, then `fullEnv` appended after the
    /// definition's own keys.
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
pub fn gateway_env() -> Map<String, Value> {
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
    // fatal. The stored definition rides as-is (its own key order), with the
    // row's slug overriding in place.
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
/// The stored JSON is the route's parsed body, re-emitted in SCHEMA shape
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

/// Slot model and per-job session dir into an invoke template. `<task>` stays
/// for the driving agent to fill — that is the steer, not a platform value.
pub fn fill_harness_cmd(
    tmpl: &str,
    h: &WorkbenchHarnessDef,
    model: Option<&str>,
    session_dir: &str,
) -> String {
    let mut s = tmpl.replace("<sessionDir>", session_dir);
    if let Some(m) = model {
        s = s.replace("<model>", &harness_model_arg(h, m));
    }
    s
}

/// Where Pi and Oh My Pi keep config, auth, and sessions. On the department
/// state volume so a session persists and a department-mate can resume it.
/// omp relocates `~/.omp/agent` here when `PI_CODING_AGENT_DIR` is set — which
/// is why both builtins set it, not only Pi. Docker creates this directory as
/// root when the render bind-mounts policy files into it; the fleet render's
/// cont-init hook chowns it back to the runtime user.
pub const PI_CODING_AGENT_DIR: &str = "/opt/data/workbench/harness/pi";

/// The three builtin definitions — one json! per harness because each wire
/// carries its OWN key order. The struct is derived from these, so there is
/// exactly one place a builtin is spelled.
fn builtin_wires() -> Vec<Value> {
    vec![
        serde_json::json!({
            "slug": "opencode",
            "label": "opencode",
            "auth": "gateway",
            "env": { "OPENCODE_CONFIG": "/opt/workbench-config/opencode.json" },
            "modelPrefix": "openai/",
            "invoke": "npx -y opencode-ai@latest run --model <model> \"<task>\"",
            "jsonInvoke": "npx -y opencode-ai@latest run --model <model> --format json \"<task>\"",
            "probe": "npx -y opencode-ai@latest --version",
            "mcpConfig": { "format": "opencode-json", "filename": "opencode.json" },
            "install": { "npm": ["opencode-ai"] },
            "guide": "You are the orchestrator; opencode is the pair programmer. Stay in the start_job workdir — each `run` continues that project's session, so follow-ups are just another run with the next steer, not a new clone. Give ONE scoped ask at a time (a function, a test, a review of the last diff), read the JSON result, then steer. Do not dump the whole ticket into one prompt. Git over https:// just works (Talaria injects the credential); never gh, never a token in a URL. After the change is right: git diff, verify, finish_job.",
        }),
        serde_json::json!({
            "slug": "pi",
            "label": "Pi",
            "auth": "gateway",
            "env": { "PI_CODING_AGENT_DIR": PI_CODING_AGENT_DIR },
            "invoke": "npx -y @earendil-works/pi-coding-agent@latest -p -a --session-dir <sessionDir> --provider talaria --model <model> \"<task>\"",
            "jsonInvoke": "npx -y @earendil-works/pi-coding-agent@latest --mode json -a --session-dir <sessionDir> --provider talaria --model <model> \"<task>\"",
            "continueInvoke": "npx -y @earendil-works/pi-coding-agent@latest -p -a --session-dir <sessionDir> -c --provider talaria --model <model> \"<task>\"",
            "continueJsonInvoke": "npx -y @earendil-works/pi-coding-agent@latest --mode json -a --session-dir <sessionDir> -c --provider talaria --model <model> \"<task>\"",
            "probe": "npx -y @earendil-works/pi-coding-agent@latest --version",
            "mcpConfig": { "format": "claude-json", "filename": "mcp.json" },
            "install": { "npm": ["@earendil-works/pi-coding-agent"] },
            "guide": "You are the orchestrator; Pi is the pair programmer. First turn: jsonRun (or run) with a scoped ask. Every later turn: continueJsonRun / continueRun (`-c`) against the same --session-dir — that is the conversation, not a new agent. Give one slice at a time, read message_end / agent_end, then steer. Never --no-session, never the TUI, never the whole ticket in one prompt. -a trusts the project so it does not stall. Git over https:// just works. After the change is right: git diff, verify, finish_job.",
        }),
        serde_json::json!({
            "slug": "oh-my-pi",
            "label": "Oh My Pi",
            "auth": "gateway",
            "env": { "PI_CODING_AGENT_DIR": PI_CODING_AGENT_DIR },
            "modelPrefix": "talaria/",
            "invoke": "npx -y @oh-my-pi/pi-coding-agent@latest -p --auto-approve --session-dir <sessionDir> --model <model> \"<task>\"",
            "jsonInvoke": "npx -y @oh-my-pi/pi-coding-agent@latest --mode json --auto-approve --session-dir <sessionDir> --model <model> \"<task>\"",
            "continueInvoke": "npx -y @oh-my-pi/pi-coding-agent@latest -p --auto-approve --session-dir <sessionDir> -c --model <model> \"<task>\"",
            "continueJsonInvoke": "npx -y @oh-my-pi/pi-coding-agent@latest --mode json --auto-approve --session-dir <sessionDir> -c --model <model> \"<task>\"",
            "probe": "npx -y @oh-my-pi/pi-coding-agent@latest --version",
            "mcpConfig": { "format": "claude-json", "filename": "mcp.json" },
            "install": { "npm": ["@oh-my-pi/pi-coding-agent"] },
            "guide": "You are the orchestrator; Oh My Pi (omp) is the pair programmer. First turn: jsonRun. Later turns: continueJsonRun (`-c`) on the same --session-dir. One scoped ask per turn — a function, a failing test, a review of the last diff — then read the JSON events and steer. Never --no-session, never the TUI, never one-shot the feature. --auto-approve skips tool prompts. Hash-anchored edits, LSP, subagents, and a browser live inside omp; do not reimplement them. Git over https:// just works. After the change is right: git diff, verify, finish_job.",
        }),
    ]
}

// ── Effort → model (the platform's call, never the agent's) ──────────────────

/// Effort → model: per-agent override first, then the global roles with a
/// fall-down chain so unset slots never strand work. A DB failure inside the
/// chain propagates — the caller surfaces it as a thrown tool error, not a
/// resolved null.
pub async fn effort_model(
    pg: &PgPool,
    effort: &str,
    overrides: Option<&Map<String, Value>>,
) -> Result<Option<String>, String> {
    async fn resolves(pg: &PgPool, model: &str) -> Result<bool, String> {
        Ok(talaria_gateway::registry::resolve_route(pg, model)
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
        if let Some(m) = talaria_model_roles::resolve_role_model(pg, &format!("code-{e}"))
            .await
            .map_err(|err| format!("role resolve: {err}"))?
        {
            return Ok(Some(m));
        }
    }
    if let Some(utility) = talaria_model_roles::resolve_role_model(pg, "utility")
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
/// key order is fixed: light, standard, heavy.
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
            "the definition's env wins over the auth-derived default — an explicit definition beats a derived default"
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
            serde_json::from_value(def("pi", "Pi", HarnessAuth::Gateway, "r", "g")).unwrap();
        assert_eq!(harness_model_arg(&plain, "m"), "m");
    }

    #[test]
    fn fill_harness_cmd_slots_session_dir_and_leaves_task() {
        let d: WorkbenchHarnessDef = serde_json::from_value(def(
            "pi",
            "Pi",
            HarnessAuth::Gateway,
            "pi -p --session-dir <sessionDir> --model <model> \"<task>\"",
            "g",
        ))
        .unwrap();
        let out = fill_harness_cmd(
            &d.invoke,
            &d,
            Some("qwen3:14b"),
            "/opt/data/workbench/sessions/abc",
        );
        assert_eq!(
            out,
            "pi -p --session-dir /opt/data/workbench/sessions/abc --model qwen3:14b \"<task>\""
        );
        assert!(
            out.contains("\"<task>\""),
            "the steer stays for the driving agent to fill"
        );
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
        // The harnesses route answers with these wires, so each wire's key
        // order is the contract. One assertion per builtin, keys in the
        // wire's order.
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
                "install",
                "guide"
            ]
        );
        assert_eq!(
            keys(&wires[1]),
            vec![
                "slug",
                "label",
                "auth",
                "env",
                "invoke",
                "jsonInvoke",
                "continueInvoke",
                "continueJsonInvoke",
                "probe",
                "mcpConfig",
                "install",
                "guide"
            ]
        );
        assert_eq!(
            keys(&wires[2]),
            vec![
                "slug",
                "label",
                "auth",
                "env",
                "modelPrefix",
                "invoke",
                "jsonInvoke",
                "continueInvoke",
                "continueJsonInvoke",
                "probe",
                "mcpConfig",
                "install",
                "guide"
            ]
        );
    }

    #[test]
    fn omp_and_pi_share_the_persistent_config_dir() {
        // omp honors PI_CODING_AGENT_DIR and relocates ~/.omp/agent there.
        // If the two builtins diverge, the render mounts policy files where
        // one of them does not look, and that harness starts unconfigured.
        let wires = builtin_wires();
        assert_eq!(
            wires[1]["env"]["PI_CODING_AGENT_DIR"],
            serde_json::json!(PI_CODING_AGENT_DIR)
        );
        assert_eq!(
            wires[2]["env"]["PI_CODING_AGENT_DIR"],
            wires[1]["env"]["PI_CODING_AGENT_DIR"]
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
