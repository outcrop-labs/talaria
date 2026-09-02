// Which reasoning-effort levels a model id may be asked for.
//
// THE ONE QUESTION THIS FILE ANSWERS, asked by two surfaces and enforced by
// two routes: "may the composer offer an effort picker for THIS id, and which
// levels may it list?" Two voices can vouch, and only these two: the
// per-model metadata the catalog refresh already extracts and stores
// (model::catalog, filled when an admin adds models on /models) — the
// provider's own `supported_efforts` — and an admin's declaration
// (`llm_endpoints.model_efforts`, edited on the endpoint modal) for providers
// whose catalog says nothing. A model neither voice has published levels for
// answers `[]`, and `[]` means the picker does not render and the request
// body carries no effort, which is the whole feature contract: effort is
// offered only where somebody who can know — the provider, or the endpoint's
// own operator — vouches for it.
//
// TWO SPELLINGS OF A MODEL ID, both real, both arriving here: a catalog id
// (bare or endpoint-qualified, pooled across every endpoint that serves it)
// and a FLEET PERSONA id ("dex-ops", "dex-ops-opus" for a tier) — not a
// catalog id at all, but a pointer at the endpoint:upstream targets its agent
// config names. Resolved here, once, so no caller re-derives it and the two
// spellings cannot drift apart.

use crate::gateway::provider::CatalogModel;
use crate::gateway::registry::{LlmEndpoint, list_endpoints};
use crate::model::catalog::{
    KEY, catalog_entries_for, catalog_entries_for_targets, effort_levels_of, entries_in_store,
};
use crate::persona::{ModelTarget, persona_targets_for};
use crate::state::AppState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// THE SECOND VOICE THAT CAN VOUCH.
/// An admin's declaration REPLACES the catalog's ladder for that endpoint's
/// build of the model — a human's word outranks a provider's, the same
/// standing a declared capability fact has over a catalog one — and never
/// merges with it, because a union would offer levels one of the two voices
/// never vouched for.
///
/// Defensive on purpose: the column is admin-typed JSON that outlives the
/// build that wrote it, and a malformed entry must degrade to the catalog's
/// answer rather than crash a chat turn's validation. Array, non-empty, every
/// element a non-empty string — or Nothing.
pub fn declared_levels(raw: &serde_json::Value) -> Option<Vec<String>> {
    let arr = raw.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(arr.len());
    for x in arr {
        let s = x.as_str()?;
        if s.is_empty() {
            return None;
        }
        out.push(s.to_string());
    }
    Some(out)
}

/// The declared ladder stands in for the catalog's, per pool member. The POOL
/// RULE is untouched: a declaration speaks for its endpoint's member only,
/// and `effort_levels_of` still intersects across members, so a level is
/// offered only where every member that speaks at all accepts it.
pub fn with_declared_efforts(
    entries: Vec<(String, CatalogModel)>,
    roster: &[LlmEndpoint],
) -> Vec<(String, CatalogModel)> {
    entries
        .into_iter()
        .map(|(endpoint, mut model)| {
            let declared = roster
                .iter()
                .find(|e| e.name == endpoint)
                .map(|e| &e.model_efforts)
                .and_then(|v| v.get(&model.id))
                .and_then(declared_levels);
            if let Some(levels) = declared {
                model.efforts = Some(levels);
            }
            (endpoint, model)
        })
        .collect()
}

/// The effort levels THIS model id supports, or `[]` when nothing vouches for
/// any. Never fails: an unreadable catalog answers the same `[]` a fresh
/// self-host does, and a chat turn must not fail because a picker could not
/// decide whether to appear.
pub async fn efforts_for_model(pg: &sqlx::PgPool, model: &str) -> Vec<String> {
    // A persona resolves to explicit targets; everything else is a catalog id.
    // `persona_targets_for` answers [] for ids it does not know, which is also
    // the cached-cheap path for the gateway spelling.
    let targets = persona_targets_for(pg, model).await;
    let entries = if targets.is_empty() {
        catalog_entries_for(pg, model).await
    } else {
        catalog_entries_for_targets(pg, &targets).await
    };
    if entries.is_empty() {
        return Vec::new();
    }
    let roster = list_endpoints(pg).await.unwrap_or_default();
    effort_levels_of(&with_declared_efforts(entries, &roster))
}

// ── The backfill ─────────────────────────────────────────────────────────────
//
// THE STALE-CATALOG PROBLEM, which is why a picker shipped and did not appear:
// the ONLY production writer of the stored catalog is the model-adder modal
// (`fleet.endpoints.$id.available`), so a deployment's catalog is only ever as
// new as the last time an admin opened it. A catalog written by a build before
// the effort extraction has no `efforts` field on any model — every question
// answers `[]`, the picker never renders, and nothing an admin short of
// re-opening the model modal heals it.
//
// `ensure_efforts_catalog` is that healing: for the endpoints serving the
// asked model, any stored catalog whose models predate the field is refreshed
// live, ONCE — a refresh written by this build stamps `efforts` (null or a
// list) on every model, so the pre-feature shape never re-triggers. A failed
// refresh (provider unreachable) retries no more than every five minutes per
// endpoint.

/// Endpoints whose stored catalog predates the effort extraction: models
/// present, none carrying the `efforts` key. A refresh written by the current
/// build always writes the key (null included), so this is exactly the set
/// worth re-fetching. RAW JSON on purpose — the parsed model drops the
/// absent-key / present-null distinction this check exists to see.
fn pre_efforts_entry(store: &serde_json::Value, endpoint: &str) -> bool {
    let Some(entry) = store.as_object().and_then(|o| o.get(endpoint)) else {
        return false;
    };
    let Some(models) = entry.get("models").and_then(|m| m.as_array()) else {
        return false;
    };
    if models.is_empty() {
        return false;
    }
    models
        .iter()
        .all(|m| m.as_object().is_some_and(|o| !o.contains_key("efforts")))
}

/// One refresh attempt per endpoint per window; a provider that is down must
/// not turn every picker question into a 10-second timeout.
const RETRY_MS: u64 = 5 * 60_000;

fn attempted_at() -> &'static Mutex<HashMap<String, u64>> {
    static M: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per-endpoint refresh guards, so two surfaces asking about the same model
/// share one live fetch rather than racing two.
fn inflight() -> &'static Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>> {
    static M: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drop the backfill's bookkeeping. For tests, which share one module
/// instance across cases.
pub fn reset_efforts_backfill() {
    attempted_at().lock().unwrap().clear();
    inflight().lock().unwrap().clear();
}

/// Run (or join) one endpoint's refresh. The throttle is re-checked INSIDE the
/// per-endpoint guard: whoever stamps first runs, and callers that queued on
/// the guard see the fresh stamp and stand down.
async fn refresh_throttled(state: &AppState, ep: &LlmEndpoint) {
    let guard = {
        let mut map = inflight().lock().unwrap();
        map.entry(ep.name.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _held = guard.lock().await;
    if now_ms().saturating_sub(
        attempted_at()
            .lock()
            .unwrap()
            .get(&ep.name)
            .copied()
            .unwrap_or(0),
    ) < RETRY_MS
    {
        return;
    }
    attempted_at()
        .lock()
        .unwrap()
        .insert(ep.name.clone(), now_ms());
    // Best-effort by construction: a failed fetch keeps the old entry, and the
    // throttle decides when to try again.
    let _ = crate::model::catalog::refresh_endpoint_catalog(state, ep).await;
}

/// Refresh the pre-efforts catalogs behind this model and answer with the
/// levels the refreshed store now vouches for. Safe to call on every empty
/// read: post-feature entries (efforts present, even null) and unknown models
/// fetch nothing, so the live call happens once per endpoint per install (or
/// per five minutes while its provider is unreachable).
pub async fn ensure_efforts_catalog(state: &AppState, model: &str) -> Vec<String> {
    let pg = &state.pg;
    let targets: Vec<ModelTarget> = persona_targets_for(pg, model).await;
    let store = crate::gateway::settings::get_setting(pg, KEY, serde_json::json!({})).await;
    // The endpoints that could serve this id: a persona's own targets, or the
    // pool a catalog id lands on. Same resolution rule as the read above,
    // first-seen order preserved.
    let mut names: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let source: Vec<String> = if !targets.is_empty() {
        targets.into_iter().map(|t| t.endpoint).collect()
    } else {
        entries_in_store(&store, model)
            .into_iter()
            .map(|(ep, _)| ep)
            .collect()
    };
    for name in source {
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }
    let now = now_ms();
    let stale: Vec<String> = {
        let attempted = attempted_at().lock().unwrap();
        names
            .into_iter()
            .filter(|name| {
                pre_efforts_entry(&store, name)
                    && now.saturating_sub(attempted.get(name).copied().unwrap_or(0)) >= RETRY_MS
            })
            .collect()
    };
    if !stale.is_empty() {
        let roster = list_endpoints(pg).await.unwrap_or_default();
        for name in stale {
            let Some(ep) = roster.iter().find(|e| e.name == name) else {
                continue;
            };
            refresh_throttled(state, ep).await;
        }
    }
    efforts_for_model(pg, model).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store(cats: &[(&str, Vec<serde_json::Value>)]) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        for (name, models) in cats {
            o.insert(
                (*name).to_string(),
                json!({"endpoint": name, "at": "2026-01-01T00:00:00.000Z", "models": models}),
            );
        }
        serde_json::Value::Object(o)
    }

    fn raw_model(efforts: Option<serde_json::Value>) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("id".into(), json!("m"));
        o.insert("name".into(), json!(null));
        o.insert("contextLength".into(), json!(null));
        o.insert("inputModalities".into(), json!(null));
        o.insert("supportedParameters".into(), json!(null));
        match efforts {
            Some(v) => o.insert("efforts".into(), v),
            None => None,
        };
        o.insert("pricing".into(), json!(null));
        serde_json::Value::Object(o)
    }

    #[test]
    fn pre_efforts_means_absent_not_null() {
        let pre = store(&[("a", vec![raw_model(None), raw_model(None)])]);
        let post = store(&[(
            "a",
            vec![
                raw_model(Some(json!(null))),
                raw_model(Some(json!(["low"]))),
            ],
        )]);
        let mixed = store(&[("a", vec![raw_model(None), raw_model(Some(json!(null)))])]);
        let empty = store(&[("a", vec![])]);
        assert!(pre_efforts_entry(&pre, "a"));
        // Post-feature rows carry the key, even when null — never re-trigger.
        assert!(!pre_efforts_entry(&post, "a"));
        assert!(!pre_efforts_entry(&mixed, "a"));
        assert!(!pre_efforts_entry(&empty, "a"));
        assert!(!pre_efforts_entry(&pre, "missing"));
    }

    #[test]
    fn a_declaration_replaces_and_malformed_degrades() {
        // Valid: replaces the catalog's ladder wholesale.
        assert_eq!(
            declared_levels(&json!(["low", "high"])),
            Some(vec!["low".to_string(), "high".to_string()])
        );
        // Malformed shapes are no declaration: the catalog's answer stands.
        assert_eq!(declared_levels(&json!([])), None);
        assert_eq!(declared_levels(&json!(["low", ""])), None);
        assert_eq!(declared_levels(&json!(["low", 3])), None);
        assert_eq!(declared_levels(&json!("low")), None);
        assert_eq!(declared_levels(&json!(null)), None);
    }

    #[test]
    fn declarations_speak_for_their_member_only() {
        let model = |efforts: Option<Vec<&str>>| CatalogModel {
            id: "m".into(),
            name: None,
            context_length: None,
            input_modalities: None,
            supported_parameters: None,
            efforts: efforts.map(|l| l.iter().map(|s| s.to_string()).collect()),
            pricing: None,
        };
        let ep = |name: &str, declared: serde_json::Value| LlmEndpoint {
            context_length: None,
            id: name.into(),
            name: name.into(),
            provider: "openai".into(),
            base_url: None,
            class: "cloud".into(),
            api_key_env: None,
            models: vec!["m".into()],
            price_in_per_mtok: None,
            price_out_per_mtok: None,
            model_prices: serde_json::Value::Null,
            auto_prices: serde_json::Value::Null,
            request_defaults: serde_json::Value::Null,
            model_efforts: declared,
        };
        // Two members: one catalog voice, one declaration. Intersected — the
        // union is never offered.
        let entries = vec![
            ("a".to_string(), model(Some(vec!["low", "high"]))),
            ("b".to_string(), model(Some(vec!["medium"]))),
        ];
        let roster = vec![ep("b", json!({"m": ["low", "medium", "high"]}))];
        let merged = with_declared_efforts(entries, &roster);
        assert_eq!(
            effort_levels_of(&merged),
            vec!["low".to_string(), "high".to_string()]
        );
        // A malformed declaration leaves the catalog voice untouched.
        let entries = vec![("a".to_string(), model(Some(vec!["low"])))];
        let roster = vec![ep("a", json!({"m": ["", "high"]}))];
        let merged = with_declared_efforts(entries, &roster);
        assert_eq!(effort_levels_of(&merged), vec!["low".to_string()]);
    }
}
