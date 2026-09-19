// PROJECT ENV STORES — the dev-env half of the secret system.
//
// A workspace_secret is a credential an agent SPENDS and never reads; a
// repo_env entry is a value the agent's BUILD reads, the way a developer's
// own .env works. The split is the security model: nothing that must stay
// out of a model's context belongs here, because the fleet render
// materializes these values into the agent's container by design. Sealing
// uses the same envelope (a database dump is not a credential dump); access
// control is the existing repo grant — an agent granted the repo gets its
// env file, nobody else gets anything.

use sqlx::PgPool;
use std::sync::OnceLock;

use regex::Regex;

fn env_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("^[A-Z_][A-Z0-9_]*$").expect("the env-key pattern compiles"))
}

/// The one validation law a key carries: it must be a legal shell env name,
/// because the file is sourced. A key that isn't would silently break the
/// whole file at source time — the worst place to discover it.
pub fn valid_env_key(key: &str) -> bool {
    key.len() <= 100 && env_key_re().is_match(key)
}

/// Patch one repo's store: `set` seals and writes (an empty value DELETES —
/// the UI's clear affordance), `delete` removes by name. Both run in one
/// transaction so a partial write can never leave a half-updated env.
pub async fn patch_env(
    pg: &PgPool,
    sb: &talaria_secretbox::SecretBox,
    repo: &str,
    actor: &str,
    set: &[(String, String)],
    delete: &[String],
) -> Result<(), String> {
    for (k, _) in set {
        if !valid_env_key(k) {
            return Err(format!(
                "\"{k}\" is not a valid environment variable name (A-Z, 0-9, _, no leading digit)"
            ));
        }
    }
    let mut tx = pg.begin().await.map_err(|e| format!("repo env tx: {e}"))?;
    for (k, v) in set {
        if v.is_empty() {
            sqlx::query("delete from repo_env where repo = $1 and key = $2")
                .bind(repo)
                .bind(k)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("repo env delete: {e}"))?;
        } else {
            let cipher = sb.seal(v).map_err(|e| format!("repo env seal: {e}"))?;
            sqlx::query(
                "insert into repo_env (repo, key, value_cipher, created_by) \
                 values ($1, $2, $3, $4) \
                 on conflict (repo, key) do update \
                 set value_cipher = excluded.value_cipher, updated_at = now()",
            )
            .bind(repo)
            .bind(k)
            .bind(&cipher)
            .bind(actor)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("repo env write: {e}"))?;
        }
    }
    for k in delete {
        sqlx::query("delete from repo_env where repo = $1 and key = $2")
            .bind(repo)
            .bind(k)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("repo env delete: {e}"))?;
    }
    tx.commit()
        .await
        .map_err(|e| format!("repo env commit: {e}"))
}

/// The store's NAMES, never its values — the UI reads this. The value half
/// leaves the database exactly once, inside the render's file write.
pub async fn env_keys(pg: &PgPool, repo: &str) -> Vec<String> {
    let Ok(rows) =
        sqlx::query_scalar::<_, String>("select key from repo_env where repo = $1 order by key")
            .bind(repo)
            .fetch_all(pg)
            .await
    else {
        return Vec::new();
    };
    rows
}

/// One repo's env as dotenv FILE BYTES: every value single-quoted with the
/// quote itself escaped, so `source` can never misread a value as syntax.
/// None when the store is empty — an absent file and an empty one are the
/// same fact to the agent, and absence is the cleaner one.
pub async fn env_file(
    pg: &PgPool,
    sb: &talaria_secretbox::SecretBox,
    repo: &str,
) -> Result<Option<String>, String> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("select key, value_cipher from repo_env where repo = $1 order by key")
            .bind(repo)
            .fetch_all(pg)
            .await
            .map_err(|e| format!("repo env read: {e}"))?;
    if rows.is_empty() {
        return Ok(None);
    }
    let mut out = String::new();
    for (key, cipher) in rows {
        let value = sb
            .open(&cipher)
            .map_err(|e| format!("repo env unseal for {repo}/{key}: {e}"))?;
        out.push_str(&format!("{key}='{}'\n", value.replace('\'', "'\\''")));
    }
    Ok(Some(out))
}

/// The repos with a non-empty store — the render's question.
pub async fn repos_with_env(pg: &PgPool) -> Vec<String> {
    let Ok(rows) =
        sqlx::query_scalar::<_, String>("select distinct repo from repo_env order by repo")
            .fetch_all(pg)
            .await
    else {
        return Vec::new();
    };
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_keys_are_shell_legal_or_refused() {
        assert!(valid_env_key("API_KEY"));
        assert!(valid_env_key("A"));
        assert!(valid_env_key("X_1"));
        // A leading digit, a dash, a space and an empty string are the
        // shapes a paste produces; all refused, each for the same reason.
        assert!(!valid_env_key("1BAD"));
        assert!(!valid_env_key("BAD-KEY"));
        assert!(!valid_env_key("BAD KEY"));
        assert!(!valid_env_key(""));
        assert!(!valid_env_key(&"K".repeat(101)));
    }
}
