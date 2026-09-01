// Reranking — the precision stage after vector recall. Port of
// ui/src/server/retrieval/rerank.ts. Vector search across collections merges
// by raw cosine score (not truly comparable, and bi-encoder recall is fuzzy);
// a cross-encoder rescoring the query against each candidate fixes both.
// Providers are a registry (like LLM endpoints): pick one, add a key, choose
// a model — model lists are fetched LIVE where the provider has a catalog
// API, with documented fallbacks where none exists. Keys are sealed
// (envelope-encrypted) in app_settings; plaintext never reaches the client.
// Reranking is best-effort — a provider failure falls back to vector order,
// never breaks search.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::gateway::registry::list_endpoints;
use crate::gateway::settings::{get_setting, set_setting};
use crate::state::AppState;

use super::HttpFetch;

/// OpenRouter's key can come from the LLM endpoint the org already registered
/// — the rerank config only needs its own key when none exists there.
async fn openrouter_fallback_key(state: &AppState) -> Option<String> {
    let eps = list_endpoints(&state.pg).await.ok()?;
    let ep = eps
        .iter()
        .find(|e| e.provider == "openrouter" || e.name == "openrouter")?;
    crate::gateway::provider::resolve_endpoint_key(state, ep).await
}

pub struct RerankProviderMeta {
    pub id: &'static str,
    pub label: &'static str,
    /// HQ country — surfaced so no-train/US-routing shops choose knowingly.
    pub country: &'static str,
    pub needs_url: bool,
    pub needs_key: bool,
    /// Documented models — the fallback when there's no live catalog API.
    pub fallback_models: &'static [&'static str],
    /// Whether we can list models live (with a key).
    pub live_catalog: bool,
}

pub const RERANK_PROVIDERS: &[RerankProviderMeta] = &[
    RerankProviderMeta {
        id: "tei",
        label: "Self-hosted (TEI)",
        country: "your hardware",
        needs_url: true,
        needs_key: false,
        fallback_models: &[],
        live_catalog: false,
    },
    // OpenRouter reuses the LLM endpoint's registered key automatically when
    // no rerank-specific key is set — one key, whole stack.
    RerankProviderMeta {
        id: "openrouter",
        label: "OpenRouter",
        country: "US",
        needs_url: false,
        needs_key: true,
        fallback_models: &["cohere/rerank-v3.5", "baai/bge-reranker-v2-m3"],
        live_catalog: true,
    },
    RerankProviderMeta {
        id: "voyage",
        label: "Voyage AI",
        country: "US",
        needs_url: false,
        needs_key: true,
        fallback_models: &["rerank-2.5", "rerank-2.5-lite", "rerank-2", "rerank-2-lite"],
        live_catalog: false,
    },
    RerankProviderMeta {
        id: "together",
        label: "Together AI",
        country: "US",
        needs_url: false,
        needs_key: true,
        fallback_models: &[
            "Salesforce/Llama-Rank-V1",
            "mixedbread-ai/Mxbai-Rerank-Large-V2",
        ],
        live_catalog: true,
    },
    RerankProviderMeta {
        id: "nvidia",
        label: "NVIDIA",
        country: "US",
        needs_url: false,
        needs_key: true,
        fallback_models: &[
            "nvidia/llama-3.2-nv-rerankqa-1b-v2",
            "nvidia/nv-rerankqa-mistral-4b-v3",
        ],
        live_catalog: false,
    },
    RerankProviderMeta {
        id: "pinecone",
        label: "Pinecone",
        country: "US",
        needs_url: false,
        needs_key: true,
        fallback_models: &[
            "bge-reranker-v2-m3",
            "pinecone-rerank-v0",
            "cohere-rerank-3.5",
        ],
        live_catalog: true,
    },
    RerankProviderMeta {
        id: "cohere",
        label: "Cohere",
        country: "Canada",
        needs_url: false,
        needs_key: true,
        fallback_models: &[
            "rerank-v3.5",
            "rerank-english-v3.0",
            "rerank-multilingual-v3.0",
        ],
        live_catalog: true,
    },
    RerankProviderMeta {
        id: "jina",
        label: "Jina AI",
        country: "Germany",
        needs_url: false,
        needs_key: true,
        fallback_models: &["jina-reranker-v2-base-multilingual", "jina-reranker-m0"],
        live_catalog: false,
    },
];

/// The wire view of the provider catalog — the admin rag GET's `providers`
/// array. Same fields as the meta, in the TS key order the panel was built
/// against (id, label, country, needsUrl, needsKey, fallbackModels,
/// liveCatalog).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RerankProviderPublic {
    pub id: &'static str,
    pub label: &'static str,
    pub country: &'static str,
    pub needs_url: bool,
    pub needs_key: bool,
    pub fallback_models: &'static [&'static str],
    pub live_catalog: bool,
}

pub fn providers_public() -> Vec<RerankProviderPublic> {
    RERANK_PROVIDERS
        .iter()
        .map(|p| RerankProviderPublic {
            id: p.id,
            label: p.label,
            country: p.country,
            needs_url: p.needs_url,
            needs_key: p.needs_key,
            fallback_models: p.fallback_models,
            live_catalog: p.live_catalog,
        })
        .collect()
}

/// The stored shape. Serialized camelCase with absent fields OMITTED, byte
/// for byte the row the TS side writes — the same app_settings key is read
/// and written by both runtimes during coexistence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RerankConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_sealed: Option<String>,
    /// How many merged candidates to rescore (cost vs recall).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<i64>,
}

const KEY: &str = "rag_rerank_config";

fn defaults() -> RerankConfig {
    RerankConfig {
        provider: "off".into(),
        candidates: Some(30),
        ..Default::default()
    }
}

/// TS's `DEFAULTS = { provider: 'off', candidates: 30 }` — key order and all,
/// because the admin surfaces below carry the row through verbatim rather
/// than re-shaping it onto a struct.
fn defaults_value() -> Value {
    json!({"provider": "off", "candidates": 30})
}

/// The stored row verbatim — TS's `rows[0]?.value ?? fallback`. The column is
/// jsonb, so the keys come back in Postgres's own canonical order (shortest
/// key first); that is the order TS's JSON.parse keeps, and it flows straight
/// onto the admin wire below.
async fn stored_config(pg: &PgPool) -> Value {
    get_setting(pg, KEY, defaults_value()).await
}

pub async fn get_rerank_config(pg: &PgPool) -> RerankConfig {
    // The typed view the SEARCH path reads. TS carries the stored row as-is
    // (`rows[0]?.value ?? fallback`); a row whose shape no longer parses
    // falls back to the defaults here rather than poisoning every search —
    // recorded divergence, unreachable unless the key is hand-edited.
    serde_json::from_value(stored_config(pg).await).unwrap_or_else(|_| defaults())
}

/// The patch the admin route sends. TS distinguishes `undefined` (leave the
/// field alone) from `null` (clear it) on url/model/apiKey — the nested
/// `Option<Option<_>>` is that distinction; `candidates` is never cleared.
pub struct RerankPatch {
    pub provider: Option<String>,
    pub url: Option<Option<String>>,
    pub model: Option<Option<String>>,
    pub api_key: Option<Option<String>>,
    pub candidates: Option<i64>,
}

/// TS's `setRerankConfig` is a spread, not a merge onto a fixed shape:
/// `{...cur}` starts from the stored row's own key order, assigning an
/// existing key keeps its position while a NEW key appends, and the
/// `?? undefined` / apiKey-ternary paths leave the key ABSENT
/// (JSON.stringify drops undefined) rather than nulled. `key_sealed` is the
/// already-sealed token (or the clear) — sealing needs the box, so the
/// caller resolves it before this pure fold runs.
fn apply_patch(
    next: &mut serde_json::Map<String, Value>,
    patch: RerankPatch,
    key_sealed: Option<Option<String>>,
) {
    if let Some(p) = patch.provider {
        next.insert("provider".into(), json!(p));
    }
    if let Some(u) = patch.url {
        match u {
            Some(url) => {
                next.insert("url".into(), json!(url));
            }
            None => {
                next.remove("url");
            }
        };
    }
    if let Some(m) = patch.model {
        match m {
            Some(model) => {
                next.insert("model".into(), json!(model));
            }
            None => {
                next.remove("model");
            }
        };
    }
    if let Some(c) = patch.candidates {
        next.insert("candidates".into(), json!(c.clamp(5, 100)));
    }
    if let Some(k) = key_sealed {
        match k {
            Some(sealed) => {
                next.insert("keySealed".into(), json!(sealed));
            }
            None => {
                next.remove("keySealed");
            }
        };
    }
}

pub async fn set_rerank_config(state: &AppState, patch: RerankPatch) -> Result<Value, String> {
    // Seal BEFORE anything touches the row — a broken box must not leave a
    // half-written config behind.
    let key_sealed = match &patch.api_key {
        Some(Some(plaintext)) => {
            let sb = state.secretbox().await?;
            Some(Some(sb.seal(plaintext).map_err(|e| e.to_string())?))
        }
        Some(None) => Some(None), // null clears
        None => None,
    };
    let mut next = match stored_config(&state.pg).await {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    apply_patch(&mut next, patch, key_sealed);
    let v = Value::Object(next);
    set_setting(&state.pg, KEY, &v)
        .await
        .map_err(|e| e.to_string())?;
    // The column re-orders canonically on the write; the returned value is
    // the same pre-normalization `next` object TS's caller holds.
    Ok(v)
}

/// The public fold: `const { keySealed, ...rest } = cfg; return { ...rest,
/// hasKey: !!keySealed }` — the stored row's own keys in their own order,
/// keySealed gone, hasKey appended last. A passthrough, not a re-shaped
/// struct: the wire's key order is the jsonb row's order, nothing else.
fn public_of(stored: Value) -> Value {
    let mut map = match stored {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let has_key = map.get("keySealed").is_some_and(|k| !k.is_null());
    map.remove("keySealed");
    map.insert("hasKey".into(), json!(has_key));
    Value::Object(map)
}

/// Redacted view for the admin UI — never carries keySealed.
pub async fn rerank_config_public(pg: &PgPool) -> Value {
    public_of(stored_config(pg).await)
}

/// The jsonFetch port: 15s, non-ok is `"{status}: {first 200 chars}"`.
async fn json_fetch(
    http: &HttpFetch,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&Value>,
) -> Result<Value, String> {
    let hdrs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let (status, text) = (http)(method, url, body, &hdrs, 15_000).await?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "{status}: {}",
            crate::body::truncate_utf16(&text, 200)
        ));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

// ── Live catalog readers ────────────────────────────────────────────────────
// One pure reader per provider so the envelope each API actually speaks is
// pinned by a test, not by whichever dispatch branch was copied last.

fn cohere_catalog_ids(j: &Value) -> Vec<String> {
    j.get("models")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter_map(|m| {
                    m.get("name")
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty())
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn together_catalog_ids(j: &Value) -> Vec<String> {
    j.as_array()
        .map(|ms| {
            ms.iter()
                .filter(|m| m.get("type").and_then(Value::as_str) == Some("rerank"))
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn pinecone_catalog_ids(j: &Value) -> Vec<String> {
    j.get("models")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter_map(|m| {
                    m.get("model")
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty())
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn openrouter_catalog_ids(j: &Value) -> Vec<String> {
    j.get("data")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter(|m| {
                    let Some(id) = m
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|i| !i.is_empty())
                    else {
                        return false;
                    };
                    let tagged = m
                        .get("architecture")
                        .and_then(|a| a.get("output_modalities"))
                        .and_then(Value::as_array)
                        .is_some_and(|mods| mods.iter().any(|x| x == "rerank"));
                    tagged || id.to_lowercase().contains("rerank")
                })
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Live model catalog for a provider (falls back to the documented list).
/// Mirrors the LLM-endpoint pattern: fetch live wherever an API exists.
/// A sealed key that cannot be OPENED is an error, not a silent fallback —
/// the TS `open()` throws here too, before its try/catch begins.
pub async fn rerank_models(
    state: &AppState,
    http: &HttpFetch,
    provider: &str,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    let Some(meta) = RERANK_PROVIDERS.iter().find(|p| p.id == provider) else {
        return Ok(Vec::new());
    };
    let mut plaintext = api_key;
    if plaintext.is_none() {
        let cfg = get_rerank_config(&state.pg).await;
        if cfg.provider == provider
            && let Some(sealed) = &cfg.key_sealed
        {
            let sb = state.secretbox().await?;
            plaintext = Some(sb.open(sealed).map_err(|e| e.to_string())?);
        }
    }
    // Any catalog failure falls through to the documented list.
    let live = async {
        let key_header = |k: &str| vec![("authorization".to_string(), format!("Bearer {k}"))];
        match provider {
            "cohere" => {
                if let Some(key) = &plaintext {
                    let j = json_fetch(
                        http,
                        "GET",
                        "https://api.cohere.com/v1/models?endpoint=rerank&page_size=50",
                        &key_header(key),
                        None,
                    )
                    .await?;
                    let ids = cohere_catalog_ids(&j);
                    if !ids.is_empty() {
                        return Ok::<Vec<String>, String>(ids);
                    }
                }
            }
            "together" => {
                if let Some(key) = &plaintext {
                    let j = json_fetch(
                        http,
                        "GET",
                        "https://api.together.xyz/v1/models",
                        &key_header(key),
                        None,
                    )
                    .await?;
                    let ids = together_catalog_ids(&j);
                    if !ids.is_empty() {
                        return Ok(ids);
                    }
                }
            }
            "pinecone" => {
                if let Some(key) = &plaintext {
                    let j = json_fetch(
                        http,
                        "GET",
                        "https://api.pinecone.io/models?type=rerank",
                        &[
                            ("Api-Key".to_string(), key.clone()),
                            ("X-Pinecone-API-Version".to_string(), "2025-01".to_string()),
                        ],
                        None,
                    )
                    .await?;
                    let ids = pinecone_catalog_ids(&j);
                    if !ids.is_empty() {
                        return Ok(ids);
                    }
                }
            }
            "openrouter" => {
                // The public catalog needs no key; rerank models are the ones
                // tagged so — or whose id says so.
                let j = json_fetch(
                    http,
                    "GET",
                    "https://openrouter.ai/api/v1/models",
                    &[],
                    None,
                )
                .await?;
                let ids = openrouter_catalog_ids(&j);
                if !ids.is_empty() {
                    return Ok(ids);
                }
            }
            _ => {}
        }
        Ok(Vec::new())
    }
    .await
    .unwrap_or_default();
    if !live.is_empty() {
        return Ok(live);
    }
    Ok(meta.fallback_models.iter().map(|s| s.to_string()).collect())
}

/// One rescored candidate. A row missing its index or score is dropped here —
/// the TS maps it to `undefined` fields and `align` skips it one step later;
// same net effect.
struct Scored {
    index: i64,
    score: f64,
}

fn scored_from(arr: &[Value], score_key: &str) -> Vec<Scored> {
    arr.iter()
        .filter_map(|e| {
            let o = e.as_object()?;
            Some(Scored {
                index: o.get("index").and_then(Value::as_i64)?,
                score: o.get(score_key).and_then(Value::as_f64)?,
            })
        })
        .collect()
}

/// Per-candidate scores aligned to the input order (absent candidates score
/// 0), bounds-checked — a provider answering with an index past the pool
/// cannot panic us or silently shift the ranking.
fn align(scored: &[Scored], len: usize) -> Vec<f64> {
    let mut scores = vec![0.0; len];
    for s in scored {
        if s.index >= 0 && (s.index as usize) < len {
            scores[s.index as usize] = s.score;
        }
    }
    scores
}

/// Unseal the configured key — the TS `open(cfg.keySealed)`, which throws on
/// a bad token and lands in the outer catch → null. Every failure here is
/// None, never a panic and never a search failure.
async fn open_key(state: &AppState, sealed: &str) -> Option<String> {
    state.secretbox().await.ok()?.open(sealed).ok()
}

/// Rescore candidates against the query. Returns per-candidate scores aligned
/// to the input order, or None when reranking is off/unconfigured/failing —
/// every arm of the TS switch that isn't a scored answer is null, and its
/// one catch turns every thrown error into null too.
pub async fn rerank(
    state: &AppState,
    http: &HttpFetch,
    query: &str,
    texts: &[String],
) -> Option<Vec<f64>> {
    if texts.is_empty() {
        return None;
    }
    let cfg = get_rerank_config(&state.pg).await;
    let scored = rerank_dispatch(state, http, &cfg, query, texts).await?;
    Some(align(&scored, texts.len()))
}

/// The TS switch, factored from the config read so the dispatch is drivable
/// against a stated config (and testable without a live app_settings row).
/// Every failure inside is the catch: None, never an error.
async fn rerank_dispatch(
    state: &AppState,
    http: &HttpFetch,
    cfg: &RerankConfig,
    query: &str,
    texts: &[String],
) -> Option<Vec<Scored>> {
    let bearer = |key: &str| vec![("authorization".to_string(), format!("Bearer {key}"))];

    let scored: Option<Vec<Scored>> = match cfg.provider.as_str() {
        "tei" => {
            let url = cfg.url.as_ref()?;
            let j = json_fetch(
                http,
                "POST",
                &format!("{}/rerank", url.trim_end_matches('/')),
                &[],
                Some(&json!({ "query": query, "texts": texts, "truncate": true })),
            )
            .await
            .ok()?;
            let arr = j.as_array()?;
            Some(scored_from(arr, "score"))
        }
        "openrouter" => {
            let key = match &cfg.key_sealed {
                Some(sealed) => open_key(state, sealed).await,
                None => openrouter_fallback_key(state).await,
            }?;
            let j = json_fetch(
                http,
                "POST",
                "https://openrouter.ai/api/v1/rerank",
                &bearer(&key),
                Some(&json!({
                    "model": cfg.model.as_deref().unwrap_or("cohere/rerank-v3.5"),
                    "query": query,
                    "documents": texts,
                })),
            )
            .await
            .ok()?;
            Some(scored_from(
                j.get("results").and_then(Value::as_array)?,
                "relevance_score",
            ))
        }
        "voyage" => {
            let sealed = cfg.key_sealed.as_ref()?;
            let key = open_key(state, sealed).await?;
            let j = json_fetch(
                http,
                "POST",
                "https://api.voyageai.com/v1/rerank",
                &bearer(&key),
                Some(&json!({
                    "model": cfg.model.as_deref().unwrap_or("rerank-2.5-lite"),
                    "query": query,
                    "documents": texts,
                })),
            )
            .await
            .ok()?;
            Some(scored_from(
                j.get("data").and_then(Value::as_array)?,
                "relevance_score",
            ))
        }
        "together" => {
            let sealed = cfg.key_sealed.as_ref()?;
            let key = open_key(state, sealed).await?;
            let j = json_fetch(
                http,
                "POST",
                "https://api.together.xyz/v1/rerank",
                &bearer(&key),
                Some(&json!({
                    "model": cfg.model.as_deref().unwrap_or("Salesforce/Llama-Rank-V1"),
                    "query": query,
                    "documents": texts,
                })),
            )
            .await
            .ok()?;
            Some(scored_from(
                j.get("results").and_then(Value::as_array)?,
                "relevance_score",
            ))
        }
        "nvidia" => {
            let sealed = cfg.key_sealed.as_ref()?;
            let key = open_key(state, sealed).await?;
            let model = cfg
                .model
                .as_deref()
                .unwrap_or("nvidia/llama-3.2-nv-rerankqa-1b-v2");
            // The TS builds this URL through `model.replace('/', '/')` — a
            // no-op that says "slashes are meant to be here"; the path keeps
            // the model's own slash.
            let j = json_fetch(
                http,
                "POST",
                &format!("https://ai.api.nvidia.com/v1/retrieval/{model}/reranking"),
                &bearer(&key),
                Some(&json!({
                    "model": model,
                    "query": { "text": query },
                    "passages": texts.iter().map(|t| json!({ "text": t })).collect::<Vec<_>>(),
                })),
            )
            .await
            .ok()?;
            Some(scored_from(
                j.get("rankings").and_then(Value::as_array)?,
                "logit",
            ))
        }
        "pinecone" => {
            let sealed = cfg.key_sealed.as_ref()?;
            let key = open_key(state, sealed).await?;
            let j = json_fetch(
                http,
                "POST",
                "https://api.pinecone.io/rerank",
                &[
                    ("Api-Key".to_string(), key.clone()),
                    ("X-Pinecone-API-Version".to_string(), "2025-01".to_string()),
                ],
                Some(&json!({
                    "model": cfg.model.as_deref().unwrap_or("bge-reranker-v2-m3"),
                    "query": query,
                    "documents": texts.iter().map(|t| json!({ "text": t })).collect::<Vec<_>>(),
                })),
            )
            .await
            .ok()?;
            Some(scored_from(
                j.get("data").and_then(Value::as_array)?,
                "score",
            ))
        }
        "cohere" => {
            let sealed = cfg.key_sealed.as_ref()?;
            let key = open_key(state, sealed).await?;
            let j = json_fetch(
                http,
                "POST",
                "https://api.cohere.com/v2/rerank",
                &bearer(&key),
                Some(&json!({
                    "model": cfg.model.as_deref().unwrap_or("rerank-v3.5"),
                    "query": query,
                    "documents": texts,
                })),
            )
            .await
            .ok()?;
            Some(scored_from(
                j.get("results").and_then(Value::as_array)?,
                "relevance_score",
            ))
        }
        "jina" => {
            let sealed = cfg.key_sealed.as_ref()?;
            let key = open_key(state, sealed).await?;
            let j = json_fetch(
                http,
                "POST",
                "https://api.jina.ai/v1/rerank",
                &bearer(&key),
                Some(&json!({
                    "model": cfg.model.as_deref().unwrap_or("jina-reranker-v2-base-multilingual"),
                    "query": query,
                    "documents": texts,
                })),
            )
            .await
            .ok()?;
            Some(scored_from(
                j.get("results").and_then(Value::as_array)?,
                "relevance_score",
            ))
        }
        _ => None, // 'off' and anything unknown
    };
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// What the scripted edge saw: (url, body) per call.
    type Seen = Arc<std::sync::Mutex<Vec<(String, Option<Value>)>>>;

    fn http_scripted(script: Vec<(&'static str, u16, &'static str)>) -> (HttpFetch, Seen) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let fetch = {
            let seen = seen.clone();
            Arc::new(
                move |_m: &str, url: &str, b: Option<&Value>, _h: &[(&str, &str)], _t: u64| {
                    let seen = seen.clone();
                    let script = script.clone();
                    let url = url.to_string();
                    let b = b.cloned();
                    Box::pin(async move {
                        seen.lock().unwrap().push((url.clone(), b));
                        for (prefix, status, body) in &script {
                            if url.starts_with(prefix) {
                                return Ok((*status, body.to_string()));
                            }
                        }
                        Ok((404u16, "{}".to_string()))
                    })
                        as futures_util::future::BoxFuture<'static, Result<(u16, String), String>>
                },
            )
        };
        (fetch, seen)
    }

    fn lazy_state() -> AppState {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://rerank-test@localhost:5432/rerank-test")
            .expect("a lazy pool connects to nothing");
        let cfg = crate::config::Config::from_parts(
            "postgres://rerank-test@localhost:5432/rerank-test".to_string(),
            "redis://rerank-test@localhost:6379".to_string(),
            "test-root".to_string(),
            String::new(),
            String::new(),
            "0".to_string(),
        )
        .expect("a test config has nothing to reject");
        AppState::new(pg, Arc::new(cfg))
    }

    #[test]
    fn the_provider_table_is_the_admin_face_of_the_registry() {
        assert_eq!(RERANK_PROVIDERS.len(), 8);
        let tei = RERANK_PROVIDERS.iter().find(|p| p.id == "tei").unwrap();
        assert!(tei.needs_url && !tei.needs_key && tei.country == "your hardware");
        // Countries are surfaced for the no-train decision, not decoration.
        assert!(RERANK_PROVIDERS.iter().all(|p| !p.country.is_empty()));
    }

    #[test]
    fn align_fills_gaps_bounds_checks_and_keeps_order() {
        let scored = vec![
            Scored {
                index: 2,
                score: 0.9,
            },
            Scored {
                index: 0,
                score: 0.1,
            },
            Scored {
                index: 99,
                score: 5.0,
            }, // out of the pool
            Scored {
                index: -1,
                score: 7.0,
            }, // below zero
        ];
        assert_eq!(align(&scored, 3), vec![0.1, 0.0, 0.9]);
        assert!(align(&[], 2).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn the_catalog_readers_speak_each_envelope() {
        let cohere = json!({ "models": [{ "name": "rerank-v3.5" }, { "name": null }, {}] });
        assert_eq!(cohere_catalog_ids(&cohere), vec!["rerank-v3.5"]);
        let together = json!([
            { "id": "Salesforce/Llama-Rank-V1", "type": "rerank" },
            { "id": "plain-llm", "type": "chat" },
            { "type": "rerank" }
        ]);
        assert_eq!(
            together_catalog_ids(&together),
            vec!["Salesforce/Llama-Rank-V1"]
        );
        let pinecone = json!({ "models": [{ "model": "bge-reranker-v2-m3" }, {}] });
        assert_eq!(pinecone_catalog_ids(&pinecone), vec!["bge-reranker-v2-m3"]);
        // OpenRouter: tagged modality OR an id that says rerank — case-blind.
        let or = json!({ "data": [
            { "id": "cohere/rerank-v3.5", "architecture": { "output_modalities": ["text"] } },
            { "id": "weird-model-x", "architecture": { "output_modalities": ["rerank"] } },
            { "id": "BAAI/BGE-RERANKER", "architecture": {} },
            { "architecture": { "output_modalities": ["rerank"] } }
        ]});
        assert_eq!(
            openrouter_catalog_ids(&or),
            vec!["cohere/rerank-v3.5", "weird-model-x", "BAAI/BGE-RERANKER"]
        );
    }

    #[test]
    fn a_config_round_trips_to_the_exact_ts_row_shape() {
        let cfg = RerankConfig {
            provider: "tei".into(),
            candidates: Some(30),
            ..Default::default()
        };
        let v = serde_json::to_value(&cfg).unwrap();
        // camelCase, absent fields omitted — the row the TS runtime reads.
        assert_eq!(v, json!({ "provider": "tei", "candidates": 30 }));
        let full = RerankConfig {
            provider: "voyage".into(),
            url: Some("https://tei.internal".into()),
            model: Some("rerank-2".into()),
            key_sealed: Some("v2:…".into()),
            candidates: Some(100),
        };
        let v = serde_json::to_value(&full).unwrap();
        assert_eq!(
            v,
            json!({ "provider": "voyage", "url": "https://tei.internal", "model": "rerank-2",
                    "keySealed": "v2:…", "candidates": 100 })
        );
        // And back.
        assert_eq!(serde_json::from_value::<RerankConfig>(v).unwrap(), full);
    }

    #[tokio::test]
    async fn rerank_off_returns_none_without_touching_anything() {
        let (http, seen) = http_scripted(vec![]);
        let state = lazy_state();
        // The lazy pool is never dialed: provider 'off' is answered before the
        // config read would matter… no — the config READ runs first; it fails
        // closed to defaults on a dead pool, which is provider 'off'.
        assert_eq!(rerank(&state, &http, "q", &["a".into()]).await, None);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tei_reranks_from_the_configured_url_with_a_plain_array_reply() {
        let (http, seen) = http_scripted(vec![(
            "https://tei.internal/rerank",
            200,
            r#"[{"index":1,"score":0.9},{"index":0,"score":0.2}]"#,
        )]);
        let state = lazy_state();
        // The config read hits the dead pool and falls back to defaults, so
        // drive the dispatch with an explicit config through the same match —
        // here that means pointing `cfg` at tei via the helper this test
        // exercises through `rerank`. Instead of a DB round trip, exercise
        // dispatch directly:
        let cfg = RerankConfig {
            provider: "tei".into(),
            url: Some("https://tei.internal/".into()),
            candidates: Some(30),
            ..Default::default()
        };
        let texts = vec!["first".to_string(), "second".to_string()];
        let got = dispatch_for_test(&state, &http, &cfg, "q", &texts).await;
        assert_eq!(got, Some(vec![0.2, 0.9]));
        let (url, body) = seen.lock().unwrap()[0].clone();
        assert_eq!(url, "https://tei.internal/rerank");
        assert_eq!(
            body,
            Some(json!({ "query": "q", "texts": texts, "truncate": true }))
        );
    }

    #[tokio::test]
    async fn a_keyed_provider_without_a_seal_or_a_url_answers_none() {
        let (http, seen) = http_scripted(vec![]);
        let state = lazy_state();
        // voyage with no key: None, no HTTP.
        let cfg = RerankConfig {
            provider: "voyage".into(),
            ..Default::default()
        };
        assert_eq!(
            dispatch_for_test(&state, &http, &cfg, "q", &["a".into()]).await,
            None
        );
        // tei with no url: None, no HTTP.
        let cfg = RerankConfig {
            provider: "tei".into(),
            ..Default::default()
        };
        assert_eq!(
            dispatch_for_test(&state, &http, &cfg, "q", &["a".into()]).await,
            None
        );
        // A sealed key that cannot open (no secretbox root in a test state)
        // reads as absent — the TS open() throws into the catch.
        let cfg = RerankConfig {
            provider: "voyage".into(),
            key_sealed: Some("v2:not-a-real-token".into()),
            ..Default::default()
        };
        assert_eq!(
            dispatch_for_test(&state, &http, &cfg, "q", &["a".into()]).await,
            None
        );
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn the_defaults_are_off_with_thirty_candidates() {
        assert_eq!(
            defaults(),
            RerankConfig {
                provider: "off".into(),
                candidates: Some(30),
                ..Default::default()
            }
        );
    }

    fn map_of(v: Value) -> serde_json::Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => unreachable!("the fixture is an object"),
        }
    }

    #[test]
    fn the_public_config_is_the_stored_row_in_its_own_order() {
        // jsonb's canonical order (shortest key first) is what TS reads back
        // and spreads — these bytes are the live dev row's, diffed against
        // the TS route.
        assert_eq!(
            public_of(
                json!({"model": "x", "provider": "tei", "candidates": 10, "keySealed": "v1:a:b:c"})
            )
            .to_string(),
            r#"{"model":"x","provider":"tei","candidates":10,"hasKey":true}"#
        );
        // No row at all: DEFAULTS plus hasKey false, in DEFAULTS' order.
        assert_eq!(
            public_of(defaults_value()).to_string(),
            r#"{"provider":"off","candidates":30,"hasKey":false}"#
        );
        // A null keySealed is as falsy as an absent one.
        assert_eq!(
            public_of(json!({"provider": "tei", "keySealed": null})).to_string(),
            r#"{"provider":"tei","hasKey":false}"#
        );
    }

    #[test]
    fn the_patch_folds_a_spread_not_a_reshape() {
        // Against DEFAULTS: an existing key keeps its position, a fresh key
        // appends, and the sealed key lands last. (`api_key` stays None in
        // the struct — the caller seals and hands the token in separately.)
        let mut next = map_of(defaults_value());
        apply_patch(
            &mut next,
            RerankPatch {
                provider: Some("tei".into()),
                url: None,
                model: Some(Some("x".into())),
                api_key: None,
                candidates: Some(10),
            },
            Some(Some("v1:a:b:c".into())),
        );
        assert_eq!(
            Value::Object(next).to_string(),
            r#"{"provider":"tei","candidates":10,"model":"x","keySealed":"v1:a:b:c"}"#
        );

        // Against a full row: a null url/apiKey REMOVES the keys outright
        // (`?? undefined`, dropped by JSON.stringify), and candidates clamps
        // to the TS bounds Math.min(100, Math.max(5, n)).
        let mut again = map_of(json!({
            "model": "x", "provider": "tei", "candidates": 10,
            "url": "http://tei.local", "keySealed": "v1:a:b:c",
        }));
        apply_patch(
            &mut again,
            RerankPatch {
                provider: None,
                url: Some(None),
                model: None,
                api_key: None,
                candidates: Some(500),
            },
            Some(None),
        );
        assert_eq!(
            Value::Object(again).to_string(),
            r#"{"model":"x","provider":"tei","candidates":100}"#
        );
    }

    #[test]
    fn the_provider_catalog_serializes_the_ts_wire_shape() {
        let all = providers_public();
        assert_eq!(all.len(), RERANK_PROVIDERS.len());
        // The first entry, byte for byte the object the TS table declares —
        // camelCase keys, declaration order, the empty list rides as [].
        assert_eq!(
            serde_json::to_value(&all[0]).unwrap(),
            json!({
                "id": "tei",
                "label": "Self-hosted (TEI)",
                "country": "your hardware",
                "needsUrl": true,
                "needsKey": false,
                "fallbackModels": [],
                "liveCatalog": false,
            })
        );
        // A catalog-carrying entry keeps its documented list verbatim.
        assert_eq!(
            serde_json::to_value(&all[1]).unwrap(),
            json!({
                "id": "openrouter",
                "label": "OpenRouter",
                "country": "US",
                "needsUrl": false,
                "needsKey": true,
                "fallbackModels": ["cohere/rerank-v3.5", "baai/bge-reranker-v2-m3"],
                "liveCatalog": true,
            })
        );
    }

    /// The rerank() body with the config read factored out, so tests drive
    /// dispatch without a live app_settings row. It is the SAME match — the
    /// test-only seam is the cfg parameter, and rerank() is its only other
    /// caller in spirit: rerank() = read cfg → this.
    async fn dispatch_for_test(
        state: &AppState,
        http: &HttpFetch,
        cfg: &RerankConfig,
        query: &str,
        texts: &[String],
    ) -> Option<Vec<f64>> {
        if texts.is_empty() {
            return None;
        }
        rerank_dispatch(state, http, cfg, query, texts)
            .await
            .map(|s| align(&s, texts.len()))
    }
}
