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
// Three kinds of literal are dropped instead, because Talaria supplies that
// credential itself and the literal is never the one in use; sealing it
// would hand the agent a credential it does not get today:
//
// - a model spec's `api_key`. The render points every spec at the gateway on
//   the agent's own key (talaria_fleet_brain::route_config_through_gateway),
//   so the stored value becomes that same `${LLM_API_KEY}`. The upstream key
//   belongs on the endpoint, entered on Models.
// - anything under the authored top-level `mcp_servers`. The render builds
//   that block fresh, every server through the MCP gateway, and an upstream
//   MCP credential belongs in the MCP registry.
// - the API server's own `key` (`platforms.api_server[.extra].key`). Its
//   bearer is the render's `API_SERVER_KEY`, the key Talaria calls the agent
//   with, and Hermes writes that over any `key` in the config (v0.22.4
//   `gateway/config_env.py` `_api_server`), so the literal only sat there in
//   plaintext.
//
// The top-level `secrets:` block is left alone. Hermes reads it raw, never
// expanded (`hermes_cli/env_loader.py` `_load_secrets_config`), and what it
// holds under credential-looking names are secret-manager references
// (`op://…`), not the credentials themselves.

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
    /// Where each literal no per-agent secret can carry sat, when the plan
    /// was asked to take those out ([`plan_config_seal_dropping_undeliverable`])
    /// rather than refuse. Always empty from [`plan_config_seal`].
    pub undeliverable: Vec<String>,
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
        // feishu's `encrypt_key`, wecom's `encoding_aes_key`.
        "encrypt_key",
        "aes_key",
    ];
    let k = key.to_ascii_lowercase().replace('-', "_");
    EXACT.contains(&k.as_str()) || LAST_WORD.iter().any(|w| k.ends_with(w))
}

/// Whether a string is a literal rather than a reference. A `${VAR}`
/// reference, alone or in a template such as `Bearer ${TOKEN}`, is already a
/// handle, matched by Hermes's own pattern (v0.22.4 `hermes_cli/config.py`
/// `_ENV_REF_RE`), so a stray `${` with no closing brace is still a literal.
/// An empty string holds nothing.
fn is_literal(s: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\$\{[^}]+\}").unwrap());
    !s.trim().is_empty() && !re.is_match(s)
}

/// The literal behind a credential key, or None (a reference, an empty
/// string, a non-string).
fn literal<'a>(key: &str, value: &'a Value) -> Option<&'a str> {
    let s = value.as_str()?;
    (credential_key(key) && is_literal(s)).then_some(s)
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
    // No other route takes it either: a typed per-agent secret has the same
    // two limits, so pointing at the Secrets tab would send the operator to a
    // form that refuses the same value.
    Err(format!(
        "{path} holds a literal credential that cannot be sealed ({why}): a per-agent secret is a single line of at most 8 KB, so Talaria has no way to deliver this value to the agent — remove it from the source config.yaml and federate again"
    ))
}

struct Planner {
    sealed: Vec<ConfigLiteral>,
    dropped: Vec<String>,
    undeliverable: Vec<String>,
    names: HashSet<String>,
    /// Take an undeliverable literal out instead of refusing the config.
    drop_undeliverable: bool,
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

    /// The handle that replaces the literal, or None when the literal cannot
    /// be delivered and the plan takes it out.
    fn seal(&mut self, path: &[Seg], value: &str) -> Result<Option<String>, String> {
        let at = dotted(path);
        if let Err(why) = deliverable(&at, value) {
            if self.drop_undeliverable {
                self.undeliverable.push(at);
                return Ok(None);
            }
            return Err(why);
        }
        let name = self.name_for(path);
        if !crate::name_ok(&name) {
            return Err(format!("{at}: no secret name fits this path"));
        }
        self.sealed.push(ConfigLiteral {
            name: name.clone(),
            path: at,
            value: value.to_string(),
        });
        Ok(Some(format!("${{{name}}}")))
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
                let mut gone: Vec<String> = Vec::new();
                for (key, child) in map.iter_mut() {
                    if path.is_empty() && key == "secrets" {
                        continue;
                    }
                    path.push(Seg::Key(key.clone()));
                    if let Some(lit) = literal(key, child) {
                        if spec && key == "api_key" {
                            self.dropped.push(dotted(path));
                            *child = Value::String(talaria_fleet_brain::GATEWAY_API_KEY.into());
                        } else {
                            match self.seal(path, lit)? {
                                Some(handle) => *child = Value::String(handle),
                                None => gone.push(key.clone()),
                            }
                        }
                    } else {
                        self.walk(child, path, in_spec || spec)?;
                    }
                    path.pop();
                }
                for key in gone {
                    map.remove(&key);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A key the render owns: a literal there is taken out, not sealed.
    fn drop_owned(&mut self, block: &mut Value, at: &str, key: &str) {
        let Some(map) = block.as_object_mut() else {
            return;
        };
        if map.get(key).and_then(Value::as_str).is_some_and(is_literal) {
            map.remove(key);
            self.dropped.push(format!("{at}.{key}"));
        }
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
    plan_with(config, false)
}

/// [`plan_config_seal`], except that a literal no per-agent secret can carry
/// (multi-line, or over 8 KB) is taken out of the config and listed in
/// `undeliverable`, instead of refusing the whole config. For redacting a
/// config already stored, where refusing would leave the plaintext in place:
/// the setting it held becomes unconfigured.
pub fn plan_config_seal_dropping_undeliverable(config: &Value) -> Result<ConfigSeal, String> {
    plan_with(config, true)
}

fn plan_with(config: &Value, drop_undeliverable: bool) -> Result<ConfigSeal, String> {
    let mut planner = Planner {
        sealed: Vec::new(),
        dropped: Vec::new(),
        undeliverable: Vec::new(),
        names: HashSet::new(),
        drop_undeliverable,
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
        // The API server's bearer is the render's (see the header). Hermes
        // promotes a key written directly under the platform into `extra`, so
        // both spellings are the same setting.
        if let Some(api) = raw.pointer_mut("/platforms/api_server") {
            planner.drop_owned(api, "platforms.api_server", "key");
            if let Some(extra) = api.get_mut("extra") {
                planner.drop_owned(extra, "platforms.api_server.extra", "key");
            }
        }
        // From the root, as the render walks it — a legacy config whose top
        // level is itself a model spec gets the same treatment as a nested one.
        planner.walk(raw, &mut path, false)?;
    }
    Ok(ConfigSeal {
        config: out,
        sealed: planner.sealed,
        dropped: planner.dropped,
        undeliverable: planner.undeliverable,
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

/// A `${CONFIG_…}` handle; the group is the secret's name.
fn handle_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\$\{(CONFIG_[A-Z0-9_]{1,57})\}").unwrap())
}

/// Every `${CONFIG_…}` handle a config carries, first-seen order.
pub fn config_secret_handles(config: &Value) -> Vec<String> {
    let re = handle_re();
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
    .map_err(|e| talaria_error::store_fault("[agent-secrets] secrets copy failed", e))?;
    Ok(())
}

/// A config copied to an agent that may not take the source agent's sealed
/// secrets along: every value in its `raw` that names one (`${CONFIG_…}`) is
/// taken out, and the paths where they sat come back. Leaving the handle in
/// would not keep the credential out of reach so much as replace it: Hermes
/// keeps an unresolved `${…}` as its literal text, so a password would become
/// a string every fleet reader can see. Taken out, the setting is simply not
/// configured (a basic-auth dashboard with no password does not register).
pub fn withhold_config_secrets(config: &Value) -> (Value, Vec<String>) {
    withhold_where(config, &|s| handle_re().is_match(s))
}

/// [`withhold_config_secrets`] for one sealed secret: every value in `raw`
/// that names `name` is taken out, every other handle stays. What deleting
/// that secret does to the config, so no value is left reading the handle's
/// own text.
pub fn withhold_config_secret(config: &Value, name: &str) -> (Value, Vec<String>) {
    withhold_where(config, &|s| names_secret(s, name))
}

/// [`withhold_config_secrets`] for the handles nothing backs: every value in
/// `raw` that names a sealed secret missing from `held` (the agent's
/// `CONFIG_*` names) is taken out, and every backed handle stays. Returns the
/// config, the paths taken out, and the missing names.
///
/// A handle with no secret behind it reaches Hermes as its own text, so the
/// render and a revert both leave that setting out. The render does it on
/// every pass, so a secret removed by any writer, not only the Secrets
/// route, can never leave the agent reading `${CONFIG_…}` as a value. Set
/// the secret again under that name and the next render restores the setting.
pub fn withhold_unheld_config_secrets(
    config: &Value,
    held: &[String],
) -> (Value, Vec<String>, Vec<String>) {
    let unheld = |name: &str| !held.iter().any(|h| h == name);
    let (out, paths) = withhold_where(config, &|s| {
        handle_re().captures_iter(s).any(|cap| unheld(&cap[1]))
    });
    let missing = config
        .get("raw")
        .map(config_secret_handles)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| unheld(n))
        .collect();
    (out, paths, missing)
}

/// The agent's sealed secret names (`CONFIG_*`), sorted.
pub async fn held_config_secrets(pg: &PgPool, agent_id: &str) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "select name from agent_secrets where agent_id = $1::uuid and name like 'CONFIG\\_%' \
         order by name",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
}

/// Where a config's `raw` names the sealed secret `name`, as dotted paths
/// (`platforms.telegram.token`, `list[2].token`).
pub fn config_secret_paths(config: &Value, name: &str) -> Vec<String> {
    fn walk(v: &Value, name: &str, path: &mut Vec<Seg>, out: &mut Vec<String>) {
        match v {
            Value::String(s) if names_secret(s, name) => out.push(dotted(path)),
            Value::Object(map) => {
                for (key, child) in map {
                    path.push(Seg::Key(key.clone()));
                    walk(child, name, path, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    path.push(Seg::Index(i));
                    walk(item, name, path, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(raw) = config.get("raw") {
        walk(raw, name, &mut Vec::new(), &mut out);
    }
    out
}

/// Whether a string carries `name`'s handle (alone or inside a template).
fn names_secret(s: &str, name: &str) -> bool {
    handle_re().captures_iter(s).any(|cap| &cap[1] == name)
}

fn withhold_where(config: &Value, hit: &dyn Fn(&str) -> bool) -> (Value, Vec<String>) {
    fn walk(v: &mut Value, path: &mut Vec<Seg>, out: &mut Vec<String>, hit: &dyn Fn(&str) -> bool) {
        let names = |v: &Value| v.as_str().is_some_and(hit);
        match v {
            Value::Object(map) => {
                let gone: Vec<String> = map
                    .iter()
                    .filter(|(_, child)| names(child))
                    .map(|(k, _)| k.clone())
                    .collect();
                for key in gone {
                    map.remove(&key);
                    path.push(Seg::Key(key));
                    out.push(dotted(path));
                    path.pop();
                }
                for (key, child) in map.iter_mut() {
                    path.push(Seg::Key(key.clone()));
                    walk(child, path, out, hit);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, mut item) in std::mem::take(items).into_iter().enumerate() {
                    path.push(Seg::Index(i));
                    if names(&item) {
                        out.push(dotted(path));
                    } else {
                        walk(&mut item, path, out, hit);
                        items.push(item);
                    }
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut out = config.clone();
    let mut withheld = Vec::new();
    if let Some(raw) = out.get_mut("raw") {
        walk(raw, &mut Vec::new(), &mut withheld, hit);
    }
    (out, withheld)
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
            // Platform credentials Hermes v0.22.4 reads from a platform's
            // `extra`: feishu, wecom.
            "encrypt_key",
            "encoding_aes_key",
            "verification_token",
            "corp_secret",
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
        // The Secrets tab refuses the same values, so it is not offered.
        assert!(!err.contains("Secrets tab"), "{err}");
        let big = "k".repeat(8193);
        let err = plan_config_seal(&json!({ "raw": { "secret": big } })).unwrap_err();
        assert!(err.contains("8 KB"), "{err}");
    }

    #[test]
    fn a_stray_dollar_brace_is_a_literal_not_a_reference() {
        let out = plan(json!({
            "dashboard": { "basic_auth": { "password": "pa${ss" } },
            "platforms": { "x": { "token": "${}" } },
        }));
        let names: Vec<(&str, &str)> = out
            .sealed
            .iter()
            .map(|l| (l.name.as_str(), l.value.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD", "pa${ss"),
                ("CONFIG_PLATFORMS_X_TOKEN", "${}"),
            ]
        );
    }

    #[test]
    fn the_secrets_block_is_read_raw_by_hermes_and_left_alone() {
        let raw = json!({
            "secrets": {
                "onepassword": {
                    "enabled": true,
                    "env": { "OPENAI_API_KEY": "op://vault/openai/credential" },
                },
                "bitwarden": { "env": { "GITHUB_TOKEN": "bws-secret-id" } },
            },
        });
        let out = plan(raw.clone());
        assert_eq!(out.config["raw"], raw);
        assert!(out.sealed.is_empty() && out.dropped.is_empty());
        // Only the top-level block: a `secrets` map deeper down is walked.
        let out = plan(json!({ "x": { "secrets": { "token": "t" } } }));
        assert_eq!(out.sealed[0].name, "CONFIG_X_SECRETS_TOKEN");
    }

    #[test]
    fn the_api_servers_key_is_the_renders_and_is_dropped() {
        let out = plan(json!({
            "platforms": {
                "api_server": {
                    "enabled": true,
                    "key": "sk-local",
                    "extra": { "key": "sk-extra", "port": 8642 },
                },
                "webhook": { "key": "not-a-credential-name" },
            },
        }));
        let api = &out.config["raw"]["platforms"]["api_server"];
        assert!(api.get("key").is_none(), "{api}");
        assert_eq!(api["extra"], json!({ "port": 8642 }));
        assert_eq!(api["enabled"], json!(true));
        assert_eq!(
            out.dropped,
            vec!["platforms.api_server.key", "platforms.api_server.extra.key"]
        );
        // A bare `key` elsewhere is not a credential name.
        assert_eq!(
            out.config["raw"]["platforms"]["webhook"]["key"],
            json!("not-a-credential-name")
        );
        assert!(out.sealed.is_empty());
        // A reference there is left as written.
        let out = plan(json!({
            "platforms": { "api_server": { "key": "${API_SERVER_KEY}" } },
        }));
        assert_eq!(
            out.config["raw"]["platforms"]["api_server"]["key"],
            json!("${API_SERVER_KEY}")
        );
        assert!(out.dropped.is_empty());
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
    fn withholding_takes_every_handle_out_of_raw() {
        let config = json!({
            "main": { "endpoint": "e", "model": "m" },
            "raw": {
                "platforms": { "telegram": { "enabled": true, "token": "${CONFIG_PLATFORMS_TELEGRAM_TOKEN}" } },
                "dashboard": { "basic_auth": { "username": "ops", "password": "${CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD}" } },
                "list": [ "keep", "Bearer ${CONFIG_A}", { "token": "${CONFIG_B}", "name": "n" } ],
                "model": { "api_key": "${LLM_API_KEY}" },
            },
        });
        let (out, withheld) = withhold_config_secrets(&config);
        assert_eq!(
            withheld,
            vec![
                "platforms.telegram.token",
                "dashboard.basic_auth.password",
                "list[1]",
                "list[2].token",
            ]
        );
        assert!(config_secret_handles(&out).is_empty(), "{out}");
        assert_eq!(
            out["raw"]["platforms"]["telegram"],
            json!({ "enabled": true })
        );
        assert_eq!(
            out["raw"]["dashboard"]["basic_auth"],
            json!({ "username": "ops" })
        );
        assert_eq!(out["raw"]["list"], json!([ "keep", { "name": "n" } ]));
        assert_eq!(out["raw"]["model"]["api_key"], json!("${LLM_API_KEY}"));
        assert_eq!(out["main"], config["main"]);
        // Nothing to withhold: the config comes back as it was.
        let bare = json!({ "raw": { "model": { "api_key": "${LLM_API_KEY}" } } });
        assert_eq!(
            withhold_config_secrets(&bare),
            (bare.clone(), Vec::<String>::new())
        );
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

    #[test]
    fn an_undeliverable_literal_is_taken_out_only_when_asked() {
        let config = json!({ "raw": {
            "platforms": {
                "telegram": { "token": "123:abc" },
                "matrix": { "password": "line one\nline two" },
            },
        } });
        // The federation contract: refuse, naming the path.
        let refused = plan_config_seal(&config).unwrap_err();
        assert!(refused.contains("platforms.matrix.password"), "{refused}");
        // The stored-config redaction: take it out, seal the rest.
        let out = plan_config_seal_dropping_undeliverable(&config).unwrap();
        assert_eq!(out.undeliverable, vec!["platforms.matrix.password"]);
        assert_eq!(out.config["raw"]["platforms"]["matrix"], json!({}));
        assert_eq!(
            out.config["raw"]["platforms"]["telegram"]["token"],
            json!("${CONFIG_PLATFORMS_TELEGRAM_TOKEN}")
        );
        assert_eq!(out.sealed.len(), 1);
        // A config with nothing undeliverable plans the same either way.
        let fine = json!({ "raw": { "platforms": { "telegram": { "token": "123:abc" } } } });
        let a = plan_config_seal(&fine).unwrap();
        let b = plan_config_seal_dropping_undeliverable(&fine).unwrap();
        assert_eq!(a.config, b.config);
        assert_eq!(a.sealed, b.sealed);
        assert!(a.undeliverable.is_empty() && b.undeliverable.is_empty());
    }

    #[test]
    fn one_secret_is_found_and_withheld_by_name_alone() {
        let config = json!({
            "main": { "endpoint": "e", "model": "m" },
            "raw": {
                "dashboard": { "basic_auth": { "username": "ops", "password": "${CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD}" } },
                "platforms": { "telegram": { "token": "${CONFIG_PLATFORMS_TELEGRAM_TOKEN}" } },
                "list": [ "Bearer ${CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD}", "${CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD_2}" ],
            },
        });
        let name = "CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD";
        // A longer name that starts the same is a different secret.
        assert_eq!(
            config_secret_paths(&config, name),
            vec!["dashboard.basic_auth.password", "list[0]"]
        );
        assert_eq!(
            config_secret_paths(&config, "CONFIG_PLATFORMS_TELEGRAM_TOKEN"),
            vec!["platforms.telegram.token"]
        );
        assert!(config_secret_paths(&config, "CONFIG_NOPE").is_empty());
        // `main` is not the Hermes config: only `raw` is read.
        assert!(config_secret_paths(&json!({ "main": "${CONFIG_X}" }), "CONFIG_X").is_empty());

        let (out, withheld) = withhold_config_secret(&config, name);
        assert_eq!(withheld, vec!["dashboard.basic_auth.password", "list[0]"]);
        assert_eq!(
            out["raw"]["dashboard"]["basic_auth"],
            json!({ "username": "ops" })
        );
        assert_eq!(
            out["raw"]["platforms"]["telegram"]["token"],
            json!("${CONFIG_PLATFORMS_TELEGRAM_TOKEN}"),
            "every other handle stays"
        );
        assert_eq!(
            out["raw"]["list"],
            json!(["${CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD_2}"])
        );
        assert_eq!(out["main"], config["main"]);
        assert!(config_secret_paths(&out, name).is_empty());
    }

    #[test]
    fn a_handle_nothing_backs_is_withheld_and_a_backed_one_stays() {
        let config = json!({
            "main": { "endpoint": "e", "model": "m", "note": "${CONFIG_OUTSIDE_RAW}" },
            "raw": {
                "dashboard": { "basic_auth": { "username": "ops", "password": "${CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD}" } },
                "platforms": { "telegram": { "token": "${CONFIG_PLATFORMS_TELEGRAM_TOKEN}" } },
                // One missing name in a template takes the whole value out.
                "headers": [ "Bearer ${CONFIG_PLATFORMS_TELEGRAM_TOKEN} ${CONFIG_GONE}" ],
                "model": { "api_key": "${LLM_API_KEY}" },
            },
        });
        let held = vec!["CONFIG_PLATFORMS_TELEGRAM_TOKEN".to_string()];
        let (out, paths, missing) = withhold_unheld_config_secrets(&config, &held);
        assert_eq!(paths, vec!["dashboard.basic_auth.password", "headers[0]"]);
        assert_eq!(
            missing,
            vec!["CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD", "CONFIG_GONE"],
            "only names `raw` carries; `main` is not the Hermes config"
        );
        assert_eq!(
            out["raw"]["dashboard"]["basic_auth"],
            json!({ "username": "ops" })
        );
        assert_eq!(
            out["raw"]["platforms"]["telegram"]["token"],
            json!("${CONFIG_PLATFORMS_TELEGRAM_TOKEN}"),
            "a backed handle stays"
        );
        assert_eq!(out["raw"]["headers"], json!([]));
        assert_eq!(
            out["raw"]["model"]["api_key"],
            json!("${LLM_API_KEY}"),
            "not a sealed handle"
        );
        assert_eq!(out["main"], config["main"]);
        // Everything backed: nothing changes.
        let all: Vec<String> = [
            "CONFIG_DASHBOARD_BASIC_AUTH_PASSWORD",
            "CONFIG_PLATFORMS_TELEGRAM_TOKEN",
            "CONFIG_GONE",
        ]
        .map(String::from)
        .to_vec();
        let (same, paths, missing) = withhold_unheld_config_secrets(&config, &all);
        assert_eq!(same, config);
        assert!(paths.is_empty() && missing.is_empty());
    }
}
