// Provider keys and catalogs: the native bases, the env-name gate, key
// resolution (sealed DB key first, env/fleet-.env fallback), and the live
// model-catalog fetchers that fill the stored catalog the effort ladder reads.

use crate::state::AppState;
use regex::Regex;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn native_base(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "anthropic" => "https://api.anthropic.com/v1",
        "openai" => "https://api.openai.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "deepseek" => "https://api.deepseek.com/v1",
        "x-ai" => "https://api.x.ai/v1",
        _ => return None,
    })
}

pub fn default_key_env(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "x-ai" => "XAI_API_KEY",
        _ => return None,
    })
}

/// Env names the key resolver may read. Only provider-key-shaped vars — an
/// endpoint pairs an admin-chosen env name with an admin-chosen base URL, so
/// without this gate an admin could exfiltrate ANY env value (GH_TOKEN,
/// HERMES_KEY_*, LITELLM_MASTER_KEY) to an arbitrary host. Cross-provider
/// key-to-wrong-host remains possible for *_API_KEY names — that's the same
/// residual class as agent-config rendering and is part of the admin trust
/// model; everything else is refused here.
pub fn key_env_allowed(env_var: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(LLM_API_KEY|[A-Z][A-Z0-9_]*_API_KEY)$").unwrap())
        .is_match(env_var)
}

/// FLEET_DIR — the render output tree whose fleet.json is the transport table
/// of where each agent's gateway lives. The path rule lives in fleet_layout
/// now that the write plane resolves through it too; this is the read side's
/// alias for the same one definition.
pub(crate) fn fleet_dir() -> std::path::PathBuf {
    crate::fleet::layout::fleet_dir()
}

fn fleet_env_path() -> std::path::PathBuf {
    fleet_dir().join(".env")
}

/// Resolve a key from the server env or the fleet .env — values never leave
/// the server.
pub async fn resolve_key(env_var: Option<&str>) -> Option<String> {
    let env_var = env_var?;
    if !key_env_allowed(env_var) {
        return None;
    }
    // An env var set but EMPTY falls through to the fleet .env.
    if let Ok(v) = std::env::var(env_var)
        && !v.is_empty()
    {
        return Some(v.trim().to_string());
    }
    let env = tokio::fs::read_to_string(fleet_env_path())
        .await
        .unwrap_or_default();
    let prefix = format!("{env_var}=");
    let line = env.lines().find(|l| l.starts_with(&prefix))?;
    let v = line[prefix.len()..].trim().to_string();
    // A bare `FOO=` line yields empty — and empty IS "no
    // key" at every call site.
    (!v.is_empty()).then_some(v)
}

/// The resolved key — cipher decrypt or env fallback, whichever won — is
/// served from a 60s window keyed by endpoint id: this used to be a
/// checkout AND a secretbox open on every completion, for a value that
/// changes only by admin rotation. The plaintext it guards already lives in
/// memory for the length of every upstream call and, on the env-fallback
/// path, in the environment forever — the cache extends its residence, not
/// its exposure. Key writes invalidate (see `invalidate_endpoint_key`).
const ENDPOINT_KEY_TTL: Duration = Duration::from_secs(60);

fn endpoint_key_cache(
) -> &'static Mutex<std::collections::HashMap<String, (std::time::Instant, Option<String>)>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, (std::time::Instant, Option<String>)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// The provider's API key: the sealed key stored in the DB is the source of
/// truth (encrypted at rest, entered via /models — never in a config file).
/// An env var / fleet .env of the endpoint's api_key_env is a fallback for
/// ops overrides and pre-migration deployments.
pub async fn resolve_endpoint_key(
    state: &AppState,
    ep: &crate::gateway::registry::LlmEndpoint,
) -> Option<String> {
    {
        let c = endpoint_key_cache().lock().expect("endpoint key cache");
        if let Some((at, hit)) = c.get(&ep.id)
            && at.elapsed() < ENDPOINT_KEY_TTL
        {
            return hit.clone();
        }
    }
    let cipher: Option<String> =
        sqlx::query_scalar("select api_key_cipher from llm_endpoints where id::text = $1")
            .bind(&ep.id)
            .fetch_optional(&state.pg)
            .await
            .ok()
            .flatten();
    let resolved = if let Some(token) = cipher
        && let Ok(sb) = state.secretbox().await
        && let Ok(key) = sb.open(&token)
    {
        Some(key)
    } else {
        // tampered/old key material — fall through to env
        let env_name = ep
            .api_key_env
            .as_deref()
            .or_else(|| default_key_env(&ep.provider));
        resolve_key(env_name).await
    };
    endpoint_key_cache()
        .lock()
        .expect("endpoint key cache")
        .insert(ep.id.clone(), (std::time::Instant::now(), resolved.clone()));
    resolved
}

/// Drop an endpoint's key entry — or, with no id, the whole map (the boot
/// migration's shape). Every writer of `api_key_cipher` calls this: the
/// admin key write in registry.rs, `migrate_env_keys_to_cipher` here, and
/// secret_health's clear.
pub fn invalidate_endpoint_key(id: Option<&str>) {
    let mut c = endpoint_key_cache().lock().expect("endpoint key cache");
    match id {
        Some(id) => {
            c.remove(id);
        }
        None => c.clear(),
    }
}

/// One-time migration: seal any provider key that still lives only in the env
/// / fleet .env into the DB, so keys stop depending on config files.
/// Idempotent — only touches endpoints with no sealed key yet. Safe to call
/// on every boot. Answers how many keys it sealed.
pub async fn migrate_env_keys_to_cipher(state: &AppState) -> Result<usize, sqlx::Error> {
    let eps: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "select id::text, provider, api_key_env from llm_endpoints \
         where api_key_cipher is null",
    )
    .fetch_all(&state.pg)
    .await?;
    let Ok(sb) = state.secretbox().await else {
        return Ok(0);
    };
    let mut sealed = 0;
    for (id, provider, api_key_env) in eps {
        let env_name = api_key_env
            .as_deref()
            .or_else(|| default_key_env(&provider));
        let Some(key) = resolve_key(env_name).await else {
            continue;
        };
        if let Ok(cipher) = sb.seal(&key)
            && sqlx::query(
                "update llm_endpoints set api_key_cipher = $2, updated_at = now() \
                 where id = $1::uuid",
            )
            .bind(&id)
            .bind(cipher)
            .execute(&state.pg)
            .await
            .is_ok()
        {
            sealed += 1;
        }
    }
    invalidate_endpoint_key(None);
    Ok(sealed)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct UsPoolCache {
    at: u64,
    /// When the last fetch attempt FAILED — while OpenRouter is unreachable
    /// the retry cadence backs off to one per minute instead of hanging a
    /// 10s fetch on every completion.
    failed_at: Option<u64>,
    slugs: Vec<String>,
}

const US_POOL_TTL_MS: u64 = 15 * 60_000;
const US_POOL_FAILURE_BACKOFF_MS: u64 = 60_000;

/// Serve the cache as-is, or go fetch. Pure so the window arithmetic is
/// testable without the network.
fn us_pool_decision(at: u64, failed_at: Option<u64>, now: u64) -> UsPoolDecision {
    if now.saturating_sub(at) < US_POOL_TTL_MS {
        return UsPoolDecision::Serve;
    }
    if failed_at.is_some_and(|f| now.saturating_sub(f) < US_POOL_FAILURE_BACKOFF_MS) {
        return UsPoolDecision::Serve;
    }
    UsPoolDecision::Fetch
}

#[derive(Debug, PartialEq, Eq)]
enum UsPoolDecision {
    Serve,
    Fetch,
}

fn us_pool_flight() -> &'static tokio::sync::Mutex<()> {
    static F: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    F.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// OpenRouter's live US provider pool for no-train routing: every provider
/// whose datacenters include the US (or, when datacenters are unreported, is
/// US-headquartered). Fetched fresh — never a maintained list — and cached
/// briefly; a failed fetch serves the last good pool. ONE flight: at a TTL
/// lapse under N concurrent completions, the first fetcher refreshes and the
/// rest join its result instead of each hanging on its own 10s call.
pub async fn openrouter_us_pool() -> Option<Vec<String>> {
    {
        let cache = us_pool().lock().ok()?;
        if let Some(c) = cache.as_ref()
            && us_pool_decision(c.at, c.failed_at, now_ms()) == UsPoolDecision::Serve
        {
            return (!c.slugs.is_empty()).then(|| c.slugs.clone());
        }
    }
    let _held = us_pool_flight().lock().await;
    // Re-check inside the gate: whoever finished first refreshed the window
    // (or stamped a failure), and everyone queued behind them stands down.
    {
        let cache = us_pool().lock().ok()?;
        if let Some(c) = cache.as_ref()
            && us_pool_decision(c.at, c.failed_at, now_ms()) == UsPoolDecision::Serve
        {
            return (!c.slugs.is_empty()).then(|| c.slugs.clone());
        }
    }
    let fetch = async {
        let client = http();
        let r = client
            .get("https://openrouter.ai/api/v1/providers")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        if !r.status().is_success() {
            return None;
        }
        let j: serde_json::Value = r.json().await.ok()?;
        let data = j.get("data")?.as_array()?;
        let mut slugs: Vec<String> = data
            .iter()
            .filter(|p| {
                let slug = p.get("slug").and_then(|s| s.as_str());
                if slug.is_none() {
                    return false;
                }
                let dcs = p.get("datacenters").and_then(|d| d.as_array());
                match dcs {
                    Some(dcs) if !dcs.is_empty() => dcs.iter().any(|d| d.as_str() == Some("US")),
                    _ => p.get("headquarters").and_then(|h| h.as_str()) == Some("US"),
                }
            })
            .filter_map(|p| p.get("slug").and_then(|s| s.as_str()).map(String::from))
            .collect();
        slugs.sort();
        (!slugs.is_empty()).then_some(slugs)
    };
    match fetch.await {
        Some(slugs) => {
            if let Ok(mut cache) = us_pool().lock() {
                *cache = Some(UsPoolCache {
                    at: now_ms(),
                    failed_at: None,
                    slugs: slugs.clone(),
                });
            }
            Some(slugs)
        }
        None => {
            // Stamp the failure, keep the last good pool serving.
            if let Ok(mut cache) = us_pool().lock()
                && let Some(c) = cache.as_mut()
            {
                c.failed_at = Some(now_ms());
            }
            us_pool()
                .lock()
                .ok()
                .and_then(|c| c.as_ref().map(|c| c.slugs.clone()))
        }
    }
}

fn us_pool() -> &'static Mutex<Option<UsPoolCache>> {
    static CACHE: OnceLock<Mutex<Option<UsPoolCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

// ── Live model catalogs ───────────────────────────────────────────────────────

/// USD per token parsed to per-MILLION tokens — the way everything else in
/// Talaria quotes a price. Each half independently absent.
pub struct CatalogPricing {
    pub in_per_m_tok: Option<f64>,
    pub out_per_m_tok: Option<f64>,
}

/// ONE MODEL AS THE PROVIDER DESCRIBES IT — the fields every OpenAI-compatible
/// catalog that publishes them agrees on, normalized.
///
/// WHY THIS TYPE EXISTS AT ALL: `string[]` ids once threw away OpenRouter's
/// `context_length`, `architecture.input_modalities`, `supported_parameters`
/// and `pricing`, and the cost was not abstract — the fitness page reported
/// "nothing advertises a context window" for a model whose catalog entry says
/// 1,048,576, and `tools`/`json`/`json-strict`/`vision` sat unmeasured on the
/// matrix while the provider had already answered all four for free.
///
/// EVERY FIELD IS ABSENT-BY-DEFAULT AND MEANS "THE PROVIDER DID NOT SAY".
/// Never "no" — see `capabilities_from_catalog` for why that distinction is
/// load-bearing rather than pedantic. All keys always serialize, so a stored
/// row written by this build can be told from a pre-feature one by the
/// `efforts` key being present (null included) — the backfill keys off that.
pub struct CatalogModel {
    pub id: String,
    pub name: Option<String>,
    pub context_length: Option<f64>,
    pub input_modalities: Option<Vec<String>>,
    pub supported_parameters: Option<Vec<String>>,
    /// The reasoning-effort levels this model accepts, exactly as the provider
    /// spells them. Absent when unpublished: a model with no list is a model
    /// nobody has vouched for, not a model that was refused.
    pub efforts: Option<Vec<String>>,
    pub pricing: Option<CatalogPricing>,
}

/// Integral numbers serialize without a decimal (`1048576`, not
/// `1048576.0`) — the integral range goes out as integers.
fn number_json(n: f64) -> serde_json::Value {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        serde_json::Value::Number((n as i64).into())
    } else {
        serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    }
}

impl CatalogModel {
    /// The stored/wire shape, every key always, in declaration order.
    pub fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("id".into(), serde_json::Value::String(self.id.clone()));
        o.insert(
            "name".into(),
            self.name
                .clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        o.insert(
            "contextLength".into(),
            self.context_length
                .map(number_json)
                .unwrap_or(serde_json::Value::Null),
        );
        o.insert(
            "inputModalities".into(),
            str_list_json(self.input_modalities.as_deref()),
        );
        o.insert(
            "supportedParameters".into(),
            str_list_json(self.supported_parameters.as_deref()),
        );
        o.insert("efforts".into(), str_list_json(self.efforts.as_deref()));
        let pricing = match &self.pricing {
            None => serde_json::Value::Null,
            Some(p) => {
                let mut po = serde_json::Map::new();
                po.insert(
                    "inPerMTok".into(),
                    p.in_per_m_tok
                        .map(number_json)
                        .unwrap_or(serde_json::Value::Null),
                );
                po.insert(
                    "outPerMTok".into(),
                    p.out_per_m_tok
                        .map(number_json)
                        .unwrap_or(serde_json::Value::Null),
                );
                serde_json::Value::Object(po)
            }
        };
        o.insert("pricing".into(), pricing);
        serde_json::Value::Object(o)
    }

    /// Read one back from the stored catalog. `model_catalog` is JSON that
    /// outlives the build that wrote it — anything off-shape is skipped, never
    /// a panic.
    pub fn from_json(v: &serde_json::Value) -> Option<CatalogModel> {
        let o = v.as_object()?;
        let id = o.get("id")?.as_str()?;
        if id.is_empty() {
            return None;
        }
        let list = |k: &str| -> Option<Option<Vec<String>>> {
            match o.get(k) {
                None => Some(None), // a pre-feature row: key absent
                Some(serde_json::Value::Null) => Some(None),
                Some(serde_json::Value::Array(a)) => {
                    let mut out = Vec::with_capacity(a.len());
                    for x in a {
                        out.push(x.as_str()?.to_string());
                    }
                    Some(Some(out))
                }
                Some(_) => None, // wrong shape: not a model row we can read
            }
        };
        Some(CatalogModel {
            id: id.to_string(),
            name: o.get("name").and_then(|n| n.as_str()).map(String::from),
            context_length: o.get("contextLength").and_then(|n| n.as_f64()),
            input_modalities: list("inputModalities")?,
            supported_parameters: list("supportedParameters")?,
            efforts: list("efforts")?,
            pricing: match o.get("pricing") {
                Some(serde_json::Value::Object(p)) => Some(CatalogPricing {
                    in_per_m_tok: p.get("inPerMTok").and_then(|n| n.as_f64()),
                    out_per_m_tok: p.get("outPerMTok").and_then(|n| n.as_f64()),
                }),
                _ => None,
            },
        })
    }
}

fn str_list_json(list: Option<&[String]>) -> serde_json::Value {
    match list {
        None => serde_json::Value::Null,
        Some(v) => serde_json::Value::Array(
            v.iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        ),
    }
}

/// A JSON number where it is one; a numeric string parsed (trimmed;
/// empty/whitespace → 0); null for everything else. Hex strings are null,
/// not parsed — no provider prices in hex.
fn js_num(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                Some(0.0)
            } else {
                t.parse::<f64>().ok()
            }
        }
        _ => None,
    }
}

/// An array whose every element is a string, else null. An empty
/// array is a valid answer (explicitly no modalities), distinct from null.
fn js_strings(v: &serde_json::Value) -> Option<Vec<String>> {
    let a = v.as_array()?;
    let mut out = Vec::with_capacity(a.len());
    for x in a {
        out.push(x.as_str()?.to_string());
    }
    Some(out)
}

/// THE SMALLER OF THE WINDOWS OPENROUTER PUBLISHES. `context_length` is the
/// model's own; `top_provider.context_length` is what the endpoint it routes
/// to will actually accept, and it is routinely lower. A claim has to hold
/// for the request that gets made, not for the spec sheet.
fn window_of(m: &serde_json::Value) -> Option<f64> {
    let mut best: Option<f64> = None;
    for raw in [
        m.get("context_length"),
        m.get("top_provider").and_then(|t| t.get("context_length")),
        m.get("max_context_length"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(n) = js_num(raw)
            && n > 0.0
            && best.is_none_or(|b| n < b)
        {
            best = Some(n);
        }
    }
    best
}

/// `gemini` normalizes `models/`-prefixed ids to the bare id
/// its chat API is documented with.
fn to_catalog_model(raw: &serde_json::Value, gemini: bool) -> Option<CatalogModel> {
    // id falls back to name only when absent/null — a present-but-empty id
    // rejects the model without consulting name.
    let id = match raw.get("id") {
        Some(serde_json::Value::Null) | None => raw
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())?,
        Some(v) => v.as_str().filter(|s| !s.is_empty())?,
    };
    let id = if gemini {
        id.strip_prefix("models/").unwrap_or(id)
    } else {
        id
    };
    // `modality: 'text+image->text'` is the older spelling, still served by
    // some entries. Split it rather than ignore it — it is the only vision
    // signal those rows carry. For a string modality the answer is always an
    // array (possibly empty), never null.
    let arch = raw.get("architecture").and_then(|a| a.as_object());
    let modalities = match arch.and_then(|a| a.get("input_modalities")) {
        Some(v) => js_strings(v),
        None => arch
            .and_then(|a| a.get("modality"))
            .and_then(|m| m.as_str())
            .map(|m| {
                m.split("->")
                    .next()
                    .unwrap_or("")
                    .split('+')
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect::<Vec<_>>()
            }),
    };
    let in_tok = raw
        .get("pricing")
        .and_then(|p| p.get("prompt"))
        .and_then(js_num);
    let out_tok = raw
        .get("pricing")
        .and_then(|p| p.get("completion"))
        .and_then(js_num);
    let efforts = raw
        .get("reasoning")
        .and_then(|r| r.get("supported_efforts"))
        .and_then(js_strings)
        // An empty list means "not vouched for", never "cannot".
        .filter(|l| !l.is_empty());
    Some(CatalogModel {
        id: id.to_string(),
        name: raw.get("name").and_then(|n| n.as_str()).map(String::from),
        context_length: window_of(raw),
        input_modalities: modalities,
        supported_parameters: raw.get("supported_parameters").and_then(js_strings),
        efforts,
        pricing: match (in_tok, out_tok) {
            (None, None) => None,
            (in_per_m_tok, out_per_m_tok) => Some(CatalogPricing {
                in_per_m_tok: in_per_m_tok.map(|n| n * 1e6),
                out_per_m_tok: out_per_m_tok.map(|n| n * 1e6),
            }),
        },
    })
}

/// Perplexity has no /models API — its DOCS are the catalog. The models index
/// (Mintlify serves it as markdown) links one card per model, and each card's
/// slug IS the API model id. Still live-fetched — no maintained list.
async fn perplexity_models() -> Result<Vec<String>, String> {
    let r = http()
        .get("https://docs.perplexity.ai/docs/sonar/models.md")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!(
            "the Perplexity docs answered {}",
            r.status().as_u16()
        ));
    }
    let text = r.text().await.map_err(|e| e.to_string())?;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"/docs/sonar/models/([a-z0-9][a-z0-9.-]*)").unwrap());
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    for cap in re.captures_iter(&text) {
        let id = cap[1].to_string();
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Err(
            "could not read the model list from the Perplexity docs. Type an id manually."
                .to_string(),
        );
    }
    Ok(ids)
}

/// `encodeURIComponent` — unreserved (+ JS's `!'()*`) pass through.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The localhost spelling of a docker-internal base URL, when the host is a
/// bare name (`inference-router` — no dot, not localhost): docker-internal
/// hostnames don't resolve from the host, and the compose stacks publish
/// their ports there. None when the fallback doesn't apply.
fn localhost_spelling(base: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^(https?)://([^/:]+)(:\d+)?(/.*)?$").unwrap());
    let m = re.captures(base)?;
    let host = m.get(2)?.as_str();
    if host.is_empty() || host.contains('.') || host == "localhost" {
        return None;
    }
    let port = m.get(3).map(|p| p.as_str()).unwrap_or("");
    let path = m.get(4).map(|p| p.as_str()).unwrap_or("");
    Some(format!("{}://localhost{port}{path}", m[1].to_lowercase()))
}

/// Fetch with the dev-mode localhost fallback (see `localhost_spelling`).
/// Err carries the transport error string; the caller maps !ok to its own.
async fn fetch_models(
    base: &str,
    headers: &[(String, String)],
    qs: &str,
) -> Result<reqwest::Response, String> {
    let attempt = |url: String| async move {
        let mut req = http().get(&url).timeout(Duration::from_secs(10));
        for (k, v) in headers {
            req = req.header(k, v);
        }
        // Transport failures surface as undici-style bare messages — this
        // string rides the `available` route's note verbatim (byte-stable).
        req.send()
            .await
            .map_err(|e| crate::mcp::registry::undici_message(&e))
    };
    let direct = attempt(format!("{base}/models{qs}")).await;
    match direct {
        Ok(r) => Ok(r),
        Err(err) => match localhost_spelling(base) {
            Some(local) => attempt(format!("{local}/models{qs}")).await,
            None => Err(err),
        },
    }
}

/// The models a provider reports right now, WITH everything it says about
/// them. Err carries the human message (logged inside
/// the refresh result; a failed fetch keeps the old entry).
pub async fn catalog_models(
    state: &AppState,
    ep: &crate::gateway::registry::LlmEndpoint,
) -> Result<Vec<CatalogModel>, String> {
    let base = match ep.base_url.as_deref().or_else(|| native_base(&ep.provider)) {
        Some(b) => b,
        None => return Err("no API base known for this provider".to_string()),
    };
    // Strip exactly ONE trailing slash.
    let base = base.strip_suffix('/').unwrap_or(base);
    // Perplexity has no catalog API; its docs give ids and nothing else, so
    // every descriptive field is honestly absent rather than guessed at.
    if base.contains("api.perplexity.ai") {
        return perplexity_models().await.map(|ids| {
            ids.into_iter()
                .map(|id| CatalogModel {
                    id,
                    name: None,
                    context_length: None,
                    input_modalities: None,
                    supported_parameters: None,
                    efforts: None,
                    pricing: None,
                })
                .collect()
        });
    }
    let key_env_name = ep
        .api_key_env
        .clone()
        .or_else(|| default_key_env(&ep.provider).map(String::from))
        .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string());
    let key = resolve_endpoint_key(state, ep).await;

    let mut headers: Vec<(String, String)> = Vec::new();
    if ep.provider == "anthropic" {
        let Some(key) = key else {
            return Err(format!(
                "add the API key for this provider (or set {key_env_name})"
            ));
        };
        headers.push(("x-api-key".into(), key));
        headers.push(("anthropic-version".into(), "2023-06-01".into()));
    } else if let Some(key) = key {
        headers.push(("Authorization".into(), format!("Bearer {key}")));
    }

    // Gemini's OpenAI-compat layer reports ids as "models/gemini-" while its
    // chat API is documented with the bare id.
    let gemini = base.contains("generativelanguage.googleapis.com");

    // Follow pagination where the provider pages (Anthropic defaults to 20
    // per page); OpenAI-compatible catalogs return everything in one response.
    // The provider's own ordering is preserved — OpenRouter lists newest
    // first, and first-wins dedupe depends on it.
    let mut out: Vec<CatalogModel> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let anthropic = ep.provider == "anthropic";
    let mut qs = if anthropic {
        "?limit=1000".to_string()
    } else {
        String::new()
    };
    for _page in 0..20 {
        let r = fetch_models(base, &headers, &qs).await?;
        if !r.status().is_success() {
            let status = r.status().as_u16();
            let hint = match status {
                401 => ": check the API key env",
                404 => {
                    ": this provider doesn't publish a model catalog; type model ids from its docs"
                }
                _ => "",
            };
            return Err(format!("provider answered {status}{hint}"));
        }
        let j: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
        let list: Vec<&serde_json::Value> = match &j {
            serde_json::Value::Array(a) => a.iter().collect(), // Together: bare array
            serde_json::Value::Object(o) => o
                .get("data")
                .or_else(|| o.get("models"))
                .and_then(|v| v.as_array())
                .map(|a| a.iter().collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for raw in list {
            let Some(m) = to_catalog_model(raw, gemini) else {
                continue;
            };
            // First entry wins — the provider's own ordering puts the
            // canonical row first.
            if seen.insert(m.id.clone()) {
                out.push(m);
            }
        }
        let last_id = match &j {
            serde_json::Value::Object(o) => o.get("last_id").and_then(|v| v.as_str()),
            _ => None,
        };
        let has_more = match &j {
            serde_json::Value::Object(o) => o.get("has_more").and_then(|v| v.as_bool()),
            _ => None,
        };
        if has_more != Some(true) || last_id.is_none() {
            break;
        }
        let last_id = last_id.unwrap_or_default();
        if last_id.is_empty() {
            break;
        }
        qs = format!(
            "{}after_id={}",
            if anthropic { "?limit=1000&" } else { "?" },
            encode_uri_component(last_id)
        );
    }
    Ok(out)
}

/// The shared outbound client. Loopback and provider hosts, rustls, HTTP/2 —
/// one pool, no per-request client construction.
pub fn http() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                // Only the CONNECT phase is bounded here: the per-request
                // timeouts own the rest (600s for a chat turn, 10s for
                // catalogs), and a provider that accepts TCP then stalls
                // must not hold an unstarted turn forever.
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client builds")
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_gate_admits_only_provider_key_shapes() {
        assert!(key_env_allowed("LLM_API_KEY"));
        assert!(key_env_allowed("ANTHROPIC_API_KEY"));
        assert!(key_env_allowed("SOMETHING_V2_API_KEY"));
        // The exfiltration shapes the gate exists to refuse.
        assert!(!key_env_allowed("GH_TOKEN"));
        assert!(!key_env_allowed("HERMES_KEY_MAIN"));
        assert!(!key_env_allowed("LITELLM_MASTER_KEY"));
        assert!(!key_env_allowed("PATH"));
        assert!(!key_env_allowed("API_KEY_LOWERCASE"));
    }

    #[test]
    fn the_us_pool_window_serves_fresh_backs_off_failure_refetches_after() {
        use UsPoolDecision::{Fetch, Serve};
        let now = 1_000_000_u64;
        // Fresh: any stamp inside 15 minutes serves.
        assert_eq!(us_pool_decision(now - 1, None, now), Serve);
        assert_eq!(us_pool_decision(now - US_POOL_TTL_MS + 1, None, now), Serve);
        // Stale with no failure on record: fetch.
        assert_eq!(us_pool_decision(now - US_POOL_TTL_MS, None, now), Fetch);
        // Stale with a recent failure: serve (stale) — one retry per minute
        // while OpenRouter is dark, not one per completion.
        assert_eq!(
            us_pool_decision(now - US_POOL_TTL_MS, Some(now - 1), now),
            Serve
        );
        assert_eq!(
            us_pool_decision(now - US_POOL_TTL_MS, Some(now - US_POOL_FAILURE_BACKOFF_MS + 1), now),
            Serve
        );
        // The backoff lapses: fetch again.
        assert_eq!(
            us_pool_decision(now - US_POOL_TTL_MS, Some(now - US_POOL_FAILURE_BACKOFF_MS), now),
            Fetch
        );
        // A clock that went backwards saturates rather than panics.
        assert_eq!(us_pool_decision(now + 1_000, Some(now + 2_000), now), Serve);
    }
}
