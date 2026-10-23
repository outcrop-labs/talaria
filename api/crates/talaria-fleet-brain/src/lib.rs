// The fleet's brain is Talaria's own gateway. Rather than pointing agents at a
// raw LLM endpoint, the whole fleet routes its tool loop through Talaria's org
// gateway (`/api/llm/v1`) — so every agent call is guarded (confab checks see
// the full tool trace), metered in one ledger, and observable in one place.
//
// Two halves: the ROUTING half (the model-set question and the config
// transform) and the PROVISIONING half (ensure_gateway_brain — the two
// fleet-wide gateway keys minted into the Talaria-owned fleet .env, the two
// key-list settings, and the LLM_BASE_URL/LLM_API_KEY lines the containers
// interpolate — plus ensure_agent_gateway_keys / set_agent_gateway_models:
// ONE key per enabled agent, `agent:<slug>`, stamped as LLM_API_KEY_<SLUG>
// and scoped to the model ids that agent's rendered config asks for).

use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashSet;

use talaria_auth::sha256_hex;
use talaria_gateway::registry::list_endpoints;
use talaria_gateway::settings::{get_setting, set_setting};
use talaria_llm_keys::mint_key;

// What a rendered agent config points its model specs at: the gateway, on
// `${LLM_API_KEY}` — which Hermes resolves from the CONTAINER's environment,
// where fleet-render sets it to that agent's own key
// (`${LLM_API_KEY_<SLUG>}`, see [`agent_llm_key_var`]). The relay leaves an
// agent key unmetered because the flow that drove the turn writes the row.
// Sandbox harnesses get the workbench key instead (workbench-harnesses).
const GATEWAY_BASE_URL: &str = "${LLM_BASE_URL}";
const GATEWAY_API_KEY: &str = "${LLM_API_KEY}";

// Imported agents carry legacy litellm-router model names. Map them to the real
// provider model ids Talaria's gateway serves. As agents are redesigned they use
// real ids directly and this bridge shrinks to nothing.
fn legacy_model(name: &str) -> Option<&'static str> {
    match name {
        // legacy litellm "glm" → OpenRouter's Z.AI GLM
        "glm" => Some("z-ai/glm-5.2"),
        _ => None,
    }
}

pub struct GatewayModels {
    /// Every model name Talaria's gateway can resolve (across all endpoints).
    pub served: HashSet<String>,
    /// Default model to fall back to for names the gateway doesn't serve yet.
    pub fallback: Option<String>,
}

/// The set of models Talaria's gateway serves, for the config transform below.
/// Never fails — an unreadable registry answers the empty set a fresh install
/// would, and a render routes everything to the fallback in that world.
pub async fn gateway_model_set(pg: &PgPool) -> GatewayModels {
    let eps = list_endpoints(pg).await.unwrap_or_default();
    let mut served = HashSet::new();
    for e in &eps {
        for m in &e.models {
            served.insert(m.clone());
        }
    }
    let local = eps
        .iter()
        .find(|e| e.class == "local" && !e.models.is_empty())
        .or_else(|| eps.iter().find(|e| !e.models.is_empty()));
    GatewayModels {
        served,
        fallback: local.and_then(|e| e.models.first().cloned()),
    }
}

/// An LLM model spec inside an agent config: has a model name and some endpoint
/// marker (base_url / provider / key_env / api_key). MCP servers (url + headers,
/// no `model`) and other blocks are left untouched.
fn is_model_spec(o: &Value) -> bool {
    let Some(obj) = o.as_object() else {
        return false;
    };
    obj.get("model").is_some_and(Value::is_string)
        && ["base_url", "provider", "key_env", "api_key"]
            .iter()
            .any(|k| obj.contains_key(*k))
}

/// Route every LLM model spec in a raw agent config through Talaria's gateway:
/// base_url/api_key point at the gateway, provider becomes openai-compatible, and
/// the model name is kept (or falls back to the default if the gateway doesn't
/// serve it yet). Un-interweaves agents from litellm/inference-router/anthropic —
/// they have exactly one upstream, and Talaria routes to the real providers.
pub fn route_config_through_gateway(
    raw: &Value,
    models: &GatewayModels,
    mut warn: impl FnMut(String),
) -> Value {
    fn walk(v: &Value, models: &GatewayModels, warn: &mut impl FnMut(String)) -> Value {
        match v {
            Value::Array(items) => {
                Value::Array(items.iter().map(|i| walk(i, models, warn)).collect())
            }
            Value::Object(map) => {
                if is_model_spec(v) {
                    let name = map["model"].as_str().unwrap_or_default().to_string();
                    let canonical = legacy_model(&name).unwrap_or(&name).to_string();
                    let model = if models.served.contains(&canonical) {
                        canonical.clone()
                    } else {
                        models.fallback.clone().unwrap_or(canonical.clone())
                    };
                    // Warn only when we couldn't route to the intended model
                    // (unserved → fallback); a known legacy remap (glm →
                    // z-ai/glm-5.2) is intentional.
                    if !models.served.contains(&canonical) {
                        warn(format!(
                            "model \"{name}\" not served by the gateway — routing to \"{model}\""
                        ));
                    }
                    let mut out = map.clone();
                    out.remove("key_env");
                    out.insert("model".into(), Value::String(model));
                    out.insert("base_url".into(), Value::String(GATEWAY_BASE_URL.into()));
                    out.insert("api_key".into(), Value::String(GATEWAY_API_KEY.into()));
                    out.insert("provider".into(), Value::String("custom".into()));
                    Value::Object(out)
                } else {
                    Value::Object(
                        map.iter()
                            .map(|(k, val)| (k.clone(), walk(val, models, warn)))
                            .collect(),
                    )
                }
            }
            leaf => leaf.clone(),
        }
    }
    walk(raw, models, &mut warn)
}

/// Every model id a routed config asks the gateway for — the spec walk
/// [`route_config_through_gateway`] makes, read back: each spec's `model`,
/// first-seen order, deduplicated. This is the set the agent's gateway key
/// is scoped to (the relay refuses anything else on that key), so it is
/// computed from the SAME shape the config.yaml is emitted from.
pub fn routed_models(routed: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Value::Object(map) => {
                if is_model_spec(v) {
                    let m = map["model"].as_str().unwrap_or_default();
                    if !m.is_empty() && !out.iter().any(|x| x == m) {
                        out.push(m.to_string());
                    }
                } else {
                    map.values().for_each(|val| walk(val, out));
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(routed, &mut out);
    out
}

// ── The provisioning half ────────────────────────────────────────────────────

const GATEWAY_KEY_NAME: &str = "fleet-gateway";
// The workbench's own gateway credential. Harness runs reach the gateway with
// this key, never the personas' — metering has to treat the two oppositely
// (see ensure_gateway_brain), and a key is the only thing the gateway can tell
// its callers apart by. Env name matches workbench-harnesses' gateway env.
const WORKBENCH_KEY_NAME: &str = "workbench-gateway";
const WORKBENCH_KEY_ENV: &str = "LLM_WORKBENCH_API_KEY";
// Where the fleet reaches Talaria's gateway. In dev the app runs on the host,
// so agents reach it via the docker host-gateway (`extra_hosts` in the
// chassis) on :5273. In a containerized stack, set TALARIA_GATEWAY_SELF_URL to
// the app's service DNS (e.g. http://talaria-ui:3000/api/llm/v1).
const DEFAULT_SELF_URL: &str = "http://host.docker.internal:5273/api/llm/v1";

pub fn self_url() -> String {
    match std::env::var("TALARIA_GATEWAY_SELF_URL") {
        Ok(v) if !v.is_empty() => v,
        _ => DEFAULT_SELF_URL.into(),
    }
}

/// A gateway URL is one that ends in the gateway path — safe for Talaria to own
/// and migrate. Any other value means the operator deliberately pointed the
/// fleet at a raw upstream, so we leave it alone.
pub fn is_gateway_url(u: &str) -> bool {
    regex::Regex::new(r"/api/llm/v1/?$")
        .expect("gateway url pattern")
        .is_match(u.trim())
}

/// Read a KEY=value line from env-file text. The captured value is trimmed.
pub fn read_env_line(content: &str, key: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r"(?m)^{}=(.*)$", regex::escape(key)))
        .expect("env line pattern");
    re.captures(content).map(|c| c[1].trim().to_string())
}

/// The fleet's LLM endpoint override — only when the operator deliberately
/// pointed it at a raw upstream. Absent, EMPTY (the fresh-install seeded
/// placeholder: the chassis's `${LLM_BASE_URL:-…}` treats empty as unset, and
/// so does this), and gateway-shaped values are all "not an override" —
/// managed provisioning owns them. An empty value read as an override is the
/// brain-dead-fresh-install bug: the early return skips provisioning and every
/// agent keeps the chassis's dev default.
fn raw_upstream_override(content: &str) -> Option<String> {
    read_env_line(content, "LLM_BASE_URL").filter(|u| !u.is_empty() && !is_gateway_url(u))
}

/// Set or replace a KEY=value line; append if absent. The append shape is
/// fixed, including the leading newline when the file is empty.
pub fn upsert_env_line(content: &str, key: &str, value: &str) -> String {
    let re =
        regex::Regex::new(&format!(r"(?m)^{}=.*$", regex::escape(key))).expect("env line pattern");
    let line = format!("{key}={value}");
    if re.is_match(content) {
        return re.replace(content, line.as_str()).into_owned();
    }
    format!("{}\n{line}\n", content.trim_end_matches('\n'))
}

struct EnsuredKey {
    secret: String,
    rotated: bool,
}

async fn ensure_gateway_key(
    pg: &PgPool,
    name: &str,
    current_key: Option<&str>,
) -> Result<Option<EnsuredKey>, String> {
    if let Some(cur) = current_key.filter(|k| k.starts_with("tlk_")) {
        let live: Option<(i32,)> = sqlx::query_as(
            "select 1 from llm_api_keys where name = $1 and revoked_at is null and key_hash = $2",
        )
        .bind(name)
        .bind(sha256_hex(cur))
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("gateway key live-check failed: {e}"))?;
        if live.is_some() {
            return Ok(Some(EnsuredKey {
                secret: cur.to_string(),
                rotated: false,
            }));
        }
    }
    let owner: Option<(String,)> = sqlx::query_as(
        "select id::text from users order by (role = 'admin') desc, created_at asc limit 1",
    )
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("gateway key owner lookup failed: {e}"))?;
    let Some((owner_id,)) = owner else {
        return Ok(None);
    };
    sqlx::query(
        "update llm_api_keys set revoked_at = now() where name = $1 and revoked_at is null",
    )
    .bind(name)
    .execute(pg)
    .await
    .map_err(|e| format!("stale gateway key revoke failed: {e}"))?;
    // Rotation revokes the key the containers hold — it must stop
    // authenticating before the replacement is even minted.
    talaria_auth::reset_identity_cache();
    let (_, secret) = mint_key(pg, &owner_id, name)
        .await
        .map_err(|e| format!("gateway key mint failed: {e}"))?;
    Ok(Some(EnsuredKey {
        secret,
        rotated: true,
    }))
}

async fn default_gateway_model(pg: &PgPool) -> Option<String> {
    gateway_model_set(pg).await.fallback
}

#[derive(Debug, Clone)]
pub struct GatewayBrain {
    pub url: Option<String>,
    /// The gateway's default model — None while no endpoint serves one, which
    /// the render warns about. Read for that question only: it is NOT
    /// provisioned into the fleet .env as `LLM_MODEL` any more, because
    /// Hermes deliberately ignores that variable (it takes its model from
    /// config.yaml alone) and its own config migration clears it.
    pub model: Option<String>,
    pub key_rotated: bool,
    /// False when the operator pointed the fleet at a non-gateway upstream.
    pub managed: bool,
}

/// Provision (or refresh) the fleet's gateway brain in the Talaria-owned .env.
/// Idempotent; a failure is best-effort — never blocks a fleet render (the
/// render loop records a warning and moves on).
pub async fn ensure_gateway_brain(pg: &PgPool) -> Result<GatewayBrain, String> {
    let env_path = talaria_fleet_layout::fleet_env();
    if let Some(parent) = env_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("fleet dir unwritable ({}): {e}", parent.display()))?;
    }
    let content = tokio::fs::read_to_string(&env_path)
        .await
        .unwrap_or_default();

    // Two settings, two different questions — one list answering both is what
    // corrupted the ledger, so keep them apart:
    //
    //   gateway_unmetered_keys — keys the gateway must NOT write a usage row
    //     for, because their spend already reaches the ledger from another
    //     writer. ONLY the personas' key qualifies: a chat/channel/ticket turn
    //     writes one row from the persona gateway's reported usage for the
    //     whole turn, so metering the inner-loop calls behind it would count
    //     every turn twice. `workbench-gateway` is deliberately NOT here —
    //     nothing else records a harness run, so the gateway is where that
    //     spend lands.
    //
    //   gateway_agent_loop_keys — keys whose replies must never be rewritten
    //     with a guard caveat, because the caller is an agent's own tool loop
    //     and a caveat would contaminate its context. BOTH gateway keys
    //     qualify (findings still record either way).
    //
    // A stored setting that isn't a string array fails the whole brain step —
    // the render carries on without it, rather than silently overwriting a
    // corrupt value.
    let unmetered: Vec<String> = serde_json::from_value(
        get_setting(pg, "gateway_unmetered_keys", json!([GATEWAY_KEY_NAME])).await,
    )
    .map_err(|e| format!("gateway_unmetered_keys is unreadable: {e}"))?;
    if !unmetered.iter().any(|k| k == GATEWAY_KEY_NAME) {
        let mut next = unmetered;
        next.push(GATEWAY_KEY_NAME.into());
        set_setting(pg, "gateway_unmetered_keys", &json!(next))
            .await
            .map_err(|e| format!("gateway_unmetered_keys write failed: {e}"))?;
    }
    let loop_keys: Vec<String> = serde_json::from_value(
        get_setting(
            pg,
            "gateway_agent_loop_keys",
            json!([GATEWAY_KEY_NAME, WORKBENCH_KEY_NAME]),
        )
        .await,
    )
    .map_err(|e| format!("gateway_agent_loop_keys is unreadable: {e}"))?;
    let missing = [GATEWAY_KEY_NAME, WORKBENCH_KEY_NAME]
        .into_iter()
        .filter(|n| !loop_keys.contains(&n.to_string()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let mut next = loop_keys;
        next.extend(missing.iter().map(|n| n.to_string()));
        set_setting(pg, "gateway_agent_loop_keys", &json!(next))
            .await
            .map_err(|e| format!("gateway_agent_loop_keys write failed: {e}"))?;
    }

    if let Some(cur) = raw_upstream_override(&content) {
        // Operator override: a raw upstream. Respect it — no gateway brain.
        return Ok(GatewayBrain {
            url: Some(cur),
            model: None,
            key_rotated: false,
            managed: false,
        });
    }

    let url = self_url();
    let key = ensure_gateway_key(
        pg,
        GATEWAY_KEY_NAME,
        read_env_line(&content, "LLM_API_KEY").as_deref(),
    )
    .await?;
    let wb_key = ensure_gateway_key(
        pg,
        WORKBENCH_KEY_NAME,
        read_env_line(&content, WORKBENCH_KEY_ENV).as_deref(),
    )
    .await?;

    let mut next = content.clone();
    if !next.contains("# talaria-managed (gateway brain)") {
        next = format!(
            "{}\n\n# talaria-managed (gateway brain) — the fleet's default LLM is Talaria's org\n\
             # gateway, so every agent call is guarded, metered, and observable. Configure\n\
             # the real upstream model in-app on /models; don't hand-edit these lines.\n\
             # LLM_API_KEY is the fleet-wide persona key (no container receives it any\n\
             # more — each agent gets its own LLM_API_KEY_<SLUG> below);\n\
             # LLM_WORKBENCH_API_KEY is the sandbox harnesses' — separate credentials so\n\
             # the ledger can tell them apart.\n",
            next.trim_end_matches('\n')
        );
    }
    next = upsert_env_line(&next, "LLM_BASE_URL", &url);
    if let Some(k) = &key {
        next = upsert_env_line(&next, "LLM_API_KEY", &k.secret);
    }
    if let Some(k) = &wb_key {
        next = upsert_env_line(&next, WORKBENCH_KEY_ENV, &k.secret);
    }

    let model = default_gateway_model(pg).await;

    if next != content {
        // The file holds the fleet's gateway secrets: written private from
        // the first byte, and never truncated in place — a process killed
        // mid-write must leave the old file, not an empty one.
        write_atomic(&env_path, next.as_bytes(), 0o600)
            .await
            .map_err(|e| format!("fleet .env unwritable ({}): {e}", env_path.display()))?;
    }
    Ok(GatewayBrain {
        url: Some(url),
        model,
        key_rotated: key.as_ref().is_some_and(|k| k.rotated)
            || wb_key.as_ref().is_some_and(|k| k.rotated),
        managed: true,
    })
}

// ── Per-agent gateway keys ───────────────────────────────────────────────────
//
// One `tlk_` key per enabled agent, named `agent:<slug>`, stamped into the
// fleet .env as LLM_API_KEY_<SLUG> and interpolated by fleet-render into that
// agent's service as its LLM_API_KEY. The relay (llm_chat.rs) reads the key
// name back as the agent: its budget subject is the agent's model id, its
// model set is what the render scoped it to (gateway_agent_models), and its
// spend stays unmetered at the relay because the chat flow ledgers the turn.
//
// Lifecycle mirrors fleet-render's TALARIA_AGENT_KEY_<SLUG>: the DB is the
// truth and the line is rewritten every render (a line the file lost comes
// back identical, never rotating a running container out of its key); a
// slug that exists nowhere in agent_defs loses both its line and its key
// (`strip_stale_agent_lines`' rule); a RETIRED agent keeps both, because
// unretire comes back to them.

const AGENT_LLM_KEY_PREFIX: &str = "LLM_API_KEY_";

/// Compose-interpolation name for an agent's own gateway key — the same
/// upper-casing rule as `TALARIA_AGENT_KEY_<SLUG>`.
pub fn agent_llm_key_var(slug: &str) -> String {
    format!("{AGENT_LLM_KEY_PREFIX}{}", slug.to_uppercase())
}

/// Drop every `LLM_API_KEY_<SLUG>=` line whose slug (lowercased) is not in
/// `live`. The fleet-wide `LLM_API_KEY=` has no suffix and is never touched.
fn strip_stale_agent_llm_lines(content: &str, live: &HashSet<String>) -> String {
    let re = regex::Regex::new(r"^LLM_API_KEY_([A-Z0-9_]+)=").expect("per-agent llm key pattern");
    let mut out = String::with_capacity(content.len());
    for line in content.split_inclusive('\n') {
        let stale = re
            .captures(line.trim_end_matches('\n'))
            .is_some_and(|c| !live.contains(&c[1].to_lowercase()));
        if stale {
            tracing::info!(
                "[fleet] dropping a gateway key line for a slug that no longer exists: {}",
                line.split('=').next().unwrap_or("").trim()
            );
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Mint (or keep) one gateway key per enabled agent slug and stamp each
/// into the fleet .env as LLM_API_KEY_<SLUG>; sweep the lines AND the keys
/// of slugs that no longer exist. Call only on a MANAGED brain — with a raw
/// upstream override the containers' LLM_API_KEY is the operator's and no
/// per-agent key may shadow it. Returns true when any key rotated (the
/// containers holding the old one need a roll).
pub async fn ensure_agent_gateway_keys(pg: &PgPool, slugs: &[String]) -> Result<bool, String> {
    use talaria_gateway::agent_keys::{AGENT_KEY_PREFIX, agent_key_name};
    let env_path = talaria_fleet_layout::fleet_env();
    let content = tokio::fs::read_to_string(&env_path)
        .await
        .unwrap_or_default();

    let known: Vec<(String,)> = sqlx::query_as("select slug from agent_defs")
        .fetch_all(pg)
        .await
        .map_err(|e| format!("agent slugs unreadable: {e}"))?;
    let known: HashSet<String> = known.into_iter().map(|(s,)| s).collect();

    // Keys for slugs that exist nowhere: revoked, so a line that escaped the
    // file (a backup, a copied .env) stops authenticating with the def.
    let live_keys: Vec<(String,)> =
        sqlx::query_as("select name from llm_api_keys where name like $1 and revoked_at is null")
            .bind(format!("{AGENT_KEY_PREFIX}%"))
            .fetch_all(pg)
            .await
            .map_err(|e| format!("agent gateway keys unreadable: {e}"))?;
    let mut swept = false;
    for (name,) in live_keys {
        let Some(slug) = talaria_gateway::agent_keys::agent_slug_of_key(&name) else {
            continue;
        };
        if known.contains(slug) {
            continue;
        }
        sqlx::query(
            "update llm_api_keys set revoked_at = now() where name = $1 and revoked_at is null",
        )
        .bind(&name)
        .execute(pg)
        .await
        .map_err(|e| format!("stale agent gateway key revoke failed: {e}"))?;
        swept = true;
    }
    if swept {
        talaria_auth::reset_identity_cache();
    }

    let mut next = strip_stale_agent_llm_lines(&content, &known);
    let mut rotated = false;
    let mut append: Vec<String> = Vec::new();
    for slug in slugs {
        let var = agent_llm_key_var(slug);
        let Some(key) = ensure_gateway_key(
            pg,
            &agent_key_name(slug),
            read_env_line(&next, &var).as_deref(),
        )
        .await?
        else {
            // No user to own a key yet (a fresh install before first login):
            // nothing to stamp; the next render tries again.
            continue;
        };
        rotated |= key.rotated;
        if read_env_line(&next, &var).is_some() {
            next = upsert_env_line(&next, &var, &key.secret);
        } else {
            append.push(format!("{var}={}", key.secret));
        }
    }
    if !append.is_empty() {
        next = format!(
            "{}\n# per-agent gateway keys — talaria-managed\n{}\n",
            next.trim_end_matches('\n'),
            append.join("\n")
        );
    }
    if next != content {
        if let Some(parent) = env_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("fleet dir unwritable ({}): {e}", parent.display()))?;
        }
        write_atomic(&env_path, next.as_bytes(), 0o600)
            .await
            .map_err(|e| format!("fleet .env unwritable ({}): {e}", env_path.display()))?;
    }
    Ok(rotated)
}

/// What one agent's gateway key is scoped to, as the render computed it.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentGatewayScope {
    pub slug: String,
    /// The agent's model id — the ledger namespace and the budget subject.
    pub model: String,
    /// The model ids the rendered config asks the gateway for.
    pub models: Vec<String>,
}

/// The settings row the relay judges agent keys by, as one JSON object.
pub fn agent_gateway_models_json(scopes: &[AgentGatewayScope]) -> Value {
    let mut out = serde_json::Map::new();
    for s in scopes {
        out.insert(
            s.slug.clone(),
            json!({ "model": s.model, "models": s.models }),
        );
    }
    Value::Object(out)
}

/// Write every enabled agent's scope to `gateway_agent_models` — the whole
/// row at once, so a slug that left the render leaves the row. Unchanged
/// content is not rewritten (the write would drop the relay's hot window
/// for nothing).
pub async fn set_agent_gateway_models(
    pg: &PgPool,
    scopes: &[AgentGatewayScope],
) -> Result<(), String> {
    use talaria_gateway::agent_keys::AGENT_MODELS_SETTING;
    let next = agent_gateway_models_json(scopes);
    let current = get_setting(pg, AGENT_MODELS_SETTING, Value::Null).await;
    if current == next {
        return Ok(());
    }
    set_setting(pg, AGENT_MODELS_SETTING, &next)
        .await
        .map_err(|e| format!("{AGENT_MODELS_SETTING} write failed: {e}"))
}

/// The row the render writes BEFORE it lands any config.yaml: each scoped
/// agent may ask for what its stored scope allows AND what its new one
/// does (the new ids first), and a slug the render dropped keeps its stored
/// entry until [`set_agent_gateway_models`] narrows the row afterwards.
/// Pure over the stored row so the merge is pinned without a database.
pub fn widened_agent_gateway_models_json(current: &Value, scopes: &[AgentGatewayScope]) -> Value {
    let mut out = current.as_object().cloned().unwrap_or_default();
    for s in scopes {
        let mut models = s.models.clone();
        let stored = out
            .get(&s.slug)
            .and_then(|e| e.get("models"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        for m in stored {
            if !models.iter().any(|x| x == m) {
                models.push(m.to_string());
            }
        }
        out.insert(
            s.slug.clone(),
            json!({ "model": s.model, "models": models }),
        );
    }
    Value::Object(out)
}

/// Write [`widened_agent_gateway_models_json`] — the first half of the
/// render's widen-then-narrow (see the scope write in render_fleet).
/// Unchanged content is not rewritten.
pub async fn widen_agent_gateway_models(
    pg: &PgPool,
    scopes: &[AgentGatewayScope],
) -> Result<(), String> {
    use talaria_gateway::agent_keys::AGENT_MODELS_SETTING;
    let current = get_setting(pg, AGENT_MODELS_SETTING, Value::Null).await;
    let next = widened_agent_gateway_models_json(&current, scopes);
    if current == next {
        return Ok(());
    }
    set_setting(pg, AGENT_MODELS_SETTING, &next)
        .await
        .map_err(|e| format!("{AGENT_MODELS_SETTING} write failed: {e}"))
}

/// Write a file ATOMICALLY and with its mode set before any byte lands:
/// the content goes to a sibling temp file opened with `mode`, is flushed,
/// and is renamed over the target. A reader (docker compose, the chat
/// proxy's manifest read) sees the old file or the new one, never a
/// truncated middle, and a process killed between open and write leaves
/// the old file in place rather than an empty one — the two things a plain
/// `tokio::fs::write` cannot promise. Lives here because every fleet file
/// writer can reach this crate (fleet-render depends on it, not the other
/// way round).
pub async fn write_atomic(path: &std::path::Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;
    let dir = path
        .parent()
        .ok_or_else(|| format!("{}: no parent directory", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: not a file path", path.display()))?;
    // Unique per process AND per call: two renders racing on the same file
    // must not share a temp name, or one's rename carries the other's bytes.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    let result: Result<(), String> = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)
            .await
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        file.write_all(bytes)
            .await
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        file.sync_all()
            .await
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        drop(file);
        // create_new's mode is masked by the umask; the target's mode is a
        // promise, so set it explicitly before the rename exposes the file.
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
            .await
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|e| format!("{} -> {}: {e}", tmp.display(), path.display()))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

/// Rewrite a file IN PLACE — same inode — for a file a container holds
/// through a single-file bind mount (an agent's config.yaml and SOUL.md).
/// Such a bind pins the inode that existed when the container was created,
/// so [`write_atomic`]'s rename lands on the host and never reaches the
/// container: it keeps reading the orphaned old file until it is recreated.
///
/// What a racing reader can see, and why each is safe for Hermes:
/// - **Never an empty file.** Nothing truncates first: the new bytes go over
///   the old ones at offset 0, and only then is a shorter file cut to length.
/// - **The new bytes followed by the old tail**, for the instant between that
///   write and the cut. The caller makes that state unparseable (the render
///   ends config.yaml with a YAML `...` document end), and Hermes keeps
///   serving its last good config through a parse error.
/// - **A stale read cached under a signature that never changes again.**
///   Hermes caches config.yaml on `(mtime, size, inode, ctime)`; a same-size
///   rewrite inside one coarse mtime tick could leave a mid-write read cached
///   for good. The mtime is set explicitly once every byte has landed, so the
///   final signature always differs from anything a reader saw before.
///
/// Writers are serialized by an exclusive `flock` on the file itself (two
/// renders racing in place would interleave; Hermes never locks it), and a
/// write whose bytes are already there is skipped — no signature change, no
/// reload, for every agent a render did not touch. Answers whether it wrote.
/// A missing file has no container holding it yet, so it is created
/// atomically.
pub async fn write_in_place(
    path: &std::path::Path,
    bytes: &[u8],
    mode: u32,
) -> Result<bool, String> {
    let owned = path.to_path_buf();
    let data = bytes.to_vec();
    let wrote = tokio::task::spawn_blocking(move || rewrite_in_place(&owned, &data, mode))
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match wrote {
        InPlace::Missing => write_atomic(path, bytes, mode).await.map(|()| true),
        InPlace::Unchanged => Ok(false),
        InPlace::Written => Ok(true),
        InPlace::Failed(e) => Err(format!("{}: {e}", path.display())),
    }
}

enum InPlace {
    Missing,
    Unchanged,
    Written,
    Failed(std::io::Error),
}

fn rewrite_in_place(path: &std::path::Path, bytes: &[u8], mode: u32) -> InPlace {
    use std::io::Read;
    use std::os::unix::fs::{FileExt, PermissionsExt};
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return InPlace::Missing,
        Err(e) => return InPlace::Failed(e),
    };
    let step = || -> std::io::Result<bool> {
        // Released when `file` drops, on every path out of here.
        file.lock()?;
        let mut current = Vec::new();
        (&file).read_to_end(&mut current)?;
        let perms = std::fs::Permissions::from_mode(mode);
        if current == bytes {
            if file.metadata()?.permissions().mode() & 0o7777 != mode {
                file.set_permissions(perms)?;
            }
            return Ok(false);
        }
        file.write_all_at(bytes, 0)?;
        if bytes.len() < current.len() {
            file.set_len(bytes.len() as u64)?;
        }
        file.sync_data()?;
        file.set_permissions(perms)?;
        file.set_modified(std::time::SystemTime::now())?;
        Ok(true)
    };
    match step() {
        Ok(true) => InPlace::Written,
        Ok(false) => InPlace::Unchanged,
        Err(e) => InPlace::Failed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn write_in_place_keeps_the_inode_skips_the_unchanged_and_shrinks_clean() {
        use std::os::unix::fs::MetadataExt;
        let dir = std::env::temp_dir().join(format!(
            "talaria-write-in-place-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("config.yaml");

        // Missing: created (atomically), and that counts as a write.
        assert!(
            write_in_place(&target, b"a: 1\nb: 22222\n...\n", 0o644)
                .await
                .unwrap()
        );
        let ino = tokio::fs::metadata(&target).await.unwrap().ino();

        // Same bytes: nothing written, signature untouched.
        let before = tokio::fs::metadata(&target).await.unwrap();
        assert!(
            !write_in_place(&target, b"a: 1\nb: 22222\n...\n", 0o644)
                .await
                .unwrap()
        );
        let after = tokio::fs::metadata(&target).await.unwrap();
        assert_eq!(before.mtime_nsec(), after.mtime_nsec());
        assert_eq!(before.ctime_nsec(), after.ctime_nsec());

        // Shorter: same inode (what a single-file bind still sees), the
        // whole new content and nothing of the old tail.
        assert!(
            write_in_place(&target, b"a: 2\n...\n", 0o644)
                .await
                .unwrap()
        );
        let meta = tokio::fs::metadata(&target).await.unwrap();
        assert_eq!(meta.ino(), ino, "rewritten in place, not renamed over");
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"a: 2\n...\n");

        // Longer: same inode again.
        assert!(
            write_in_place(&target, b"a: 3\nc: [1, 2, 3]\n...\n", 0o644)
                .await
                .unwrap()
        );
        assert_eq!(tokio::fs::metadata(&target).await.unwrap().ino(), ino);
        assert_eq!(
            tokio::fs::read(&target).await.unwrap(),
            b"a: 3\nc: [1, 2, 3]\n...\n"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn write_atomic_lands_the_bytes_private_and_leaves_no_temp_behind() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "talaria-write-atomic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("fleet.env");
        tokio::fs::write(&target, "OLD=1\n").await.unwrap();
        write_atomic(&target, b"NEW=2\n", 0o600).await.unwrap();
        assert_eq!(tokio::fs::read_to_string(&target).await.unwrap(), "NEW=2\n");
        let mode = tokio::fs::metadata(&target)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the target carries the requested mode");
        let mut entries = tokio::fs::read_dir(&dir).await.unwrap();
        let mut names = Vec::new();
        while let Some(e) = entries.next_entry().await.unwrap() {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(
            names,
            vec!["fleet.env".to_string()],
            "no temp file survives"
        );
        // A missing parent is an error, not a panic, and nothing is created.
        let orphan = dir.join("missing").join("x");
        assert!(write_atomic(&orphan, b"x", 0o644).await.is_err());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    fn models(served: &[&str], fallback: Option<&str>) -> GatewayModels {
        GatewayModels {
            served: served.iter().map(|s| s.to_string()).collect(),
            fallback: fallback.map(|f| f.to_string()),
        }
    }

    #[test]
    fn a_model_spec_is_a_model_plus_an_endpoint_marker() {
        assert!(is_model_spec(&json!({"model": "m", "base_url": "x"})));
        assert!(is_model_spec(&json!({"model": "m", "provider": "p"})));
        assert!(is_model_spec(&json!({"model": "m", "key_env": "K"})));
        assert!(is_model_spec(&json!({"model": "m", "api_key": "k"})));
        // MCP servers, bare names, arrays, scalars: untouched shapes.
        assert!(!is_model_spec(&json!({"url": "u", "headers": {}})));
        assert!(!is_model_spec(&json!({"model": "m"})));
        assert!(!is_model_spec(&json!(["model", "m"])));
        assert!(!is_model_spec(&json!("model")));
    }

    #[test]
    fn specs_route_to_the_gateway_and_drop_key_env() {
        let raw = json!({
            "specs": {
                "main": {"model": "qwen3:14b", "provider": "ollama", "key_env": "OLD_KEY"},
                "mcp_servers": {"talaria": {"url": "http://x/talaria", "headers": {"h": "v"}}}
            },
            "nested": [{"model": "qwen3:14b", "base_url": "http://litellm:4000"}]
        });
        let mut warned = Vec::new();
        let out =
            route_config_through_gateway(&raw, &models(&["qwen3:14b"], None), |m| warned.push(m));
        let main = &out["specs"]["main"];
        assert_eq!(main["model"], json!("qwen3:14b"));
        assert_eq!(main["base_url"], json!("${LLM_BASE_URL}"));
        assert_eq!(main["api_key"], json!("${LLM_API_KEY}"));
        assert_eq!(main["provider"], json!("custom"));
        assert!(
            main.get("key_env").is_none(),
            "the raw upstream's env name must not survive"
        );
        assert_eq!(out["nested"][0]["base_url"], json!("${LLM_BASE_URL}"));
        // MCP servers ride through untouched
        assert_eq!(
            out["specs"]["mcp_servers"]["talaria"]["url"],
            json!("http://x/talaria")
        );
        assert!(warned.is_empty(), "served models do not warn");
    }

    #[test]
    fn routed_models_reads_back_every_spec_once_in_first_seen_order() {
        let raw = json!({
            "specs": {
                "main": {"model": "qwen3:14b", "provider": "ollama"},
                "fast": {"model": "glm", "base_url": "x"},
                "mcp_servers": {"talaria": {"url": "http://x/talaria", "headers": {"h": "v"}}}
            },
            "nested": [{"model": "qwen3:14b", "base_url": "http://litellm:4000"}],
            "bare": {"model": "not-a-spec"}
        });
        let routed = route_config_through_gateway(
            &raw,
            &models(&["qwen3:14b", "z-ai/glm-5.2"], None),
            |_| {},
        );
        assert_eq!(
            routed_models(&routed),
            vec!["qwen3:14b".to_string(), "z-ai/glm-5.2".to_string()],
            "the ROUTED ids — after the legacy remap — deduplicated; a bare `model` key is not a spec"
        );
        assert!(routed_models(&json!({"mcp_servers": {}})).is_empty());
        assert!(routed_models(&json!("x")).is_empty());
    }

    #[test]
    fn agent_llm_key_lines_follow_the_agent_key_rule() {
        assert_eq!(agent_llm_key_var("analyst"), "LLM_API_KEY_ANALYST");
        assert_eq!(agent_llm_key_var("ops2"), "LLM_API_KEY_OPS2");
        let live: HashSet<String> = ["analyst".to_string(), "ops2".to_string()]
            .into_iter()
            .collect();
        let content = "LLM_API_KEY=tlk_fleet\n\
                       # per-agent gateway keys — talaria-managed\n\
                       LLM_API_KEY_ANALYST=tlk_1\n\
                       LLM_API_KEY_OLDNAME=tlk_2\n\
                       LLM_API_KEY_OPS2=tlk_3\n\
                       TALARIA_AGENT_KEY_OLDNAME=tak_2\n\
                       LLM_WORKBENCH_API_KEY=tlk_wb\n";
        let out = strip_stale_agent_llm_lines(content, &live);
        assert_eq!(
            out,
            "LLM_API_KEY=tlk_fleet\n\
             # per-agent gateway keys — talaria-managed\n\
             LLM_API_KEY_ANALYST=tlk_1\n\
             LLM_API_KEY_OPS2=tlk_3\n\
             TALARIA_AGENT_KEY_OLDNAME=tak_2\n\
             LLM_WORKBENCH_API_KEY=tlk_wb\n",
            "only LLM_API_KEY_<SLUG> lines for unknown slugs go; the fleet-wide key, the workbench key and fleet-render's lines stay"
        );
        assert_eq!(strip_stale_agent_llm_lines(&out, &live), out, "idempotent");
        let none = HashSet::new();
        assert_eq!(
            strip_stale_agent_llm_lines("A=1\nLLM_API_KEY_X=2\nB=3", &none),
            "A=1\nB=3",
            "a last line without a newline is kept as written"
        );
    }

    #[test]
    fn the_scope_row_is_one_object_keyed_by_slug() {
        let scopes = vec![
            AgentGatewayScope {
                slug: "analyst".into(),
                model: "analyst".into(),
                models: vec!["qwen3:14b".into()],
            },
            AgentGatewayScope {
                slug: "xeng".into(),
                model: "x-eng".into(),
                models: vec![],
            },
        ];
        assert_eq!(
            agent_gateway_models_json(&scopes),
            json!({
                "analyst": { "model": "analyst", "models": ["qwen3:14b"] },
                "xeng": { "model": "x-eng", "models": [] }
            })
        );
        // What the relay reads back is exactly what the render wrote.
        let row = agent_gateway_models_json(&scopes);
        let s = talaria_gateway::agent_keys::scope_of(&row, "xeng").unwrap();
        assert_eq!(s.model, "x-eng");
        assert!(s.models.is_empty());
        assert_eq!(agent_gateway_models_json(&[]), json!({}));
    }

    #[test]
    fn the_widened_row_allows_both_scopes_until_the_narrow_write() {
        // A model switch on `analyst`, a new agent, and a slug this render
        // dropped: while the configs land, every id either render routes to
        // is allowed, and nothing stored is lost yet.
        let stored = json!({
            "analyst": { "model": "analyst", "models": ["old-main", "shared-aux"] },
            "gone": { "model": "gone", "models": ["m"] }
        });
        let scopes = vec![
            AgentGatewayScope {
                slug: "analyst".into(),
                model: "analyst".into(),
                models: vec!["new-main".into(), "shared-aux".into()],
            },
            AgentGatewayScope {
                slug: "fresh".into(),
                model: "fresh".into(),
                models: vec!["n".into()],
            },
        ];
        let wide = widened_agent_gateway_models_json(&stored, &scopes);
        assert_eq!(
            wide,
            json!({
                "analyst": { "model": "analyst", "models": ["new-main", "shared-aux", "old-main"] },
                "gone": { "model": "gone", "models": ["m"] },
                "fresh": { "model": "fresh", "models": ["n"] }
            })
        );
        let s = talaria_gateway::agent_keys::scope_of(&wide, "analyst").unwrap();
        for id in ["old-main", "new-main", "shared-aux"] {
            assert!(talaria_gateway::agent_keys::model_in_scope(&s, id), "{id}");
        }
        // Widening an already-current row changes nothing (no rewrite), and
        // a missing or malformed row widens to exactly the new scope.
        let narrow = agent_gateway_models_json(&scopes);
        assert_eq!(widened_agent_gateway_models_json(&narrow, &scopes), narrow);
        assert_eq!(
            widened_agent_gateway_models_json(&Value::Null, &scopes),
            narrow
        );
    }

    #[test]
    fn a_spec_object_is_terminal_its_own_children_never_walk() {
        // The walk returns at the spec branch — a spec carries no nested
        // specs, so anything nested inside one rides through untouched. This
        // pins that branch semantics, not an accident of the walk order.
        let raw = json!({
            "model": "qwen3:14b",
            "provider": "ollama",
            "weird": {"model": "gpt:old", "base_url": "http://litellm:4000"}
        });
        let out = route_config_through_gateway(&raw, &models(&["qwen3:14b"], None), |_| {});
        assert_eq!(
            out["weird"]["base_url"],
            json!("http://litellm:4000"),
            "inside a spec, nothing re-routes"
        );
    }

    #[test]
    fn unserved_names_fall_back_and_legacy_names_remap_without_warning() {
        let raw = json!({"main": {"model": "glm", "base_url": "x"}, "deep": {"model": "gpt:old", "base_url": "x"}});
        let mut warned = Vec::new();
        let out = route_config_through_gateway(
            &raw,
            &models(&["z-ai/glm-5.2"], Some("qwen3:14b")),
            |m| warned.push(m),
        );
        // glm is a known remap landing on a SERVED id — intentional, no warning.
        assert_eq!(out["main"]["model"], json!("z-ai/glm-5.2"));
        // gpt:old is served by nothing — falls back, and says so.
        assert_eq!(out["deep"]["model"], json!("qwen3:14b"));
        assert_eq!(
            warned,
            vec![
                "model \"gpt:old\" not served by the gateway — routing to \"qwen3:14b\""
                    .to_string()
            ]
        );
    }

    #[test]
    fn with_no_fallback_an_unserved_name_keeps_itself() {
        let raw = json!({"model": "gpt:old", "base_url": "x"});
        let out = route_config_through_gateway(&raw, &models(&[], None), |_| {});
        assert_eq!(out["model"], json!("gpt:old"), "fallback ?? canonical");
    }

    #[test]
    fn gateway_urls_are_the_path_with_or_without_the_slash() {
        assert!(is_gateway_url(
            "http://host.docker.internal:5273/api/llm/v1"
        ));
        assert!(is_gateway_url(
            "http://host.docker.internal:5273/api/llm/v1/"
        ));
        assert!(is_gateway_url("  http://talaria-ui:3000/api/llm/v1 \n"));
        // A raw upstream the operator chose: not ours to migrate.
        assert!(!is_gateway_url("https://openrouter.ai/api/v1"));
        assert!(!is_gateway_url("http://litellm:4000/v1"));
    }

    #[test]
    fn an_empty_seeded_llm_base_url_is_not_an_override() {
        // The fresh-install shape: `talaria setup` seeds placeholder lines
        // with empty values, and the override check must read an empty value
        // as "not set" — the same call the chassis's `${LLM_BASE_URL:-…}`
        // makes — so managed provisioning runs instead of silently keeping
        // the chassis's dev default (the brain-dead-fresh-install bug).
        assert_eq!(
            raw_upstream_override("LLM_BASE_URL=\nLLM_API_KEY=\nLLM_MODEL=\n"),
            None
        );
        assert_eq!(raw_upstream_override(""), None);
        assert_eq!(
            raw_upstream_override("LLM_BASE_URL=http://talaria:5273/api/llm/v1\n"),
            None,
            "a gateway-shaped URL is ours to manage"
        );
        // A real raw upstream stays an override.
        assert_eq!(
            raw_upstream_override("LLM_BASE_URL=https://openrouter.ai/api/v1\n").as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
    }

    #[test]
    fn env_lines_read_trimmed_and_upsert_in_place() {
        let f = "A=1\nLLM_BASE_URL = http://x/api/llm/v1  \n# comment\nB=2\n";
        assert_eq!(
            read_env_line(f, "LLM_BASE_URL"),
            None,
            "the regex is anchored on the bare key — 'LLM_BASE_URL =' (spaced) is not a line it owns"
        );
        assert_eq!(read_env_line(f, "A"), Some("1".into()));
        assert_eq!(read_env_line(f, "B"), Some("2".into()));
        assert_eq!(read_env_line(f, "C"), None);

        let out = upsert_env_line(f, "A", "9");
        assert_eq!(
            out,
            "A=9\nLLM_BASE_URL = http://x/api/llm/v1  \n# comment\nB=2\n"
        );

        // Appends after stripping trailing newlines — and an empty file gains
        // a LEADING newline.
        assert_eq!(upsert_env_line("", "K", "v"), "\nK=v\n");
        assert_eq!(upsert_env_line("A=1\n\n\n", "K", "v"), "A=1\nK=v\n");
    }
}
