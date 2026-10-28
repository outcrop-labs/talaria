// Per-agent secrets — env vars an agent needs that don't belong in the shared
// fleet .env (a Figma token for one agent, a vendor key for another). Set in
// the UI, stored encrypted (secretbox), and materialized ONLY at render time
// into fleet/agents/<slug>/secrets.env (0600), which the rendered service
// loads via env_file. Values are write-only through the API: names and
// timestamps list; plaintext never leaves the server.
//
// The render also lists their names in config.yaml's
// `terminal.env_passthrough` ([`passthrough_names`]): Hermes runs
// execute_code with only the env names it allows, so without that list a
// per-agent secret reached the agent's terminal and never its scripts.
//
// Two kinds of row are the exception. A `CONFIG_*` secret is a credential
// sealed out of the agent's own config: Hermes reads it, its commands must
// not. The render declares those names in a platform plugin manifest Hermes
// never loads ([`sealed_manifest`]), which is how Hermes keeps its own
// platform tokens out of every child it spawns. And a row saved under a name
// the rules now refuse ([`reserved`]) is not loaded at all.

use sqlx::PgPool;
use std::path::Path;

use talaria_secretbox::SecretBox;

mod config_seal;
mod env_file;
pub use config_seal::{
    CONFIG_SECRET_PREFIX, ConfigLiteral, ConfigSeal, config_secret_handles, config_secret_paths,
    copy_config_secrets, held_config_secrets, plan_config_seal,
    plan_config_seal_dropping_undeliverable, seal_config_literals, withhold_config_secret,
    withhold_config_secrets, withhold_unheld_config_secrets,
};

/// Container-env name: UPPER_SNAKE, must not collide with the stamped vars.
fn name_ok(name: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[A-Z][A-Z0-9_]{1,63}$").unwrap())
        .is_match(name)
}

/// Names the render stamps on an agent's container itself. Compose lets the
/// service's `environment:` win over its `env_file:`, so a secret under one of
/// these would be silently overridden — or, for the ones only some agents get
/// (the Developer overlay's), silently change how that agent's sandbox runs.
const TALARIA_STAMPED: &[&str] = &[
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_CONFIG_SYSTEM",
    "GIT_TERMINAL_PROMPT",
    "OMP_AUTH_BROKER_TOKEN",
    "OMP_AUTH_BROKER_URL",
    "PI_CODING_AGENT_DIR",
    "PI_PLAN_MODEL",
    "PI_SLOW_MODEL",
    "PI_SMOL_MODEL",
    "PLAYWRIGHT_BROWSERS_PATH",
    "XDG_DATA_HOME",
];

/// Prefixes that are Talaria's own (its keys, its URLs, the gateway's model
/// env) — `TALARIA_AGENT_KEY`, `TALARIA_WORKBENCH_API_KEY`, `LLM_API_KEY` …
const TALARIA_PREFIXES: &[&str] = &["TALARIA_", "LLM_"];

/// Prefixes Hermes reads as its own runtime settings: `HERMES_*` (home,
/// redaction, timeouts …), `API_SERVER_*` (the listener Talaria dials),
/// `GATEWAY_*` (who may talk to the agent, relays), `AUXILIARY_*` (side-model
/// credentials) and `TERMINAL_*` (`TERMINAL_ENV=ssh` moves the agent's shell
/// to another machine). A secret under one of these reconfigures the agent.
const HERMES_PREFIXES: &[&str] = &[
    "HERMES_",
    "API_SERVER_",
    "GATEWAY_",
    "AUXILIARY_",
    "TERMINAL_",
];

/// Names the container's own processes read at start — the shell, the
/// dynamic loader, the Python Hermes runs on, node (omp), s6-overlay's init,
/// and the browser executable Hermes's boot hook keeps when one is already
/// set. A secret here reconfigures the agent's runtime, not a tool.
const PROCESS_RUNTIME: &[&str] = &[
    "AGENT_BROWSER_EXECUTABLE_PATH",
    "BASH_ENV",
    "ENV",
    "HOME",
    "LD_AUDIT",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "NODE_OPTIONS",
    "NODE_PATH",
    "PATH",
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "SHELL",
    "USER",
];

/// s6-overlay's settings (`S6_*`), read by the container's init.
const PROCESS_RUNTIME_PREFIXES: &[&str] = &["S6_"];

/// Hermes v0.22.4's own stripped set — see the file's header for what it is
/// and how to refresh it when the image pin moves.
const HERMES_STRIPPED_SNAPSHOT: &str = include_str!("hermes_stripped_env.txt");

fn hermes_stripped() -> &'static std::collections::HashSet<&'static str> {
    static SET: std::sync::OnceLock<std::collections::HashSet<&'static str>> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| {
        HERMES_STRIPPED_SNAPSHOT
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect()
    })
}

/// Every name Hermes strips from the commands an agent runs, at the pinned
/// image. A command the platform runs in the sandbox on the agent's behalf
/// (`docker exec`) unsets these first, so it sees the env the agent's own
/// terminal would — not one that passes where the agent's would fail.
pub fn hermes_stripped_names() -> impl Iterator<Item = &'static str> {
    let mut names: Vec<&'static str> = hermes_stripped().iter().copied().collect();
    names.sort_unstable();
    names.into_iter()
}

/// Why `name` cannot be a per-agent secret, or `None` when it can.
///
/// A per-agent secret is for the agent's own tools — a vendor key a skill's
/// script reads. Four kinds of name are not that: Talaria's (stamped on the
/// container, which wins over the secret), Hermes's runtime settings and the
/// container's process settings (they reconfigure the agent), and the names
/// Hermes strips from every command the agent runs. Those last never reach a
/// tool, and Hermes reads many of them for itself: a `TELEGRAM_BOT_TOKEN`
/// turns on a Telegram adapter, and a provider key such as
/// `OPENROUTER_API_KEY` is one Hermes would call a model with if its config
/// ever stopped naming the gateway (v0.22.4 does not fall back to it while a
/// main provider is set — `_discovery_chain_allowed`).
pub fn reserved(name: &str) -> Option<String> {
    if TALARIA_STAMPED.contains(&name) || TALARIA_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return Some(format!("{name} is managed by Talaria"));
    }
    if PROCESS_RUNTIME.contains(&name)
        || PROCESS_RUNTIME_PREFIXES.iter().any(|p| name.starts_with(p))
    {
        return Some(format!(
            "{name} configures the agent's runtime (its container's processes read it at start), not a secret for its tools"
        ));
    }
    if let Some(prefix) = HERMES_PREFIXES.iter().find(|p| name.starts_with(*p)) {
        return Some(format!(
            "{name} is a Hermes setting ({prefix}* configures the agent's runtime), not a secret for its tools"
        ));
    }
    if hermes_stripped().contains(name) {
        return Some(format!(
            "{name} is one of Hermes's own provider, platform or tool names: Hermes strips it from every command the agent runs, so no tool of the agent's would see it, and reads it for itself instead. A key a command of the agent's reads goes under a name of your own; Hermes's own web search takes its key from Admin → Search"
        ));
    }
    None
}

/// Wire shape (camelCase — the agent-view route serves these verbatim).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSecretMeta {
    pub name: String,
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
    /// Why this name would be refused now ([`reserved`]), for a row saved
    /// before the rule. Such a row is not loaded into the container
    /// ([`materialize_agent_secrets`] skips it), so the UI says why and
    /// offers the delete. Absent for an ordinary secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// For a `CONFIG_*` secret: where the agent's current config names it
    /// (`platforms.telegram.token`). Empty when nothing does any more.
    /// Absent for every other secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_at: Option<Vec<String>>,
}

/// The stored config of the version the render runs (`current_version`),
/// or None for a def with no version.
pub async fn current_config(
    pg: &PgPool,
    agent_id: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    sqlx::query_scalar(
        "select v.config from agent_defs d \
         join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.id::text = $1",
    )
    .bind(agent_id)
    .fetch_optional(pg)
    .await
}

pub async fn list_agent_secrets(
    pg: &PgPool,
    agent_id: &str,
) -> Result<Vec<AgentSecretMeta>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<i64>)> = sqlx::query_as(
        "select name, updated_by::text, (extract(epoch from updated_at) * 1000)::bigint \
         from agent_secrets where agent_id::text = $1 order by name",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await?;
    let config = if rows
        .iter()
        .any(|(n, _, _)| n.starts_with(CONFIG_SECRET_PREFIX))
    {
        current_config(pg, agent_id).await?
    } else {
        None
    };
    Ok(rows
        .into_iter()
        .map(|(name, updated_by, at)| AgentSecretMeta {
            refused: reserved(&name),
            used_at: name.starts_with(CONFIG_SECRET_PREFIX).then(|| {
                config
                    .as_ref()
                    .map(|c| config_secret_paths(c, &name))
                    .unwrap_or_default()
            }),
            name,
            updated_by,
            updated_at: at.map(talaria_agent_auth::epoch_ms_to_iso),
        })
        .collect())
}

/// Set (or overwrite) one secret. Validation is the API contract — the
/// messages are what the route returns.
pub async fn set_agent_secret(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
    name: &str,
    value: &str,
    actor: Option<&str>,
) -> Result<(), String> {
    if !name_ok(name) {
        return Err("secret names are UPPER_SNAKE (2–64 chars, starting with a letter)".into());
    }
    if let Some(why) = reserved(name) {
        return Err(why);
    }
    if value.is_empty() || value.len() > 8192 {
        return Err("value required (max 8 KB)".into());
    }
    if value.contains('\n') || value.contains('\r') {
        return Err("value cannot contain newlines".into());
    }
    let sealed = sb
        .seal(value)
        .map_err(|e| format!("secret seal failed: {e}"))?;
    // `updated_by` is TEXT and the route passes the actor's email (or name):
    // a `$4::uuid` cast here refused every write from a person with an email.
    sqlx::query(
        "insert into agent_secrets (agent_id, name, value_enc, updated_by) \
         values ($1::uuid, $2, $3, $4) \
         on conflict (agent_id, name) do update \
         set value_enc = excluded.value_enc, updated_by = excluded.updated_by, updated_at = now()",
    )
    .bind(agent_id)
    .bind(name)
    .bind(&sealed)
    .bind(actor)
    .execute(pg)
    .await
    .map_err(|e| format!("secret write failed: {e}"))?;
    Ok(())
}

pub async fn delete_agent_secret(
    pg: &PgPool,
    agent_id: &str,
    name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from agent_secrets where agent_id::text = $1 and name = $2")
        .bind(agent_id)
        .bind(name)
        .execute(pg)
        .await?;
    Ok(())
}

/// The secret names the render lists in `terminal.env_passthrough`, so they
/// reach execute_code as well as the terminal. A row stored under a name that
/// is reserved now (written before the rule) is left out: Hermes refuses to
/// pass its own names through anyway, and a Talaria name is not the agent's.
pub async fn passthrough_names(pg: &PgPool, agent_id: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = sqlx::query_scalar(
        "select name from agent_secrets where agent_id::text = $1 order by name",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("agent secrets read failed: {e}"))?;
    Ok(names.into_iter().filter(|n| passes_through(n)).collect())
}

/// Whether a stored secret's name belongs in `terminal.env_passthrough`.
///
/// A `CONFIG_*` secret is a credential sealed out of the agent's own
/// config.yaml (a platform token, a dashboard password, an Authorization
/// header): the config names it as `${CONFIG_…}` and Hermes reads it for
/// itself. It is not a key for the agent's tools, so it is never listed.
/// execute_code admits a credential-looking name only by passthrough, so
/// leaving these out keeps them out of the agent's scripts; the sealed
/// manifest ([`sealed_manifest`]) keeps them out of its terminal too.
fn passes_through(name: &str) -> bool {
    reserved(name).is_none() && !name.starts_with(CONFIG_SECRET_PREFIX)
}

/// What [`materialize_agent_secrets`] wrote for one agent.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Materialized {
    /// Whether the service should declare the env_file (any row was written).
    pub env_file: bool,
    /// The `CONFIG_*` names written: credentials sealed out of the agent's
    /// own config, declared in the sealed manifest so no child sees them.
    pub sealed: Vec<String>,
    /// Rows left out because their name is refused now ([`reserved`]),
    /// each with the reason. The render logs them.
    pub skipped: Vec<(String, String)>,
}

/// The directory, inside the agent's fleet dir, that holds the sealed
/// manifest. Mounted whole at [`SEALED_PLUGIN_MOUNT`], so a rewrite (a
/// rename) is visible in a running container.
pub const SEALED_PLUGIN_DIR: &str = "hermes-sealed";

/// Where Hermes looks for a user platform plugin (`$HERMES_HOME/plugins/
/// platforms/<name>/plugin.yaml`, `/opt/data` being the agent's home).
pub const SEALED_PLUGIN_MOUNT: &str = "/opt/data/plugins/platforms/talaria-sealed";

/// The sealed manifest: a platform plugin that declares the agent's
/// `CONFIG_*` names as password entries, and nothing else.
///
/// Hermes v0.22.4 strips the secrets a profile's own platform plugins
/// declare from EVERY child it spawns, the foreground terminal included
/// (`tools/environments/local.py` `_filter_secret_env` and
/// `_scrub_credentials`, via `local_env_policy._home_adapter_secret_env`,
/// which scans `<home>/plugins/platforms/*/plugin.yaml` on every spawn,
/// cached on the manifest's file signature). It refuses them as
/// `env_passthrough` names too. The plugin itself is never loaded: it is not
/// in `plugins.enabled`, and it has no adapter to load. Hermes's own process
/// keeps the values, which is what lets it expand `${CONFIG_…}` in
/// config.yaml on every reload.
///
/// `optional_env`, not `requires_env`: nothing is required of an adapter that
/// never runs. No `category` (the scan counts only `messaging`, the default).
pub fn sealed_manifest(names: &[String]) -> String {
    let mut out = String::from(
        "# Rendered by Talaria. Do not hand-edit.\n\
         # A declaration only: no adapter, never enabled. Hermes strips every secret a\n\
         # platform plugin declares from the commands the agent runs, so these sealed\n\
         # config credentials reach Hermes (config.yaml's ${CONFIG_...}) and no child.\n\
         name: talaria-sealed\n\
         label: Talaria sealed config credentials\n\
         kind: platform\n\
         version: \"1\"\n\
         description: Declares the agent's sealed config credentials so Hermes keeps them out of every command it runs. Never enabled; no adapter.\n",
    );
    if names.is_empty() {
        out.push_str("optional_env: []\n");
    } else {
        out.push_str("optional_env:\n");
        for name in names {
            // The names are `CONFIG_[A-Z0-9_]+` (name_ok), plain YAML scalars.
            out.push_str(&format!("  - name: {name}\n    password: true\n"));
        }
    }
    out
}

/// Write (or remove) the agent's secrets env file for the renderer, and its
/// sealed manifest. An unsealable row fails the render — a secret the server
/// can no longer open must not silently vanish from a container that runs on
/// it.
///
/// A row under a name that is refused now ([`reserved`], saved before the
/// rule) is not written. No command the agent runs could read one of those
/// names: Hermes strips its own provider, platform and tool names from every
/// command, and the rest reconfigure the agent or are Talaria's. Loading one
/// let it reconfigure the agent: a `TELEGRAM_BOT_TOKEN` switched on an
/// adapter outside Talaria's channels, a `TERMINAL_ENV` moved the shell.
///
/// One thing does stop working. Hermes's own in-process tools read their
/// vendor keys from its environment (v0.22.4's web search picks Tavily when
/// `TAVILY_API_KEY` is set, `plugins/web/tavily/provider.py`), so a per-agent
/// tool key saved before the rule stops reaching them on the agent's next
/// recreate. Web search takes its key from Admin → Search, for every agent
/// (talaria-agent-web-search); the render logs each row it leaves out.
pub async fn materialize_agent_secrets(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
    slug: &str,
) -> Result<Materialized, String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select name, value_enc from agent_secrets where agent_id::text = $1 order by name",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("agent secrets read failed: {e}"))?;
    let agent_dir = talaria_fleet_layout::fleet_dir().join("agents").join(slug);
    let path = agent_dir.join("secrets.env");
    let (lines, mut out) = env_lines(rows, |enc| sb.open(enc).map_err(|e| e.to_string()))?;
    write_sealed_manifest(&agent_dir, &out.sealed).await?;
    if lines.is_empty() {
        // force: a missing file is already the goal.
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        return Ok(out);
    }
    write_0600(
        &path,
        &format!(
            "# Rendered by Talaria — per-agent secrets. Do not hand-edit; edit in Talaria.\n{}\n",
            lines.join("\n")
        ),
    )
    .await?;
    out.env_file = true;
    Ok(out)
}

/// The env_file lines for an agent's rows, and what they hold: a refused
/// name is skipped (with its reason), a `CONFIG_*` name is listed as sealed.
fn env_lines(
    rows: Vec<(String, String)>,
    open: impl Fn(&str) -> Result<String, String>,
) -> Result<(Vec<String>, Materialized), String> {
    let mut out = Materialized::default();
    let mut lines = Vec::with_capacity(rows.len());
    for (name, value_enc) in rows {
        if let Some(why) = reserved(&name) {
            out.skipped.push((name, why));
            continue;
        }
        let value =
            open(&value_enc).map_err(|e| format!("secret \"{name}\" unseal failed: {e}"))?;
        lines.push(env_file::line(&name, &value));
        if name.starts_with(CONFIG_SECRET_PREFIX) {
            out.sealed.push(name);
        }
    }
    Ok((lines, out))
}

/// The manifest is written on every render, with no entries when the agent
/// has no sealed secret, so the mount never changes. World-readable: it
/// holds names only, and Hermes reads it strictly — an unreadable manifest
/// under `plugins/platforms/` fails every command the agent runs. Written by
/// rename into the bind-mounted directory, so a spawn never reads half a file.
async fn write_sealed_manifest(agent_dir: &Path, names: &[String]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let dir = agent_dir.join(SEALED_PLUGIN_DIR);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let _ = tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).await;
    let file = dir.join("plugin.yaml");
    let text = sealed_manifest(names);
    if tokio::fs::read_to_string(&file).await.ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    talaria_fleet_brain::write_atomic(&file, text.as_bytes(), 0o644).await
}

/// Truncate and rewrite, owner-only. One blocking task, the close included:
/// a tokio `File` dropped on the async side closes its descriptor on the
/// runtime's worker, and the last close of a file truncated and rewritten
/// can start its writeback (XFS does), which waits on a saturated disk.
async fn write_0600(path: &Path, content: &str) -> Result<(), String> {
    let (owned, bytes) = (path.to_path_buf(), content.as_bytes().to_vec());
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let at = |e: std::io::Error| format!("{}: {e}", owned.display());
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&owned)
            .map_err(at)?;
        file.write_all(&bytes).map_err(at)?;
        // An install whose file predates the 0600 rule is fixed here.
        // Best-effort, as before.
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        Ok(())
    })
    .await
    .map_err(|e| format!("{}: {e}", path.display()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_names_are_upper_snake_and_never_the_stamped_vars() {
        assert!(name_ok("FIGMA_TOKEN"));
        assert!(name_ok("A1"));
        assert!(!name_ok("figma_token"));
        assert!(!name_ok("A"));
        assert!(!name_ok("A B"));
        assert!(!name_ok("1ABC"));
        assert!(!name_ok(&"A".repeat(65)));
        assert!(reserved("API_SERVER_KEY").is_some());
        assert!(reserved("API_SERVER_MODEL_NAME").is_some());
        assert!(reserved("FIGMA_TOKEN").is_none());
    }

    #[test]
    fn sealed_config_credentials_never_pass_through_to_the_agents_scripts() {
        // A tool key the manager gave the agent passes through.
        assert!(passes_through("FIGMA_TOKEN"));
        assert!(passes_through("STRIPE_API_KEY"));
        // A credential sealed out of the agent's own config is Hermes's,
        // read through `${CONFIG_…}`; listing it would hand it to execute_code.
        for name in [
            "CONFIG_PLATFORMS_TELEGRAM_TOKEN",
            "CONFIG_PROVIDERS_ACME_API_KEY",
            "CONFIG_DASHBOARD_PASSWORD",
            "CONFIG_MCP_SERVERS_LINEAR_HEADERS_AUTHORIZATION",
        ] {
            assert!(
                !passes_through(name),
                "{name} must stay out of env_passthrough"
            );
        }
        // The names config_seal mints all carry the prefix this checks.
        let seal = plan_config_seal(&serde_json::json!({
            "raw": { "platforms": { "telegram": { "enabled": true, "token": "123:abc" } } }
        }))
        .expect("plan");
        assert!(!seal.sealed.is_empty(), "the telegram token is sealed");
        for lit in &seal.sealed {
            assert!(!passes_through(&lit.name), "{} must stay out", lit.name);
        }
        let handles = config_secret_handles(&seal.config);
        assert!(handles.iter().all(|n| !passes_through(n)), "{handles:?}");
        // Reserved names stay out too, as before.
        assert!(!passes_through("OPENAI_API_KEY"));
    }

    #[test]
    fn hermes_names_are_refused_with_the_reason() {
        // What Hermes strips from every command — the omp key bug's class.
        for name in [
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "TELEGRAM_BOT_TOKEN",
            "SLACK_BOT_TOKEN",
            "SEARXNG_URL",
        ] {
            let why = reserved(name).unwrap_or_else(|| panic!("{name} must be refused"));
            assert!(why.contains("Hermes strips it"), "{name}: {why}");
        }
        // Hermes's own runtime settings, by prefix.
        for name in [
            "HERMES_HOME",
            "HERMES_REDACT_SECRETS",
            "TERMINAL_ENV",
            "TERMINAL_SSH_HOST",
            "GATEWAY_MULTIPLEX_PROFILES",
            "AUXILIARY_VISION_API_KEY",
        ] {
            let why = reserved(name).unwrap_or_else(|| panic!("{name} must be refused"));
            assert!(why.contains("Hermes setting"), "{name}: {why}");
        }
    }

    #[test]
    fn talarias_own_names_are_refused() {
        for name in [
            "TALARIA_AGENT_KEY",
            "TALARIA_WORKBENCH_API_KEY",
            "TALARIA_API_URL",
            "LLM_API_KEY",
            "LLM_WORKBENCH_API_KEY",
            "PI_CODING_AGENT_DIR",
            "PI_SMOL_MODEL",
            "OMP_AUTH_BROKER_TOKEN",
            "GIT_AUTHOR_EMAIL",
            "XDG_DATA_HOME",
            "PLAYWRIGHT_BROWSERS_PATH",
        ] {
            let why = reserved(name).unwrap_or_else(|| panic!("{name} must be refused"));
            assert!(why.contains("managed by Talaria"), "{name}: {why}");
        }
    }

    #[test]
    fn the_containers_process_settings_are_refused() {
        for name in [
            "PATH",
            "HOME",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "PYTHONPATH",
            "NODE_OPTIONS",
            "BASH_ENV",
            "S6_KEEP_ENV",
            "S6_CMD_WAIT_FOR_SERVICES_MAXTIME",
            "AGENT_BROWSER_EXECUTABLE_PATH",
        ] {
            let why = reserved(name).unwrap_or_else(|| panic!("{name} must be refused"));
            assert!(
                why.contains("configures the agent's runtime"),
                "{name}: {why}"
            );
        }
    }

    #[test]
    fn a_legacy_row_under_a_refused_name_is_marked_on_the_wire() {
        let meta = |name: &str| AgentSecretMeta {
            refused: reserved(name),
            used_at: None,
            name: name.into(),
            updated_by: Some("a@b.c".into()),
            updated_at: None,
        };
        let legacy = serde_json::to_value(meta("OPENROUTER_API_KEY")).unwrap();
        assert!(
            legacy["refused"]
                .as_str()
                .is_some_and(|w| w.contains("Hermes strips it")),
            "{legacy}"
        );
        // An ordinary secret's wire shape is unchanged: no `refused` key.
        let ok = serde_json::to_value(meta("FIGMA_TOKEN")).unwrap();
        assert!(ok.get("refused").is_none(), "{ok}");
    }

    #[test]
    fn an_ordinary_vendor_key_is_still_a_secret() {
        // The feature's point: a key for the agent's own tools passes.
        for name in [
            "FIGMA_TOKEN",
            "STRIPE_API_KEY",
            "LINEAR_API_KEY",
            "MY_OPENAI_KEY",
            "NOTION_TOKEN",
            "GITHUB_PAT_FOR_SCRIPTS",
        ] {
            assert_eq!(reserved(name), None, "{name} is the agent's own");
        }
    }

    #[test]
    fn the_hermes_snapshot_is_loaded_and_clean() {
        let names: Vec<&str> = hermes_stripped_names().collect();
        assert!(names.len() > 400, "the snapshot loaded: {}", names.len());
        for n in &names {
            assert!(name_ok(n), "{n} is an env name");
            assert!(!n.starts_with('#'));
        }
        // Not Hermes's: the names Talaria hands the sandbox must survive the
        // scrub, or the tool that reads them never sees them.
        for ours in [
            "TALARIA_WORKBENCH_API_KEY",
            "TALARIA_AGENT_KEY",
            "LLM_API_KEY",
        ] {
            assert!(!names.contains(&ours), "Hermes strips {ours}");
        }
    }

    #[test]
    fn a_refused_name_is_not_loaded_and_sealed_names_are_listed() {
        let rows = vec![
            (
                "CONFIG_PLATFORMS_TELEGRAM_TOKEN".to_string(),
                "enc:tg".to_string(),
            ),
            ("FIGMA_TOKEN".to_string(), "enc:figma".to_string()),
            ("TAVILY_API_KEY".to_string(), "enc:tavily".to_string()),
            ("TERMINAL_ENV".to_string(), "enc:ssh".to_string()),
        ];
        let open = |enc: &str| Ok(enc.trim_start_matches("enc:").to_string());
        let (lines, out) = env_lines(rows, open).expect("lines");
        assert_eq!(
            lines,
            vec!["CONFIG_PLATFORMS_TELEGRAM_TOKEN=tg", "FIGMA_TOKEN=figma"]
        );
        assert_eq!(out.sealed, vec!["CONFIG_PLATFORMS_TELEGRAM_TOKEN"]);
        let skipped: Vec<&str> = out.skipped.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(skipped, vec!["TAVILY_API_KEY", "TERMINAL_ENV"]);
        assert!(
            out.skipped[0].1.contains("Hermes strips it"),
            "{:?}",
            out.skipped
        );
        // Every row refused: nothing to load, so no env_file line at all.
        let (lines, out) = env_lines(
            vec![("OPENAI_API_KEY".to_string(), "enc:sk".to_string())],
            open,
        )
        .unwrap();
        assert!(lines.is_empty() && out.sealed.is_empty());
        assert_eq!(out.skipped.len(), 1);
        // An unsealable row still fails, naming it.
        let err = env_lines(vec![("FIGMA_TOKEN".to_string(), "x".to_string())], |_| {
            Err("bad tag".to_string())
        })
        .unwrap_err();
        assert!(
            err.contains("FIGMA_TOKEN") && err.contains("bad tag"),
            "{err}"
        );
    }

    #[test]
    fn the_sealed_manifest_declares_each_name_as_a_password_and_nothing_else() {
        let names = vec![
            "CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD".to_string(),
            "CONFIG_PLATFORMS_TELEGRAM_TOKEN".to_string(),
        ];
        let text = sealed_manifest(&names);
        assert!(text.contains("\nname: talaria-sealed\n"), "{text}");
        assert!(text.contains("\nkind: platform\n"), "{text}");
        assert!(
            text.contains(
                "optional_env:\n  - name: CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD\n    password: true\n  - name: CONFIG_PLATFORMS_TELEGRAM_TOKEN\n    password: true\n"
            ),
            "{text}"
        );
        // No adapter to load and nothing Hermes would read as one: no
        // entry point, no requires_env, no category other than the default.
        for absent in ["requires_env", "category", "entry", "provides_"] {
            assert!(!text.contains(absent), "{absent} in {text}");
        }
        // The empty case still renders, so the mount never changes.
        let empty = sealed_manifest(&[]);
        assert!(empty.contains("optional_env: []\n"), "{empty}");
        assert!(!empty.contains("password: true"));
        assert!(SEALED_PLUGIN_MOUNT.starts_with("/opt/data/plugins/platforms/"));
        assert!(SEALED_PLUGIN_MOUNT.ends_with("/talaria-sealed"));
    }
}
