// Per-agent secrets — env vars an agent needs that don't belong in the shared
// fleet .env (a Figma token for one agent, a vendor key for another). Set in
// the UI, stored encrypted (secretbox), and materialized ONLY at render time
// into fleet/agents/<slug>/secrets.env (0600), which the rendered service
// loads via env_file. Values are write-only through the API: names and
// timestamps list; plaintext never leaves the server.

use sqlx::PgPool;
use std::path::Path;

use crate::secretbox::SecretBox;

/// Container-env name: UPPER_SNAKE, must not collide with the stamped vars.
fn name_ok(name: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[A-Z][A-Z0-9_]{1,63}$").unwrap())
        .is_match(name)
}

fn reserved(name: &str) -> bool {
    name == "API_SERVER_KEY" || name == "API_SERVER_MODEL_NAME"
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
            updated_at: at.map(crate::agent_auth::epoch_ms_to_iso),
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
    if reserved(name) {
        return Err(format!("{name} is managed by Talaria"));
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
    sqlx::query(
        "insert into agent_secrets (agent_id, name, value_enc, updated_by) \
         values ($1::uuid, $2, $3, $4::uuid) \
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
    let path = crate::fleet::layout::fleet_dir()
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

/// Write mode 0600: create-mode only applies to new files, so an existing
/// file is chmod'd explicitly.
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
        assert!(reserved("API_SERVER_KEY"));
        assert!(reserved("API_SERVER_MODEL_NAME"));
        assert!(!reserved("FIGMA_TOKEN"));
    }
}
