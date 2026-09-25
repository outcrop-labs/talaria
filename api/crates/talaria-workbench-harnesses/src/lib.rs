// The workbench's coding harness: Oh My Pi (omp), and only omp. A Developer
// Agent drives it from its sandbox: the persona is the orchestrator, omp is
// the pair programmer. There is no registry to pick from and nothing for an
// admin to configure per agent; this file is the whole definition.
//
// Models are omp's call, not a per-job pin. The org's Workbench roles on
// /models (code-light / code-standard / code-heavy) become omp's own model
// roles (smol / default / slow + plan), so omp moves between them the way it
// is built to (subagents and commits on smol, the advisor on slow, plan mode
// on plan) instead of running a whole job on one model the platform chose.

use serde_json::{Map, Value, json};
use sqlx::PgPool;

/// Oh My Pi, as a start_job answer spells it. `<model>` is the default role's
/// model (the platform fills it, the agent never picks), `<sessionDir>` the
/// job's session dir, and `<task>` stays for the driving agent to fill.
pub struct Harness {
    pub slug: &'static str,
    pub label: &'static str,
    pub invoke: &'static str,
    pub json_invoke: &'static str,
    pub continue_invoke: &'static str,
    pub continue_json_invoke: &'static str,
    /// A cheap command that proves the binary runs in the sandbox, surfaced
    /// by the workbench doctor for agents to self-verify.
    pub probe: &'static str,
    /// What a driving agent should understand: sessions, resume, results.
    pub guide: &'static str,
}

pub const OMP: Harness = Harness {
    slug: "oh-my-pi",
    label: "Oh My Pi",
    invoke: "npx -y @oh-my-pi/pi-coding-agent@latest -p --auto-approve --session-dir <sessionDir> --model <model> \"<task>\"",
    json_invoke: "npx -y @oh-my-pi/pi-coding-agent@latest --mode json --auto-approve --session-dir <sessionDir> --model <model> \"<task>\"",
    continue_invoke: "npx -y @oh-my-pi/pi-coding-agent@latest -p --auto-approve --session-dir <sessionDir> -c --model <model> \"<task>\"",
    continue_json_invoke: "npx -y @oh-my-pi/pi-coding-agent@latest --mode json --auto-approve --session-dir <sessionDir> -c --model <model> \"<task>\"",
    probe: "npx -y @oh-my-pi/pi-coding-agent@latest --version",
    guide: "You are the orchestrator; Oh My Pi (omp) is the pair programmer. First turn: jsonRun. Later turns: continueJsonRun (`-c`) on the same --session-dir. One scoped ask per turn (a function, a failing test, a review of the last diff), then read the JSON events and steer. Never --no-session, never the TUI, never one-shot the feature. --auto-approve skips tool prompts. omp picks its own models for the work (a fast model for subagents and small steps, a reasoning model for planning and review); do not pass a different --model. Hash-anchored edits, LSP, subagents, and a browser live inside omp; do not reimplement them. Git over https:// just works. After the change is right: git diff, verify, finish_job.",
};

/// The provider omp's models.json declares; model ids on the command line and
/// in the role env carry it as a prefix.
pub const PROVIDER: &str = "talaria";

/// Where omp keeps config, auth, and sessions. On the department state volume
/// so a session persists and a department-mate can resume it. omp relocates
/// `~/.omp/agent` here when `PI_CODING_AGENT_DIR` is set. Docker creates this
/// directory as root when the render bind-mounts policy files into it; the
/// fleet render's cont-init hook chowns it back to the runtime user.
pub const PI_CODING_AGENT_DIR: &str = "/opt/data/workbench/harness/pi";

/// The MCP pass-through file omp reads inside [`PI_CODING_AGENT_DIR`].
pub const MCP_CONFIG_FILE: &str = "mcp.json";

// The harness reaches the same org gateway the personas do, but on its OWN
// credential (`workbench-gateway`, minted into the fleet .env as
// LLM_WORKBENCH_API_KEY by fleet-brain). It must: a harness run is spend NO
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

/// The container env omp needs: gateway auth plus its config dir.
pub fn omp_env() -> Map<String, Value> {
    let mut env = gateway_env();
    env.insert("PI_CODING_AGENT_DIR".into(), json!(PI_CODING_AGENT_DIR));
    env
}

/// A model id as omp's CLI and role env expect it.
pub fn model_arg(model: &str) -> String {
    format!("{PROVIDER}/{model}")
}

/// Slot model and per-job session dir into an invoke template. `<task>` stays
/// for the driving agent to fill — that is the steer, not a platform value.
pub fn fill_cmd(tmpl: &str, model: Option<&str>, session_dir: &str) -> String {
    let mut s = tmpl.replace("<sessionDir>", session_dir);
    if let Some(m) = model {
        s = s.replace("<model>", &model_arg(m));
    }
    s
}

// ── Model roles (the org's Workbench roles, handed to omp) ──────────────────

/// omp's model roles, resolved from the org's Workbench roles. `None` means
/// nothing in the chain resolves, so omp runs on its default alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OmpRoles {
    pub default: Option<String>,
    pub smol: Option<String>,
    pub slow: Option<String>,
    pub plan: Option<String>,
}

impl OmpRoles {
    /// The map agents see (doctor, list_repos, start_job). Fixed key order.
    pub fn wire(&self) -> Value {
        json!({
            "default": self.default,
            "smol": self.smol,
            "slow": self.slow,
            "plan": self.plan,
        })
    }

    /// omp reads its non-default roles from these env vars; the default rides
    /// `--model` on the invocation line. Unresolved roles are left unset so
    /// omp falls back to its default rather than a model that does not exist.
    pub fn env(&self) -> Map<String, Value> {
        let mut env = Map::new();
        for (key, model) in [
            ("PI_SMOL_MODEL", &self.smol),
            ("PI_SLOW_MODEL", &self.slow),
            ("PI_PLAN_MODEL", &self.plan),
        ] {
            if let Some(m) = model {
                env.insert(key.into(), json!(model_arg(m)));
            }
        }
        env
    }

    /// Every distinct model, default first: what models.json declares.
    pub fn model_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for m in [&self.default, &self.smol, &self.slow, &self.plan]
            .into_iter()
            .flatten()
        {
            if !ids.contains(m) {
                ids.push(m.clone());
            }
        }
        ids
    }
}

/// The org's code-standard / code-light / code-heavy roles as omp's default /
/// smol / slow+plan. Each resolves with a fall-down chain so unset slots never
/// strand work. A DB failure inside the chain propagates; the caller surfaces
/// it as a thrown tool error, not a resolved null.
pub async fn omp_roles(pg: &PgPool) -> Result<OmpRoles, String> {
    let default = role_model(pg, "standard").await?;
    let smol = role_model(pg, "light").await?;
    let heavy = role_model(pg, "heavy").await?;
    Ok(OmpRoles {
        default,
        smol,
        slow: heavy.clone(),
        plan: heavy,
    })
}

async fn role_model(pg: &PgPool, weight: &str) -> Result<Option<String>, String> {
    async fn resolves(pg: &PgPool, model: &str) -> Result<bool, String> {
        Ok(talaria_gateway::registry::resolve_route(pg, model)
            .await
            .map_err(|e| format!("route resolve: {e}"))?
            .is_some())
    }
    let order: &[&str] = match weight {
        "heavy" => &["heavy", "standard", "light"],
        "standard" => &["standard", "light"],
        _ => &["light"],
    };
    for w in order {
        if let Some(m) = talaria_model_roles::resolve_role_model(pg, &format!("code-{w}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omp_env_carries_the_workbench_key_fallback_and_its_config_dir() {
        let env = omp_env();
        assert_eq!(
            env.get("OPENAI_API_KEY").and_then(Value::as_str),
            Some("${LLM_WORKBENCH_API_KEY:-${LLM_API_KEY}}")
        );
        assert_eq!(
            env.get("OPENAI_BASE_URL").and_then(Value::as_str),
            Some("${LLM_BASE_URL}")
        );
        assert_eq!(
            env.get("PI_CODING_AGENT_DIR").and_then(Value::as_str),
            Some(PI_CODING_AGENT_DIR)
        );
    }

    #[test]
    fn fill_cmd_slots_the_prefixed_model_and_session_dir_and_leaves_task() {
        let out = fill_cmd(
            OMP.json_invoke,
            Some("qwen3:14b"),
            "/opt/data/workbench/sessions/abc",
        );
        assert_eq!(
            out,
            "npx -y @oh-my-pi/pi-coding-agent@latest --mode json --auto-approve --session-dir /opt/data/workbench/sessions/abc --model talaria/qwen3:14b \"<task>\""
        );
        assert!(
            out.contains("\"<task>\""),
            "the steer stays for the driving agent to fill"
        );
    }

    #[test]
    fn every_invocation_line_takes_a_session_dir_and_a_model() {
        for t in [
            OMP.invoke,
            OMP.json_invoke,
            OMP.continue_invoke,
            OMP.continue_json_invoke,
        ] {
            assert!(t.contains("<sessionDir>"), "{t}");
            assert!(t.contains("<model>"), "{t}");
        }
        assert!(OMP.continue_invoke.contains(" -c "));
        assert!(OMP.continue_json_invoke.contains(" -c "));
    }

    #[test]
    fn roles_become_omp_env_with_the_provider_prefix_and_skip_unresolved() {
        let roles = OmpRoles {
            default: Some("mid".into()),
            smol: Some("fast".into()),
            slow: None,
            plan: Some("deep".into()),
        };
        let env = roles.env();
        assert_eq!(env["PI_SMOL_MODEL"], json!("talaria/fast"));
        assert_eq!(env["PI_PLAN_MODEL"], json!("talaria/deep"));
        assert!(
            !env.contains_key("PI_SLOW_MODEL"),
            "an unresolved role is left for omp's default"
        );
        assert!(
            !env.contains_key("PI_MODEL"),
            "the default rides --model, not the env"
        );
    }

    #[test]
    fn model_ids_are_distinct_with_the_default_first() {
        let roles = OmpRoles {
            default: Some("mid".into()),
            smol: Some("fast".into()),
            slow: Some("deep".into()),
            plan: Some("deep".into()),
        };
        assert_eq!(roles.model_ids(), vec!["mid", "fast", "deep"]);
        assert!(OmpRoles::default().model_ids().is_empty());
    }
}
