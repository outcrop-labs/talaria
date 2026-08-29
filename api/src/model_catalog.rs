// WHAT THE PROVIDER ALREADY TOLD US ABOUT EACH MODEL, cached and read back —
// port of model-catalog.ts (read half + the one refresh the effort backfill
// needs). It is a CACHE, not a source of truth: provider.rs does the fetching
// and stays the only thing that talks to a provider; this module decides how
// long an answer is good for, where it lives, and what a capability reader
// may conclude from it. A stale entry is served while a refresh is in flight,
// and a failed refresh serves the last good answer — a catalog blip must not
// empty the model picker.
//
// ── THE ONE RULE THAT MATTERS ────────────────────────────────────────────────
// A CATALOG MAY SAY YES. IT MAY NEVER SAY NO.
//
// `supported_parameters` listing `tools` is a provider committing to
// something, and Talaria records it as a catalog fact. NOT listing `tools` is
// not a denial — it is a field a hundred OpenAI-compatible servers never
// populate at all, plus vLLM, Ollama, LM Studio and every self-host that
// answers `/models` with `{id}` and nothing else. Absence stays unknown,
// capability.rs's cardinal rule (UNKNOWN IS NOT FALSE) keeps the model
// running, and a probe is what turns unknown into a no.
//
// `advertised_window` and the full `refresh_catalogs` sweep port with the
// probe suite and the admin refresh button (the surfaces that own them); the
// effort ladder's needs stop at what is here.

use crate::capability::{CapabilityFact, capability_key, merge_capabilities};
use crate::gateway::provider::CatalogModel;
use crate::gateway::registry::LlmEndpoint;
use crate::gateway::settings::{get_setting, set_setting};
use sqlx::PgPool;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub const KEY: &str = "model_catalog";

/// A CATALOG IS NOT VOLATILE, and a day is the honest number. Providers add
/// models continuously and change a model's advertised window almost never,
/// so a shorter TTL buys nothing but request volume against four hundred rows.
pub const CATALOG_TTL_MS: u64 = 24 * 60 * 60 * 1000;

// ── Reading ──────────────────────────────────────────────────────────────────

/// Everything known about one model id, across every endpoint that serves it
/// — port of catalogEntriesFor. MORE THAN ONE ENTRY IS NORMAL and is why this
/// returns a list: a bare id can land on any endpoint in the pool, and the
/// two can differ in exactly the way that matters (a quantized local build
/// next to the vendor's own API). Callers reduce it themselves.
pub async fn catalog_entries_for(pg: &PgPool, model: &str) -> Vec<(String, CatalogModel)> {
    entries_in_store(&read_store(pg).await, model)
}

/// The pure core of `catalog_entries_for`, over an already-read store.
///
/// BOTH SPELLINGS, and the endpoint-qualified one is why this exists at all.
/// A catalog is keyed by the id the PROVIDER publishes — OpenRouter says
/// `deepseek/deepseek-v4-flash` — while everything that asks about a model
/// says `openrouter/deepseek/deepseek-v4-flash`, the one spelling the picker
/// offers. The prefix is stripped only for the endpoint whose name it is, so
/// a bare id still searches every catalog (the pooled case) and a qualified
/// id can never match another endpoint's entry.
pub fn entries_in_store(store: &serde_json::Value, model: &str) -> Vec<(String, CatalogModel)> {
    let mut out = Vec::new();
    let Some(obj) = store.as_object() else {
        return out;
    };
    for (name, entry) in obj {
        let models = entry.get("models").and_then(|m| m.as_array());
        let Some(models) = models else { continue };
        let prefix = format!("{name}/");
        let upstream = model.strip_prefix(&prefix).unwrap_or(model);
        for raw in models {
            let Some(m) = CatalogModel::from_json(raw) else {
                continue;
            };
            if m.id == upstream {
                out.push((name.clone(), m));
                break;
            }
        }
    }
    out
}

/// Catalog entries for EXPLICIT endpoint:upstream pairs — the persona path.
/// A fleet persona id is not a catalog id, so it cannot be looked up the way
/// `entries_in_store` looks one up; its agent config names the exact targets
/// (persona.rs), and this answers the catalog rows for those targets and
/// nothing else: prefix-stripping here would be guessing at what the config
/// already knows.
pub async fn catalog_entries_for_targets(
    pg: &PgPool,
    targets: &[crate::persona::ModelTarget],
) -> Vec<(String, CatalogModel)> {
    targets_in_store(&read_store(pg).await, targets)
}

pub fn targets_in_store(
    store: &serde_json::Value,
    targets: &[crate::persona::ModelTarget],
) -> Vec<(String, CatalogModel)> {
    let mut out = Vec::new();
    let obj = store.as_object();
    for t in targets {
        let Some(entry) = obj.and_then(|o| o.get(&t.endpoint)) else {
            continue;
        };
        let Some(models) = entry.get("models").and_then(|m| m.as_array()) else {
            continue;
        };
        for raw in models {
            let Some(m) = CatalogModel::from_json(raw) else {
                continue;
            };
            if m.id == t.model {
                out.push((t.endpoint.clone(), m));
                break;
            }
        }
    }
    out
}

async fn read_store(pg: &PgPool) -> serde_json::Value {
    get_setting(pg, KEY, serde_json::json!({})).await
}

// ── Reasoning effort ─────────────────────────────────────────────────────────

/// The effort ladder's display order, weakest to strongest. Levels a provider
/// coins that this list does not know still show — appended after the known
/// ones, in the provider's own order — because the provider's spelling is the
/// contract and the picker must never rename a level into one the model
/// rejects.
pub const EFFORT_ORDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

pub fn ordered_efforts(levels: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for known in EFFORT_ORDER {
        if levels.iter().any(|l| l == known) && seen.insert(known.to_string()) {
            out.push(known.to_string());
        }
    }
    for l in levels {
        if !EFFORT_ORDER.contains(&l.as_str()) && seen.insert(l.clone()) {
            out.push(l.clone());
        }
    }
    out
}

/// THE EFFORT LEVELS A POOL AGREES ON — port of effortLevelsOf.
///
/// Same posture as a minimum window: a bare id (and a persona behind
/// fallbacks) can land on any member of the pool, so a level is only offered
/// when every member that speaks at all accepts it. Members that say nothing
/// do not veto — UNKNOWN IS NOT FALSE — they are skipped, exactly as an
/// endpoint without a published window does not shrink the minimum. An empty
/// answer means "nobody vouched for any level" and the picker stays hidden,
/// which is the honest rendering of an unmeasured model.
pub fn effort_levels_of(entries: &[(String, CatalogModel)]) -> Vec<String> {
    let lists: Vec<Vec<String>> = entries
        .iter()
        .filter_map(|(_, m)| m.efforts.as_ref().filter(|l| !l.is_empty()).cloned())
        .collect();
    let Some((first, rest)) = lists.split_first() else {
        return Vec::new();
    };
    let mut shared = first.clone();
    for l in rest {
        shared.retain(|x| l.contains(x));
    }
    ordered_efforts(&shared)
}

// ── Deriving capabilities ────────────────────────────────────────────────────

/// Which advertised parameter proves which capability. Only entries whose
/// PRESENCE is a real commitment belong here — see the file header on why
/// absence proves nothing.
const PARAM_PROVES: [(&str, &str, &str); 4] = [
    (
        "tools",
        "tools",
        "the provider catalog advertises tool calling",
    ),
    (
        "response_format",
        "json",
        "the provider catalog advertises response_format",
    ),
    (
        "structured_outputs",
        "json-strict",
        "the provider catalog advertises structured outputs",
    ),
    (
        "web_search_options",
        "search",
        "the provider catalog advertises native web search",
    ),
];

/// WHAT A CATALOG ENTRY PROVES, and nothing more — port of
/// capabilitiesFromCatalog. Pure, exported, and tested directly, because this
/// function is where the yes-only rule either holds or quietly stops holding.
/// Every branch below pushes a `true` or pushes NOTHING. There is no path
/// that writes `false`.
pub fn capabilities_from_catalog(m: &CatalogModel, at: &str) -> Vec<(String, CapabilityFact)> {
    let mut facts: Vec<(String, CapabilityFact)> = Vec::new();
    let declare = |facts: &mut Vec<(String, CapabilityFact)>, cap: &str, detail: &str| {
        // `source: "catalog"`, never "declared". "declared" means a HUMAN said
        // so and outranks a provider's own claim — see capability.rs. Writing
        // these as "declared" would let a nightly refresh overrule an admin.
        facts.push((
            cap.to_string(),
            CapabilityFact {
                value: true,
                source: "catalog".to_string(),
                at: at.to_string(),
                detail: Some(detail.to_string()),
                score: None,
            },
        ));
    };
    if let Some(params) = &m.supported_parameters {
        for (param, cap, detail) in PARAM_PROVES {
            if params.iter().any(|p| p == param) {
                declare(&mut facts, cap, detail);
            }
        }
        // `tool_choice` without `tools` is not a thing any catalog publishes,
        // but a model that accepts it can certainly be handed tools.
        if params.iter().any(|p| p == "tool_choice") && !facts.iter().any(|(cap, _)| cap == "tools")
        {
            declare(
                &mut facts,
                "tools",
                "the provider catalog advertises tool_choice",
            );
        }
    }
    // VISION IS THE ONE PLACE ABSENCE IS ALMOST INFORMATIVE and still is not
    // recorded as a no: a catalog that lists `input_modalities: ['text']` is
    // describing the model precisely, but plenty of catalogs list nothing at
    // all, and one shape must not be read as the other. A probe closes it.
    if m.input_modalities
        .as_ref()
        .is_some_and(|mods| mods.iter().any(|x| x == "image"))
    {
        declare(
            &mut facts,
            "vision",
            "the provider catalog advertises image input",
        );
    }
    // `long-context` IS DELIBERATELY NOT DECLARED HERE. `context_length` is
    // the only catalog field that looks like a capability and is not one: it
    // says what the model will ACCEPT, and the probe measures whether the
    // model can still find a fact planted in the middle of that window.
    // Advertising 1M tokens and retrieving from 1M tokens are different
    // claims, and the gap between them is exactly what an admin is trying to
    // find out. The window is not wasted — `advertised_window` (probe suite,
    // later batch) serves it to the probe, which sizes the measurement.
    facts
}

// ── Refreshing ───────────────────────────────────────────────────────────────

pub struct RefreshResult {
    pub endpoint: String,
    pub models: usize,
    /// Capability facts written across all of this endpoint's models.
    pub facts: usize,
    pub error: Option<String>,
}

/// Re-fetch one endpoint's catalog, store it, and declare what it proves —
/// port of refreshEndpointCatalog.
///
/// A FAILED FETCH KEEPS THE OLD ENTRY. An endpoint that is briefly
/// unreachable must not empty the picker or un-declare facts an admin is
/// looking at; the error is reported and the last good catalog stays.
pub async fn refresh_endpoint_catalog(
    state: &crate::state::AppState,
    ep: &LlmEndpoint,
) -> RefreshResult {
    // `new Date().toISOString()` — millisecond precision, Z.
    let at = crate::agent_auth::epoch_ms_to_iso(now_ms() as i64);
    let models = match crate::gateway::provider::catalog_models(state, ep).await {
        Ok(m) => m,
        Err(err) => {
            return RefreshResult {
                endpoint: ep.name.clone(),
                models: 0,
                facts: 0,
                error: Some(err),
            };
        }
    };

    let mut store = read_store(&state.pg).await;
    let entry = serde_json::json!({
        "endpoint": ep.name,
        "at": at,
        "models": models.iter().map(CatalogModel::to_json).collect::<Vec<_>>(),
    });
    if let Some(obj) = store.as_object_mut() {
        // Preserve-order insert: an existing key keeps its position (the TS
        // spread's behavior), a new one appends.
        obj.insert(ep.name.clone(), entry);
    }
    if set_setting(&state.pg, KEY, &store).await.is_err() {
        // TS throws here and the caller decides; the fetch succeeded but the
        // last good catalog stays until the next attempt.
        return RefreshResult {
            endpoint: ep.name.clone(),
            models: 0,
            facts: 0,
            error: Some("could not store the refreshed catalog".to_string()),
        };
    }

    // Per endpoint:model, exactly as every other capability writer keys it — a
    // fact learned about one endpoint's build is never credited to another's.
    // The whole endpoint goes in ONE merge: four hundred separate
    // read-modify-writes of a row that grows with each one is not a thing to
    // do on a cadence.
    let batch: Vec<(String, Vec<(String, CapabilityFact)>)> = models
        .iter()
        .map(|m| {
            (
                capability_key(&ep.name, &m.id),
                capabilities_from_catalog(m, &at),
            )
        })
        .filter(|(_, facts)| !facts.is_empty())
        .collect();
    let facts = merge_capabilities(&state.pg, &batch).await.unwrap_or(0);
    RefreshResult {
        endpoint: ep.name.clone(),
        models: models.len(),
        facts,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(id: &str, efforts: Option<Vec<&str>>) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("id".into(), json!(id));
        o.insert("name".into(), json!(null));
        o.insert("contextLength".into(), json!(null));
        o.insert("inputModalities".into(), json!(null));
        o.insert("supportedParameters".into(), json!(null));
        o.insert(
            "efforts".into(),
            match efforts {
                None => json!(null),
                Some(l) => json!(l),
            },
        );
        o.insert("pricing".into(), json!(null));
        serde_json::Value::Object(o)
    }

    fn store(cats: &[(&str, &[serde_json::Value])]) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        for (name, models) in cats {
            o.insert(
                (*name).to_string(),
                json!({"endpoint": name, "at": "2026-01-01T00:00:00.000Z", "models": models}),
            );
        }
        serde_json::Value::Object(o)
    }

    #[test]
    fn qualified_ids_match_only_their_own_endpoint() {
        let s = store(&[
            ("openrouter", &[model("deepseek/v4", None)]),
            ("local", &[model("deepseek/v4", None)]),
        ]);
        // Qualified: only openrouter's entry, prefix stripped.
        let hits = entries_in_store(&s, "openrouter/deepseek/v4");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "openrouter");
        assert_eq!(hits[0].1.id, "deepseek/v4");
        // Bare: every catalog that serves it.
        assert_eq!(entries_in_store(&s, "deepseek/v4").len(), 2);
        // A qualified id must not match another endpoint's entry, even when
        // the vendor prefix looks like one: "local/deepseek/v4" as an
        // OPENROUTGER catalog id is matched only by that literal id.
        assert_eq!(entries_in_store(&s, "local/deepseek/v4").len(), 1);
        assert!(entries_in_store(&s, "nowhere/x").is_empty());
    }

    #[test]
    fn the_pool_intersects_and_silent_members_do_not_veto() {
        let e = |ep: &str, efforts: Option<Vec<&str>>| {
            (
                ep.to_string(),
                CatalogModel::from_json(&model("m", efforts)).unwrap(),
            )
        };
        // Both speak: intersection.
        let levels = effort_levels_of(&[
            e("a", Some(vec!["low", "high"])),
            e("b", Some(vec!["high", "max"])),
        ]);
        assert_eq!(levels, vec!["high".to_string()]);
        // A silent member does not veto.
        let levels = effort_levels_of(&[e("a", Some(vec!["low"])), e("b", None)]);
        assert_eq!(levels, vec!["low".to_string()]);
        // Nobody speaks: empty.
        assert!(effort_levels_of(&[e("a", None), e("b", None)]).is_empty());
        assert!(effort_levels_of(&[]).is_empty());
        // Unknown levels append in provider order after the known ones.
        let levels = effort_levels_of(&[e("a", Some(vec!["turbo", "high", "eco"]))]);
        assert_eq!(
            levels,
            vec!["high".to_string(), "turbo".to_string(), "eco".to_string()]
        );
    }

    #[test]
    fn a_catalog_may_say_yes_never_no() {
        let with_params = |params: Option<Vec<&str>>, mods: Option<Vec<&str>>| CatalogModel {
            id: "m".into(),
            name: None,
            context_length: Some(1_048_576.0),
            input_modalities: mods.map(|m| m.iter().map(|s| s.to_string()).collect()),
            supported_parameters: params.map(|p| p.iter().map(|s| s.to_string()).collect()),
            efforts: None,
            pricing: None,
        };
        let at = "2026-01-01T00:00:00.000Z";
        // Silence writes NOTHING — not one false.
        let facts = capabilities_from_catalog(&with_params(None, None), at);
        assert!(facts.is_empty());
        let facts = capabilities_from_catalog(&with_params(Some(vec![]), Some(vec!["text"])), at);
        assert!(facts.is_empty());
        // Each advertised param proves exactly its capability.
        let facts = capabilities_from_catalog(
            &with_params(
                Some(vec![
                    "tools",
                    "response_format",
                    "structured_outputs",
                    "web_search_options",
                ]),
                Some(vec!["text", "image"]),
            ),
            at,
        );
        let caps: Vec<&str> = facts.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(
            caps,
            vec!["tools", "json", "json-strict", "search", "vision"]
        );
        assert!(facts.iter().all(|(_, f)| f.value && f.source == "catalog"));
        // tool_choice alone proves tools; long-context is never declared.
        let facts = capabilities_from_catalog(&with_params(Some(vec!["tool_choice"]), None), at);
        let caps: Vec<&str> = facts.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(caps, vec!["tools"]);
        // tools already present: tool_choice does not double-declare.
        let facts =
            capabilities_from_catalog(&with_params(Some(vec!["tools", "tool_choice"]), None), at);
        assert_eq!(facts.len(), 1);
    }
}
