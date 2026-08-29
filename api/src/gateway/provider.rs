// Provider keys and catalogs — port of the key-resolution half of
// ui/src/server/provider-catalog.ts: NATIVE_BASE, the env-name gate, and
// resolveEndpointKey (sealed DB key first, env/fleet-.env fallback). The
// live model-catalog fetchers (catalogModels &c.) are a later batch — they
// serve the admin model picker, not the relay.

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

fn fleet_env_path() -> std::path::PathBuf {
    // FLEET_DIR = TALARIA_FLEET_DIR ?? <cwd>/../fleet (fleet-render.ts:38).
    if let Ok(d) = std::env::var("TALARIA_FLEET_DIR") {
        return std::path::PathBuf::from(d).join(".env");
    }
    std::env::current_dir()
        .map(|c| c.join("../fleet/.env"))
        .unwrap_or_else(|_| std::path::PathBuf::from("../fleet/.env"))
}

/// Resolve a key from the server env or the fleet .env — values never leave
/// the server. Port of resolveKey.
pub async fn resolve_key(env_var: Option<&str>) -> Option<String> {
    let env_var = env_var?;
    if !key_env_allowed(env_var) {
        return None;
    }
    // TS truthiness: an env var set but EMPTY falls through to the fleet .env.
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
    // TS returns "" for a bare `FOO=` line — falsy there; here empty IS "no
    // key" at every call site.
    (!v.is_empty()).then_some(v)
}

/// The provider's API key: the sealed key stored in the DB is the source of
/// truth (encrypted at rest, entered via /models — never in a config file).
/// An env var / fleet .env of the endpoint's api_key_env is a fallback for
/// ops overrides and pre-migration deployments. Port of resolveEndpointKey.
pub async fn resolve_endpoint_key(
    state: &AppState,
    ep: &crate::gateway::registry::LlmEndpoint,
) -> Option<String> {
    let cipher: Option<String> =
        sqlx::query_scalar("select api_key_cipher from llm_endpoints where id::text = $1")
            .bind(&ep.id)
            .fetch_optional(&state.pg)
            .await
            .ok()
            .flatten();
    if let Some(token) = cipher
        && let Ok(sb) = state.secretbox().await
        && let Ok(key) = sb.open(&token)
    {
        return Some(key);
    }
    // tampered/old key material — fall through to env
    let env_name = ep
        .api_key_env
        .as_deref()
        .or_else(|| default_key_env(&ep.provider));
    resolve_key(env_name).await
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct UsPoolCache {
    at: u64,
    slugs: Vec<String>,
}

/// OpenRouter's live US provider pool for no-train routing: every provider
/// whose datacenters include the US (or, when datacenters are unreported, is
/// US-headquartered). Fetched fresh — never a maintained list — and cached
/// briefly; a failed fetch serves the last good pool. Port of openrouterUsPool.
pub async fn openrouter_us_pool() -> Option<Vec<String>> {
    const TTL_MS: u64 = 15 * 60_000;
    {
        let cache = us_pool().lock().ok()?;
        if let Some(c) = cache.as_ref()
            && now_ms() - c.at < TTL_MS
        {
            return Some(c.slugs.clone());
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
                    slugs: slugs.clone(),
                });
            }
            Some(slugs)
        }
        None => us_pool()
            .lock()
            .ok()
            .and_then(|c| c.as_ref().map(|c| c.slugs.clone())),
    }
}

fn us_pool() -> &'static Mutex<Option<UsPoolCache>> {
    static CACHE: OnceLock<Mutex<Option<UsPoolCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// The shared outbound client. Loopback and provider hosts, rustls, HTTP/2 —
/// one pool, no per-request client construction.
pub fn http() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
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
}
