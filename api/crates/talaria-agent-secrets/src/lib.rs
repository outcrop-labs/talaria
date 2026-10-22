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

use sqlx::PgPool;
use std::path::Path;

use talaria_secretbox::SecretBox;

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
/// script reads. Three kinds of name are not that: Talaria's (stamped on the
/// container, which wins over the secret), Hermes's runtime settings (they
/// reconfigure the agent), and the names Hermes strips from every command the
/// agent runs (the secret would never reach a tool, and Hermes may read it
/// itself: an `OPENROUTER_API_KEY` is a route to a model around the gateway,
/// a `TELEGRAM_BOT_TOKEN` turns on a Telegram adapter).
pub fn reserved(name: &str) -> Option<String> {
    if TALARIA_STAMPED.contains(&name) || TALARIA_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return Some(format!("{name} is managed by Talaria"));
    }
    if let Some(prefix) = HERMES_PREFIXES.iter().find(|p| name.starts_with(*p)) {
        return Some(format!(
            "{name} is a Hermes setting ({prefix}* configures the agent's runtime), not a secret for its tools"
        ));
    }
    if hermes_stripped().contains(name) {
        return Some(format!(
            "{name} is one of Hermes's own credential or platform names: Hermes strips it from every command the agent runs and may use it itself, around Talaria. Give the secret a name of your own"
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
    Ok(rows
        .into_iter()
        .map(|(name, updated_by, at)| AgentSecretMeta {
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
    Ok(names
        .into_iter()
        .filter(|n| reserved(n).is_none())
        .collect())
}

/// Write (or remove) the agent's secrets env file for the renderer. Returns
/// whether the service should declare an env_file. An unsealable row fails
/// the render — a secret the server can no longer open must not silently
/// vanish from a container that runs on it.
pub async fn materialize_agent_secrets(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
    slug: &str,
) -> Result<bool, String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select name, value_enc from agent_secrets where agent_id::text = $1 order by name",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("agent secrets read failed: {e}"))?;
    let path = talaria_fleet_layout::fleet_dir()
        .join("agents")
        .join(slug)
        .join("secrets.env");
    if rows.is_empty() {
        // force: a missing file is already the goal.
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        return Ok(false);
    }
    let mut lines = Vec::with_capacity(rows.len());
    for (name, value_enc) in rows {
        let value = sb
            .open(&value_enc)
            .map_err(|e| format!("secret \"{name}\" unseal failed: {e}"))?;
        lines.push(format!("{name}={value}"));
    }
    write_0600(
        &path,
        &format!(
            "# Rendered by Talaria — per-agent secrets. Do not hand-edit; edit in Talaria.\n{}\n",
            lines.join("\n")
        ),
    )
    .await?;
    Ok(true)
}

async fn write_0600(path: &Path, content: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(content.as_bytes())
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = tokio::fs::metadata(path).await {
        let mut p = meta.permissions();
        p.set_mode(0o600);
        let _ = tokio::fs::set_permissions(path, p).await;
    }
    Ok(())
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
}
