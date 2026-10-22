// Literal credentials inside an agent's own config. A Hermes config can carry
// a credential inline — a platform token, a named provider's key, a webhook
// secret, an Authorization header. Stored as written it sat in
// `agent_versions` in plaintext, was served back by the versions and history
// reads, and was re-emitted into the 0644 config.yaml the render writes —
// against the rule that secrets live envelope-encrypted in Postgres
// (docs/ENCRYPTION.md).
//
// So a config that arrives from outside (federation) is sealed before it is
// stored. Each literal the render would emit becomes one of the agent's own
// per-agent secrets, `CONFIG_<PATH>`, and the config keeps a `${CONFIG_<PATH>}`
// handle in its place. Nothing else has to change for the agent to keep the
// value: the render already delivers per-agent secrets through the service's
// env_file, and Hermes expands `${VAR}` in every string of config.yaml when it
// loads it (v0.22.4 `hermes_cli/config.py` `_expand_env_vars`) — the same way
// the rendered `${LLM_API_KEY}` and `${TALARIA_AGENT_KEY}` already resolve.
//
// Two kinds of literal are dropped instead, because the render never emits
// them and sealing one would hand the agent a credential it does not get
// today:
//
// - a model spec's `api_key`. The render points every spec at the gateway on
//   the agent's own key (talaria_fleet_brain::route_config_through_gateway),
//   so the stored value becomes that same `${LLM_API_KEY}`. The upstream key
//   belongs on the endpoint, entered on Models.
// - anything under the authored top-level `mcp_servers`. The render builds
//   that block fresh, every server through the MCP gateway, and an upstream
//   MCP credential belongs in the MCP registry.

use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashSet;

use talaria_secretbox::SecretBox;

/// The prefix of a per-agent secret sealed out of the agent's own config.
pub const CONFIG_SECRET_PREFIX: &str = "CONFIG_";

/// One literal credential found in a config, to seal under the agent.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigLiteral {
    /// The per-agent secret it becomes (`CONFIG_PLATFORMS_TELEGRAM_TOKEN`).
    pub name: String,
    /// Where it sat in the raw config (`platforms.telegram.token`).
    pub path: String,
    pub value: String,
}

/// What sealing a config's literals does, worked out before anything is
/// written — so a config that cannot be sealed is refused while nothing
/// exists yet.
#[derive(Debug, Clone)]
pub struct ConfigSeal {
    /// The config to store: every sealed literal replaced by its handle, every
    /// dropped one replaced or removed.
    pub config: Value,
    pub sealed: Vec<ConfigLiteral>,
    /// Where each dropped literal sat in the raw config.
    pub dropped: Vec<String>,
}

/// Whether a config key names a credential, on the key's last word: `api_key`,
/// `bot_token`, `client_secret`, `password`, an `X-Api-Key` or `Authorization`
/// header. `max_tokens` (plural), `key_env` and `access_token_env` (env var
/// NAMES), `token_url` and `password_hash` do not count.
fn credential_key(key: &str) -> bool {
    const EXACT: &[&str] = &["authorization", "proxy_authorization", "cookie"];
    const LAST_WORD: &[&str] = &[
        "api_key",
        "apikey",
        "token",
        "secret",
        "password",
        "passwd",
        "private_key",
        "secret_key",
        "access_key",
        "signing_key",
        "encryption_key",
    ];
    let k = key.to_ascii_lowercase().replace('-', "_");
    EXACT.contains(&k.as_str()) || LAST_WORD.iter().any(|w| k.ends_with(w))
}

/// The literal behind a credential key, or None: a reference (`${VAR}`, or a
/// template such as `Bearer ${TOKEN}`) is already a handle, an empty string
/// holds nothing, and a non-string is not a credential.
fn literal<'a>(key: &str, value: &'a Value) -> Option<&'a str> {
    let s = value.as_str()?;
    (credential_key(key) && !s.trim().is_empty() && !s.contains("${")).then_some(s)
}

#[derive(Debug, Clone)]
enum Seg {
    Key(String),
    Index(usize),
}

fn dotted(path: &[Seg]) -> String {
    let mut out = String::new();
    for seg in path {
        match seg {
            Seg::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
            Seg::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// FNV-1a — a stable tag for a path whose name would not fit or collides.
fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The longest value, and the characters, a per-agent secret can carry —
/// the same limits `set_agent_secret` holds a typed secret to, because both
/// end up as one env_file line.
fn deliverable(path: &str, value: &str) -> Result<(), String> {
    let why = if value.len() > 8192 {
        "longer than 8 KB"
    } else if value.contains(['\n', '\r', '\0']) {
        "it spans more than one line"
    } else {
        return Ok(());
    };
    Err(format!(
        "{path} holds a literal credential that cannot be sealed as a per-agent secret ({why}) — replace it with a ${{VAR}} reference and set VAR on the agent's Secrets tab"
    ))
}

struct Planner {
    sealed: Vec<ConfigLiteral>,
    dropped: Vec<String>,
    names: HashSet<String>,
}

impl Planner {
    /// `CONFIG_` + the path in UPPER_SNAKE; past 64 characters, or on a clash
    /// with another path's name, the tail becomes a hash of the path.
    fn name_for(&mut self, path: &[Seg]) -> String {
        let mut body = String::new();
        for seg in path {
            let text = match seg {
                Seg::Key(k) => k.clone(),
                Seg::Index(i) => i.to_string(),
            };
            body.extend(text.chars().map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            }));
            body.push('_');
        }
        let mut squeezed = String::with_capacity(body.len());
        for c in body.chars() {
            if !(c == '_' && squeezed.ends_with('_')) {
                squeezed.push(c);
            }
        }
        let mut name = format!("{CONFIG_SECRET_PREFIX}{}", squeezed.trim_matches('_'));
        if name.len() > 64 || self.names.contains(&name) {
            let tag = format!("_{:08X}", fnv1a(&dotted(path)));
            name.truncate(64 - tag.len());
            let trimmed = name.trim_end_matches('_').len();
            name.truncate(trimmed);
            name.push_str(&tag);
        }
        self.names.insert(name.clone());
        name
    }

    fn seal(&mut self, path: &[Seg], value: &str) -> Result<String, String> {
        let at = dotted(path);
        deliverable(&at, value)?;
        let name = self.name_for(path);
        if !crate::name_ok(&name) {
            return Err(format!("{at}: no secret name fits this path"));
        }
        self.sealed.push(ConfigLiteral {
            name: name.clone(),
            path: at,
            value: value.to_string(),
        });
        Ok(format!("${{{name}}}"))
    }

    /// The render's walk, mirrored: a model spec's `api_key` is the one key
    /// it overwrites, and it copies everything else inside a spec as written
    /// — a spec-shaped object nested in a spec gets no gateway treatment, so
    /// its literals are sealed like any other.
    fn walk(&mut self, v: &mut Value, path: &mut Vec<Seg>, in_spec: bool) -> Result<(), String> {
        match v {
            Value::Array(items) => {
                for (i, item) in items.iter_mut().enumerate() {
                    path.push(Seg::Index(i));
                    self.walk(item, path, in_spec)?;
                    path.pop();
                }
            }
            Value::Object(_) => {
                let spec = !in_spec && talaria_fleet_brain::is_model_spec(v);
                let Value::Object(map) = v else {
                    unreachable!("matched as an object above")
                };
                for (key, child) in map.iter_mut() {
                    path.push(Seg::Key(key.clone()));
                    if let Some(lit) = literal(key, child) {
                        if spec && key == "api_key" {
                            self.dropped.push(dotted(path));
                            *child = Value::String(talaria_fleet_brain::GATEWAY_API_KEY.into());
                        } else {
                            let handle = self.seal(path, lit)?;
                            *child = Value::String(handle);
                        }
                    } else {
                        self.walk(child, path, in_spec || spec)?;
                    }
                    path.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Authored MCP entries are never rendered: their literal credentials go.
    fn strip(&mut self, v: &mut Value, path: &mut Vec<Seg>) {
        match v {
            Value::Array(items) => {
                for (i, item) in items.iter_mut().enumerate() {
                    path.push(Seg::Index(i));
                    self.strip(item, path);
                    path.pop();
                }
            }
            Value::Object(map) => {
                let gone: Vec<String> = map
                    .iter()
                    .filter(|(k, val)| literal(k, val).is_some())
                    .map(|(k, _)| k.clone())
                    .collect();
                for key in gone {
                    map.remove(&key);
                    path.push(Seg::Key(key));
                    self.dropped.push(dotted(path));
                    path.pop();
                }
                for (key, child) in map.iter_mut() {
                    path.push(Seg::Key(key.clone()));
                    self.strip(child, path);
                    path.pop();
                }
            }
            _ => {}
        }
    }
}

/// Work out how to seal the literal credentials in a stored agent config
/// (`{ main, aliases, …, raw }` — only `raw`, the Hermes config, carries
/// any). Pure: nothing is written. Refuses a literal no per-agent secret can
/// carry, naming where it sat.
pub fn plan_config_seal(config: &Value) -> Result<ConfigSeal, String> {
    let mut planner = Planner {
        sealed: Vec::new(),
        dropped: Vec::new(),
        names: HashSet::new(),
    };
    let mut out = config.clone();
    if let Some(raw @ Value::Object(_)) = out.get_mut("raw") {
        let mut path = Vec::new();
        // The MCP block first: once its literals are gone, the walk below
        // finds nothing left in it to seal.
        if let Some(servers) = raw.get_mut("mcp_servers") {
            path.push(Seg::Key("mcp_servers".into()));
            planner.strip(servers, &mut path);
            path.pop();
        }
        // From the root, as the render walks it — a legacy config whose top
        // level is itself a model spec gets the same treatment as a nested one.
        planner.walk(raw, &mut path, false)?;
    }
    Ok(ConfigSeal {
        config: out,
        sealed: planner.sealed,
        dropped: planner.dropped,
    })
}

/// Seal a plan's literals under the agent, all or none. A name the agent
/// already holds is overwritten: the config being stored is what the value
/// came from.
pub async fn seal_config_literals(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
    sealed: &[ConfigLiteral],
    actor: Option<&str>,
) -> Result<(), String> {
    if sealed.is_empty() {
        return Ok(());
    }
    let mut tx = pg
        .begin()
        .await
        .map_err(|e| format!("secret write failed: {e}"))?;
    for lit in sealed {
        let enc = sb
            .seal(&lit.value)
            .map_err(|e| format!("{}: secret seal failed: {e}", lit.path))?;
        sqlx::query(
            "insert into agent_secrets (agent_id, name, value_enc, updated_by) \
             values ($1::uuid, $2, $3, $4) \
             on conflict (agent_id, name) do update \
             set value_enc = excluded.value_enc, updated_by = excluded.updated_by, updated_at = now()",
        )
        .bind(agent_id)
        .bind(&lit.name)
        .bind(&enc)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("secret write failed: {e}"))?;
    }
    tx.commit()
        .await
        .map_err(|e| format!("secret write failed: {e}"))
}

/// Every `${CONFIG_…}` handle a config carries, first-seen order.
pub fn config_secret_handles(config: &Value) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\$\{(CONFIG_[A-Z0-9_]{1,57})\}").unwrap());
    fn walk(v: &Value, re: &regex::Regex, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                for cap in re.captures_iter(s) {
                    let name = cap[1].to_string();
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|i| walk(i, re, out)),
            Value::Object(map) => map.values().for_each(|i| walk(i, re, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(config, re, &mut out);
    out
}

/// A config copied from one agent to another (a hire from a template) takes
/// the sealed literals its handles name with it — copied as ciphertext, so
/// nothing is opened. Without them the new agent's handles would resolve to
/// nothing. A name the new agent already holds is left alone.
pub async fn copy_config_secrets(
    pg: &PgPool,
    from_agent: &str,
    to_agent: &str,
    config: &Value,
) -> Result<(), String> {
    let names = config_secret_handles(config);
    if names.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "insert into agent_secrets (agent_id, name, value_enc, updated_by) \
         select $2::uuid, name, value_enc, updated_by from agent_secrets \
         where agent_id = $1::uuid and name = any($3) \
         on conflict (agent_id, name) do nothing",
    )
    .bind(from_agent)
    .bind(to_agent)
    .bind(&names)
    .execute(pg)
    .await
    .map_err(|e| format!("agent secrets copy failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(raw: Value) -> ConfigSeal {
        plan_config_seal(&json!({ "main": { "endpoint": "e", "model": "m" }, "raw": raw }))
            .expect("plans")
    }

    #[test]
    fn credential_keys_are_read_on_their_last_word() {
        for k in [
            "api_key",
            "token",
            "bot_token",
            "access_token",
            "client_secret",
            "signing_secret",
            "password",
            "private_key",
            "Authorization",
            "X-Api-Key",
            "x-auth-token",
            "GITHUB_TOKEN",
            "aws_secret_access_key",
        ] {
            assert!(credential_key(k), "{k}");
        }
        for k in [
            "max_tokens",
            "threshold_tokens",
            "key_env",
            "api_key_env",
            "access_token_env",
            "service_account_token_env",
            "token_url",
            "password_hash",
            "secrets",
            "redact_secrets",
            "auth",
            "key",
            "X-Agent-Name",
        ] {
            assert!(!credential_key(k), "{k}");
        }
    }

    #[test]
    fn rendered_literals_become_handles_and_their_values_are_sealed() {
        let out = plan(json!({
            "platforms": { "telegram": { "enabled": true, "token": "123:abc" } },
            "providers": { "lan-proxy": { "base_url": "http://proxy.lan/v1", "api_key": "sk-proxy" } },
            "dashboard": { "basic_auth": { "username": "ops", "password": "pa$$ #word" } },
        }));
        assert_eq!(
            out.config["raw"]["platforms"]["telegram"]["token"],
            json!("${CONFIG_PLATFORMS_TELEGRAM_TOKEN}")
        );
        assert_eq!(
            out.config["raw"]["platforms"]["telegram"]["enabled"],
            json!(true)
        );
        assert_eq!(
            out.config["raw"]["providers"]["lan-proxy"]["api_key"],
            json!("${CONFIG_PROVIDERS_LAN_PROXY_API_KEY}")
        );
        assert_eq!(
            out.config["raw"]["providers"]["lan-proxy"]["base_url"],
            json!("http://proxy.lan/v1")
        );
        assert_eq!(
            out.config["raw"]["dashboard"]["basic_auth"]["username"],
            json!("ops")
        );
        let by_name: Vec<(&str, &str, &str)> = out
            .sealed
            .iter()
            .map(|l| (l.name.as_str(), l.path.as_str(), l.value.as_str()))
            .collect();
        assert_eq!(
            by_name,
            vec![
                (
                    "CONFIG_PLATFORMS_TELEGRAM_TOKEN",
                    "platforms.telegram.token",
                    "123:abc"
                ),
                (
                    "CONFIG_PROVIDERS_LAN_PROXY_API_KEY",
                    "providers.lan-proxy.api_key",
                    "sk-proxy"
                ),
                (
                    "CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD",
                    "dashboard.basic_auth.password",
                    "pa$$ #word"
                ),
            ]
        );
        assert!(out.dropped.is_empty());
        // The structured half carries no credential and is untouched.
        assert_eq!(out.config["main"], json!({ "endpoint": "e", "model": "m" }));
    }

    #[test]
    fn references_empties_and_non_strings_stay_as_written() {
        let raw = json!({
            "providers": { "a": { "base_url": "u", "api_key": "${OPENROUTER_API_KEY}" } },
            "auxiliary": { "vision": { "headers": { "Authorization": "Bearer ${VISION_TOKEN}" } } },
            "dashboard": { "basic_auth": { "password": "", "secret": "   " } },
            "agent": { "max_tokens": 4096 },
            "platforms": { "x": { "token": 12345 } },
        });
        let out = plan(raw.clone());
        assert_eq!(out.config["raw"], raw);
        assert!(out.sealed.is_empty() && out.dropped.is_empty());
    }

    #[test]
    fn a_model_specs_key_is_the_gateways_and_is_not_stored() {
        let out = plan(json!({
            "model": { "provider": "openrouter", "model": "x/y", "api_key": "sk-or-live" },
            "fallback_providers": [ { "base_url": "https://api.z/v1", "model": "z", "api_key": "sk-z" } ],
            "delegation": { "model": "d", "base_url": "https://d/v1", "api_key": "sk-d" },
        }));
        assert_eq!(
            out.config["raw"]["model"]["api_key"],
            json!("${LLM_API_KEY}")
        );
        assert_eq!(
            out.config["raw"]["fallback_providers"][0]["api_key"],
            json!("${LLM_API_KEY}")
        );
        assert_eq!(
            out.config["raw"]["delegation"]["api_key"],
            json!("${LLM_API_KEY}")
        );
        assert_eq!(out.config["raw"]["model"]["model"], json!("x/y"));
        assert!(out.sealed.is_empty());
        assert_eq!(
            out.dropped,
            vec![
                "model.api_key",
                "fallback_providers[0].api_key",
                "delegation.api_key"
            ]
        );
        // Still a spec after the swap: the render keeps routing it.
        assert!(talaria_fleet_brain::is_model_spec(
            &out.config["raw"]["model"]
        ));
    }

    #[test]
    fn inside_a_spec_everything_but_its_key_is_rendered_and_so_sealed() {
        let out = plan(json!({
            "model": {
                "provider": "custom", "model": "m", "api_key": "sk-up",
                "default_headers": { "X-Api-Key": "hdr-secret" },
                "nested": { "model": "n", "base_url": "u", "api_key": "sk-nested" },
            },
        }));
        assert_eq!(out.dropped, vec!["model.api_key"]);
        let names: Vec<&str> = out.sealed.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "CONFIG_MODEL_DEFAULT_HEADERS_X_API_KEY",
                "CONFIG_MODEL_NESTED_API_KEY"
            ]
        );
        assert_eq!(
            out.config["raw"]["model"]["nested"]["api_key"],
            json!("${CONFIG_MODEL_NESTED_API_KEY}")
        );
    }

    #[test]
    fn authored_mcp_credentials_are_dropped_not_sealed() {
        let out = plan(json!({
            "mcp_servers": {
                "github": {
                    "url": "https://api.github.example/mcp",
                    "headers": { "Authorization": "Bearer ghp_live", "X-Agent-Name": "scout" },
                    "env": { "GITHUB_TOKEN": "ghp_env", "DEBUG": "1" },
                },
                "linear": { "url": "https://mcp.linear.app/sse", "headers": { "Authorization": "Bearer ${LINEAR}" } },
            },
        }));
        let gh = &out.config["raw"]["mcp_servers"]["github"];
        assert_eq!(gh["headers"], json!({ "X-Agent-Name": "scout" }));
        assert_eq!(gh["env"], json!({ "DEBUG": "1" }));
        assert_eq!(gh["url"], json!("https://api.github.example/mcp"));
        assert_eq!(
            out.config["raw"]["mcp_servers"]["linear"]["headers"]["Authorization"],
            json!("Bearer ${LINEAR}")
        );
        assert!(out.sealed.is_empty());
        assert_eq!(
            out.dropped,
            vec![
                "mcp_servers.github.headers.Authorization",
                "mcp_servers.github.env.GITHUB_TOKEN"
            ]
        );
    }

    #[test]
    fn a_top_level_literal_and_array_paths_get_names() {
        let out = plan(json!({
            "api_key": "sk-top",
            "custom_providers": [ { "name": "lan", "base_url": "http://lan/v1", "api_key": "sk-lan" } ],
        }));
        let names: Vec<(&str, &str)> = out
            .sealed
            .iter()
            .map(|l| (l.name.as_str(), l.path.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("CONFIG_API_KEY", "api_key"),
                (
                    "CONFIG_CUSTOM_PROVIDERS_0_API_KEY",
                    "custom_providers[0].api_key"
                ),
            ]
        );
    }

    #[test]
    fn long_and_clashing_paths_still_get_distinct_legal_names() {
        let long = "a_very_long_platform_name_that_keeps_going_and_going";
        let out = plan(json!({
            "platforms": { long: { "extra": { "webhook_signing_secret": "s1" } } },
            "providers": { "a-b": { "token": "t1" }, "a_b": { "token": "t2" } },
        }));
        let names: Vec<&str> = out.sealed.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names.len(), 3);
        for n in &names {
            assert!(crate::name_ok(n), "{n}");
            assert!(n.len() <= 64, "{n}");
            assert!(n.starts_with(CONFIG_SECRET_PREFIX), "{n}");
        }
        assert_eq!(names[1], "CONFIG_PROVIDERS_A_B_TOKEN");
        assert_ne!(
            names[1], names[2],
            "a clash is told apart by the path's hash"
        );
        assert!(names[2].starts_with("CONFIG_PROVIDERS_A_B_TOKEN_"));
        // Same input, same names: the plan is deterministic.
        let again = plan(json!({
            "platforms": { long: { "extra": { "webhook_signing_secret": "s1" } } },
            "providers": { "a-b": { "token": "t1" }, "a_b": { "token": "t2" } },
        }));
        let again: Vec<&str> = again.sealed.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, again);
    }

    #[test]
    fn a_literal_no_secret_can_carry_is_refused_by_path() {
        let err = plan_config_seal(&json!({
            "raw": { "platforms": { "x": { "private_key": "-----BEGIN-----\nabc\n-----END-----" } } }
        }))
        .unwrap_err();
        assert!(
            err.starts_with("platforms.x.private_key holds a literal credential"),
            "{err}"
        );
        assert!(err.contains("more than one line"), "{err}");
        let big = "k".repeat(8193);
        let err = plan_config_seal(&json!({ "raw": { "secret": big } })).unwrap_err();
        assert!(err.contains("8 KB"), "{err}");
    }

    #[test]
    fn a_root_that_is_a_spec_is_routed_like_one() {
        let out =
            plan(json!({ "model": "m", "provider": "p", "api_key": "sk-root", "token": "t" }));
        assert_eq!(out.config["raw"]["api_key"], json!("${LLM_API_KEY}"));
        assert_eq!(out.dropped, vec!["api_key"]);
        assert_eq!(out.config["raw"]["token"], json!("${CONFIG_TOKEN}"));
    }

    #[test]
    fn a_config_without_raw_or_literals_plans_to_itself() {
        let bare = json!({ "main": { "endpoint": "e", "model": "m" }, "aliases": [] });
        let out = plan_config_seal(&bare).unwrap();
        assert_eq!(out.config, bare);
        assert!(out.sealed.is_empty() && out.dropped.is_empty());
    }

    #[test]
    fn handles_are_found_wherever_they_sit() {
        let config = json!({
            "raw": {
                "platforms": { "telegram": { "token": "${CONFIG_PLATFORMS_TELEGRAM_TOKEN}" } },
                "list": [ "x ${CONFIG_A} y ${CONFIG_A}", "${LLM_API_KEY}", "${CONFIG_B}" ],
            }
        });
        assert_eq!(
            config_secret_handles(&config),
            vec!["CONFIG_PLATFORMS_TELEGRAM_TOKEN", "CONFIG_A", "CONFIG_B"]
        );
    }
}
