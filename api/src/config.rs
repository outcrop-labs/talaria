// Boot configuration, read once. The philosophy is ui/src/server/env.ts's:
// collect EVERY problem and fail once with the full list, so an operator
// fixing config one error at a time isn't told about them one at a time.

use std::net::SocketAddr;

pub const DEFAULT_PORT: u16 = 5274;

#[derive(Debug)]
pub struct Config {
    pub database_url: String,
    pub redis_url: String,
    /// The secretbox root material. NEVER logged, never serialized — only its
    /// provenance is (`root_source`), which is how secretbox.ts's `rootSource`
    /// reports too.
    pub secret_root: SecretRoot,
    pub bind: SocketAddr,
}

#[derive(Debug)]
pub struct SecretRoot {
    material: String,
    source: RootSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    /// TALARIA_SECRET_KEY — the dedicated, stable root. The only safe one.
    SecretKey,
    /// TALARIA_SECRET_KEY_FILE contents (read + trimmed).
    SecretKeyFile,
    /// AUTH_SECRET doing double duty. Works, but AUTH_SECRET's own docs call
    /// it safe to rotate — warn, like secretbox.ts does at boot.
    AuthSecretFallback,
}

impl SecretRoot {
    /// DELETE THIS ALLOW with the phase-2 relay (reaches the material via
    /// AppState::secretbox); only provenance is read until then.
    #[allow(dead_code)]
    pub fn material(&self) -> &str {
        &self.material
    }
    pub fn source(&self) -> RootSource {
        self.source
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_parts(
            env("DATABASE_URL"),
            env("REDIS_URL"),
            env("TALARIA_SECRET_KEY"),
            env("TALARIA_SECRET_KEY_FILE"),
            env("AUTH_SECRET"),
            env("TALARIA_API_PORT"),
        )
    }

    /// Named `from_parts` (not `from`) so it can never collide with the
    /// blanket `From` impl at the `Self::…` call site above. Public because
    /// integration tests assemble configs without touching the environment.
    pub fn from_parts(
        database_url: String,
        redis_url: String,
        secret_key: String,
        secret_key_file: String,
        auth_secret: String,
        port: String,
    ) -> Result<Self, String> {
        let mut problems: Vec<String> = Vec::new();

        if database_url.is_empty() {
            problems.push("DATABASE_URL is required (postgres://…)".into());
        } else if !database_url.starts_with("postgres://")
            && !database_url.starts_with("postgresql://")
        {
            problems.push("DATABASE_URL must start with postgres:// or postgresql://".into());
        }

        if redis_url.is_empty() {
            problems.push("REDIS_URL is required (redis://…)".into());
        } else if !redis_url.starts_with("redis://") && !redis_url.starts_with("rediss://") {
            problems.push("REDIS_URL must start with redis:// or rediss://".into());
        }

        // Root precedence is secretbox.ts's, verbatim: SECRET_KEY, then the
        // FILE contents, then AUTH_SECRET. The file is only read when the env
        // value is absent, so a broken file path only matters when it matters.
        let (material, source) = if !secret_key.is_empty() {
            (secret_key, RootSource::SecretKey)
        } else if !secret_key_file.is_empty() {
            match std::fs::read_to_string(&secret_key_file) {
                Ok(contents) => (contents.trim().to_string(), RootSource::SecretKeyFile),
                Err(e) => {
                    problems.push(format!(
                        "TALARIA_SECRET_KEY_FILE ({secret_key_file}) could not be read: {e}"
                    ));
                    (String::new(), RootSource::SecretKey)
                }
            }
        } else if !auth_secret.is_empty() {
            (auth_secret, RootSource::AuthSecretFallback)
        } else {
            problems.push(
                "an encryption root is required: set TALARIA_SECRET_KEY (or TALARIA_SECRET_KEY_FILE, \
                 or AUTH_SECRET) — it must match what sealed this database's secrets"
                    .into(),
            );
            (String::new(), RootSource::SecretKey)
        };

        let port = if port.is_empty() {
            DEFAULT_PORT
        } else {
            match port.parse::<u16>() {
                Ok(p) => p,
                Err(_) => {
                    problems.push(format!(
                        "TALARIA_API_PORT ({port}) is not a valid port number"
                    ));
                    DEFAULT_PORT
                }
            }
        };

        if !problems.is_empty() {
            return Err(format!(
                "talaria-api: cannot start — {} problem(s):\n  - {}",
                problems.len(),
                problems.join("\n  - ")
            ));
        }

        Ok(Config {
            database_url,
            redis_url,
            secret_root: SecretRoot { material, source },
            bind: SocketAddr::from(([127, 0, 0, 1], port)),
        })
    }
}

/// process.env with empty strings normalized to unset, like env.ts.
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> [String; 6] {
        [
            "postgres://t:t@127.0.0.1:5544/t".into(),
            "redis://127.0.0.1:6399".into(),
            "root-secret".into(),
            String::new(),
            String::new(),
            String::new(),
        ]
    }

    #[test]
    fn aggregates_every_problem_at_once() {
        let [_, rd, sk, skf, auth, port] = base();
        let err = Config::from_parts(String::new(), rd, sk, skf, auth, port).unwrap_err();
        assert!(err.contains("DATABASE_URL"), "{err}");
        let [db, _, _, _, _, _] = base();
        let err = Config::from_parts(
            db,
            "http://nope".into(),
            String::new(),
            String::new(),
            String::new(),
            "nope".into(),
        )
        .unwrap_err();
        // All three failures named in one error, not the first one.
        assert!(err.contains("REDIS_URL must start"), "{err}");
        assert!(err.contains("encryption root"), "{err}");
        assert!(err.contains("TALARIA_API_PORT"), "{err}");
    }

    #[test]
    fn secret_root_precedence_matches_secretbox_ts() {
        let [db, rd, _, _, _, port] = base();
        let c = Config::from_parts(
            db.clone(),
            rd.clone(),
            "a".into(),
            "/nonexistent".into(),
            "c".into(),
            port.clone(),
        )
        .unwrap();
        assert_eq!(c.secret_root.source(), RootSource::SecretKey);
        assert_eq!(c.secret_root.material(), "a");

        // Empty strings are unset (env() trims at its edge, so a blank value
        // arrives here as ""): AUTH_SECRET wins only when the others are absent.
        let [db, rd, _, _, _, port] = base();
        let c = Config::from_parts(
            db,
            rd,
            String::new(),
            String::new(),
            "fallback".into(),
            port,
        )
        .unwrap();
        assert_eq!(c.secret_root.source(), RootSource::AuthSecretFallback);
        assert_eq!(c.secret_root.material(), "fallback");

        let [db, rd, sk, _, _, _] = base();
        let c =
            Config::from_parts(db, rd, sk, String::new(), String::new(), String::new()).unwrap();
        assert_eq!(c.bind.port(), DEFAULT_PORT);
    }
}
