// Agent harness — the agent-definition WRITE core: identity alphabets,
// versioned configs, and the create path's row plumbing. Port of the
// definitions+versions half of ui/src/server/agent-defs.ts (the endpoint CRUD
// half stays TS until the Models admin routes cross; the read half that the
// relay needed already lives in gateway/registry.rs).
//
// A CONFIG IS DATA, NOT A CONTRACT: every signature here moves the version's
// `config` jsonb as a `serde_json::Value`, the way fleet.rs already reads it.
// A typed struct would silently DROP unknown keys on round-trip — TS spreads
// `{...prev}` and keeps whatever a future field adds — and the three things
// the write path does with a config (build it, compare it, store it) all
// want the tree, not a schema.
//
// One accepted divergence, stated once: TS object key order is insertion
// order, serde_json's is alphabetical. Nothing here can observe it —
// `canonical` sorts keys before comparing, and the config tree feeds a YAML
// emitter whose consumers parse mappings order-insensitively.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgPool;
use std::sync::OnceLock;

use crate::agent_auth::epoch_ms_to_iso;
use crate::gateway::registry::{LlmEndpoint, list_endpoints};

/// Agent slug / department alphabets (agent-defs.ts:13-14). `create_agent`
/// has always enforced these; they live HERE because every path that accepts
/// an agent identity needs the same bar — a slug reaches an .env line
/// (`HERMES_KEY_<SLUG>` — a `\n` injects a line), path joins (a `../`
/// traverses), and a regex interpolation.
pub fn slug_ok(slug: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9]{1,30}$").unwrap())
        .is_match(slug)
}

pub fn dept_ok(department: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9-]{1,40}$").unwrap())
        .is_match(department)
}

/// A model target: which endpoint serves it and the upstream model id there
/// (agent-defs.ts ModelTarget). `contextLength`/`effort` are ABSENT when
/// unset — TS's optional keys drop on serialize, and jsonb round-trips what
/// TS wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTarget {
    pub endpoint: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// One alias target — a ModelTarget with the tier's name on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasTarget {
    pub name: String,
    #[serde(flatten)]
    pub target: ModelTarget,
}

/// The structured edit a caller applies onto a previous config
/// (applyConfigEdits' second parameter).
#[derive(Debug, Clone)]
pub struct ConfigEdits {
    pub main: ModelTarget,
    pub aliases: Vec<AliasTarget>,
    pub fallbacks: Vec<ModelTarget>,
}

/// A version row, the columns the create path reads (agent-defs.ts
/// AgentVersion minus the timestamps nothing here touches).
#[derive(Debug, Clone)]
pub struct AgentVersionRow {
    pub id: String,
    pub agent_id: String,
    pub version: i32,
    pub soul: String,
    pub config: Value,
    pub note: Option<String>,
    pub created_by: Option<String>,
    /// ISO via epoch-ms — postgres.js hands TS a Date, json() stringifies it.
    pub created_at: String,
}

/// A def row, the columns the create path hands around (the run's HiredDef
/// and the renderer's RenderTarget grow out of this). `managed` is carried
/// because create sets it in a second statement and the caller must see the
/// set value, not the insert default.
#[derive(Debug, Clone)]
pub struct AgentDefRow {
    pub id: String,
    pub slug: String,
    pub department: String,
    pub model: String,
    pub display_name: String,
    pub role: Option<String>,
    pub source: String,
    pub managed: bool,
    pub current_version: i32,
}

/// The def row as SQL hands it back — one alias so every reader maps the
/// same nine columns through the same constructor.
type DefTuple = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    bool,
    i32,
);

fn def_row(
    (id, slug, department, model, display_name, role, source, managed, current_version): DefTuple,
) -> AgentDefRow {
    AgentDefRow {
        id,
        slug,
        department,
        model,
        display_name,
        role,
        source,
        managed,
        current_version,
    }
}

// Literal SQL, not format!: the column list appears in each statement so the
// query is auditable as written (sqlx's dynamic-SQL lint exists for this).
pub async fn agent_def_by_slug(
    pg: &PgPool,
    slug: &str,
) -> Result<Option<AgentDefRow>, sqlx::Error> {
    let row: Option<DefTuple> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, role, source, managed, current_version \
         from agent_defs where slug = $1",
    )
    .bind(slug)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(def_row))
}

pub async fn agent_def_by_id(pg: &PgPool, id: &str) -> Result<Option<AgentDefRow>, sqlx::Error> {
    let row: Option<DefTuple> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, role, source, managed, current_version \
         from agent_defs where id = $1::uuid",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(def_row))
}

type VersionTuple = (
    String,
    String,
    i32,
    String,
    Value,
    Option<String>,
    Option<String>,
    i64,
);

fn version_row(
    (id, agent_id, version, soul, config, note, created_by, created_ms): VersionTuple,
) -> AgentVersionRow {
    AgentVersionRow {
        id,
        agent_id,
        version,
        soul,
        config,
        note,
        created_by,
        created_at: epoch_ms_to_iso(created_ms),
    }
}

/// Versions newest-first (listVersions).
pub async fn list_versions(
    pg: &PgPool,
    agent_id: &str,
) -> Result<Vec<AgentVersionRow>, sqlx::Error> {
    let rows: Vec<VersionTuple> = sqlx::query_as(
        "select id::text, agent_id::text, version, soul, config, note, created_by, \
             (trunc(extract(epoch from created_at) * 1000))::bigint \
             from agent_versions where agent_id = $1::uuid order by version desc",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(version_row).collect())
}

/// Insert-or-refresh a def by slug (upsertAgentDef). The create flow reaches
/// this only after its exists pre-check, but the upsert — not a bare insert —
/// is the contract: a slug already present keeps its role unless a new one is
/// supplied (imports don't carry it).
pub async fn upsert_agent_def(
    pg: &PgPool,
    input: &UpsertDef<'_>,
) -> Result<AgentDefRow, sqlx::Error> {
    let model = format!("{}-{}", input.slug, input.department);
    let row: DefTuple = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, role, source) \
         values ($1, $2, $3, $4, $5, $6) \
         on conflict (slug) do update set \
           department = excluded.department, model = excluded.model, \
           display_name = excluded.display_name, \
           -- keep an existing role unless a new one is supplied (imports don't carry it) \
           role = coalesce(excluded.role, agent_defs.role), updated_at = now() \
         returning id::text, slug, department, model, display_name, role, source, managed, current_version",
    )
    .bind(input.slug)
    .bind(input.department)
    .bind(&model)
    .bind(input.display_name)
    .bind(input.role)
    .bind(input.source)
    .fetch_one(pg)
    .await?;
    Ok(def_row(row))
}

pub struct UpsertDef<'a> {
    pub slug: &'a str,
    pub department: &'a str,
    pub display_name: &'a str,
    pub role: Option<&'a str>,
    pub source: &'a str,
}

/// Key-order-insensitive stringify — Postgres jsonb reorders object keys, so
/// a plain `to_string` comparison would see every round-trip as a change
/// (agent-defs.ts canonical). ASCII keys sort identically under TS's UTF-16
/// code-unit compare and Rust's byte compare; config keys are ASCII.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical(&map[k.as_str()])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        leaf => serde_json::to_string(leaf).unwrap_or_else(|_| "null".into()),
    }
}

/// Structured edits applied onto a previous version's config
/// (applyConfigEdits). The raw Hermes config is rebuilt in the same stroke so
/// rendering reflects the edit: model/base_url/api_key come from the endpoint
/// registry; extra raw keys (e.g. context_length env refs) survive when the
/// endpoint is unchanged.
pub async fn apply_config_edits(
    pg: &PgPool,
    prev: &Value,
    edits: &ConfigEdits,
) -> Result<Value, String> {
    let endpoints = list_endpoints(pg).await.map_err(|e| e.to_string())?;
    apply_config_edits_over(&endpoints, prev, edits)
}

/// The pure core of apply_config_edits, over an already-loaded endpoint
/// registry — separated so the block-building rules are testable without a
/// database.
pub fn apply_config_edits_over(
    endpoints: &[LlmEndpoint],
    prev: &Value,
    edits: &ConfigEdits,
) -> Result<Value, String> {
    /// One raw model block: endpoint facts under the target's model, keeping
    /// the previous block's keys only when it pointed at the same place.
    fn block(ep: &LlmEndpoint, target: &ModelTarget, prev_block: Option<&Value>) -> Value {
        let prev_b = prev_block.and_then(Value::as_object);
        // `(prevB.provider === ep.provider) && (prevB.base_url ?? null) === (ep.baseUrl ?? null)`
        let same_place = prev_b
            .map(|b| {
                b.get("provider").and_then(Value::as_str) == Some(ep.provider.as_str())
                    && b.get("base_url").and_then(Value::as_str) == ep.base_url.as_deref()
            })
            .unwrap_or(false);
        let mut out: Map<String, Value> = if same_place {
            prev_b.cloned().unwrap_or_default()
        } else {
            Map::new()
        };
        out.insert("provider".into(), Value::String(ep.provider.clone()));
        out.insert("model".into(), Value::String(target.model.clone()));
        // TS truthiness on baseUrl: null and '' both skip the key.
        if let Some(base) = ep.base_url.as_deref().filter(|b| !b.is_empty()) {
            out.insert("base_url".into(), Value::String(base.into()));
        }
        if !same_place && let Some(env) = ep.api_key_env.as_deref().filter(|e| !e.is_empty()) {
            out.insert("api_key".into(), Value::String(format!("${{{env}}}")));
        }
        Value::Object(out)
    }

    let ep_of = |name: &str| -> Result<&LlmEndpoint, String> {
        endpoints
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| format!("unknown endpoint \"{name}\""))
    };

    let prev_raw = prev
        .get("raw")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let prev_aliases = prev_raw
        .get("model_aliases")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut raw = prev_raw;
    raw.insert(
        "model".into(),
        block(
            ep_of(&edits.main.endpoint)?,
            &edits.main,
            prev.get("raw").and_then(|r| r.get("model")),
        ),
    );
    let mut aliases = Map::new();
    for alias in &edits.aliases {
        aliases.insert(
            alias.name.clone(),
            block(
                ep_of(&alias.target.endpoint)?,
                &alias.target,
                prev_aliases.get(&alias.name),
            ),
        );
    }
    raw.insert("model_aliases".into(), Value::Object(aliases));
    let mut fallbacks: Vec<Value> = Vec::new();
    for f in &edits.fallbacks {
        fallbacks.push(block(ep_of(&f.endpoint)?, f, None));
    }
    raw.insert("fallback_providers".into(), Value::Array(fallbacks));

    // `{...prev, main, aliases, fallbacks, raw}` — the structured fields
    // replace, every other key the previous config carried survives.
    let mut out = prev.clone();
    if !out.is_object() {
        out = Value::Object(Map::new());
    }
    let obj = out.as_object_mut().unwrap();
    obj.insert("main".into(), serde_json::to_value(&edits.main).unwrap());
    obj.insert(
        "aliases".into(),
        serde_json::to_value(&edits.aliases).unwrap(),
    );
    obj.insert(
        "fallbacks".into(),
        serde_json::to_value(&edits.fallbacks).unwrap(),
    );
    obj.insert("raw".into(), Value::Object(raw));
    Ok(out)
}

pub struct NewVersion<'a> {
    pub soul: &'a str,
    pub config: &'a Value,
    pub note: Option<&'a str>,
    pub created_by: Option<&'a str>,
}

/// Append a new version (and point current_version at it). Skips when the
/// payload equals the latest version — safe to re-run imports, and the hire
/// run's resume depends on it (addVersionIfChanged).
pub async fn add_version_if_changed(
    pg: &PgPool,
    agent_id: &str,
    payload: &NewVersion<'_>,
) -> Result<(i32, bool), sqlx::Error> {
    let latest: Option<(i32, String, Value)> = sqlx::query_as(
        "select version, soul, config from agent_versions \
         where agent_id = $1::uuid order by version desc limit 1",
    )
    .bind(agent_id)
    .fetch_optional(pg)
    .await?;
    if let Some((version, soul, config)) = &latest
        && soul == payload.soul
        && canonical(config) == canonical(payload.config)
    {
        return Ok((*version, false));
    }
    let next = latest.as_ref().map(|(v, _, _)| *v + 1).unwrap_or(1);
    let mut tx = pg.begin().await?;
    sqlx::query(
        "insert into agent_versions (agent_id, version, soul, config, note, created_by) \
         values ($1::uuid, $2, $3, $4, $5, $6::text)",
    )
    .bind(agent_id)
    .bind(next)
    .bind(payload.soul)
    .bind(payload.config)
    .bind(payload.note)
    .bind(payload.created_by)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "update agent_defs set current_version = $1, updated_at = now() where id = $2::uuid",
    )
    .bind(next)
    .bind(agent_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((next, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    // No database in here: the alphabets, the canonical form, and the
    // block-building rules are pure; the row plumbing is thin sqlx over
    // queries copied from the TS, exercised later by the run's own tests.

    #[test]
    fn the_alphabets_admit_slugs_and_refuse_everything_else() {
        assert!(slug_ok("analyst"));
        assert!(slug_ok("a1"));
        assert!(!slug_ok("Analyst"));
        assert!(!slug_ok("a"));
        assert!(!slug_ok("a-b"));
        assert!(!slug_ok("-a"));
        assert!(!slug_ok("a\nB=1"));
        assert!(
            slug_ok(&"a".repeat(31)),
            "1 + 30 is the ceiling, and it is in"
        );
        assert!(!slug_ok(&"a".repeat(32)));
        assert!(dept_ok("research"));
        assert!(dept_ok("customer-success-2"));
        assert!(!dept_ok("Research"));
        assert!(!dept_ok("research_"));
        assert!(!dept_ok(&"d".repeat(42)));
    }

    #[test]
    fn canonical_sorts_keys_and_ignores_array_order() {
        let a: Value = serde_json::from_str(r#"{"b":1,"a":{"y":[1,2],"x":null}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":{"x":null,"y":[1,2]},"b":1}"#).unwrap();
        assert_eq!(canonical(&a), canonical(&b));
        let c: Value = serde_json::from_str(r#"{"a":{"y":[2,1],"x":null},"b":1}"#).unwrap();
        assert_ne!(canonical(&a), canonical(&c), "array order IS significant");
        // The exact form, pinned: sorted keys, no spaces — jsonb's reorder is
        // invisible, a value change never is.
        assert_eq!(canonical(&a), r#"{"a":{"x":null,"y":[1,2]},"b":1}"#);
    }

    fn ep(
        name: &str,
        provider: &str,
        base_url: Option<&str>,
        api_key_env: Option<&str>,
    ) -> LlmEndpoint {
        LlmEndpoint {
            id: format!("id-{name}"),
            name: name.into(),
            provider: provider.into(),
            base_url: base_url.map(String::from),
            class: "local".into(),
            api_key_env: api_key_env.map(String::from),
            models: vec![],
            price_in_per_mtok: None,
            price_out_per_mtok: None,
            model_prices: Value::Null,
            auto_prices: Value::Null,
            request_defaults: Value::Null,
            model_efforts: Value::Null,
            context_length: None,
        }
    }

    fn target(endpoint: &str, model: &str) -> ModelTarget {
        ModelTarget {
            endpoint: endpoint.into(),
            model: model.into(),
            context_length: None,
            effort: None,
        }
    }

    #[test]
    fn a_fresh_default_builds_the_raw_block_from_the_registry() {
        let endpoints = vec![
            ep(
                "ollama",
                "ollama",
                Some("http://127.0.0.1:11434"),
                Some("OLLAMA_API_KEY"),
            ),
            ep("openai", "openai", None, Some("LLM_API_KEY")),
        ];
        // The hire's platform-defaults call: prev is the same bare shape.
        let prev: Value = serde_json::from_str(
            r#"{"main":{"endpoint":"ollama","model":"m"},"aliases":[],"fallbacks":[]}"#,
        )
        .unwrap();
        let edits = ConfigEdits {
            main: target("ollama", "m"),
            aliases: vec![],
            fallbacks: vec![],
        };
        let out = apply_config_edits_over(&endpoints, &prev, &edits).unwrap();
        let raw = out.get("raw").unwrap();
        // no previous raw → the block is registry facts only, and the
        // api_key placeholder interpolates the endpoint's env name
        assert_eq!(
            raw.get("model"),
            Some(&serde_json::json!({"provider": "ollama", "model": "m",
                                     "base_url": "http://127.0.0.1:11434",
                                     "api_key": "${OLLAMA_API_KEY}"}))
        );
        assert_eq!(raw.get("model_aliases"), Some(&serde_json::json!({})));
        assert_eq!(raw.get("fallback_providers"), Some(&serde_json::json!([])));
        // the structured fields ride along camelCase
        assert_eq!(
            out.get("main"),
            Some(&serde_json::json!({"endpoint": "ollama", "model": "m"}))
        );
    }

    #[test]
    fn a_same_place_edit_keeps_the_previous_raw_keys() {
        let endpoints = vec![ep("ollama", "ollama", Some("http://127.0.0.1:11434"), None)];
        let prev: Value = serde_json::from_str(
            r#"{"main":{"endpoint":"ollama","model":"old"},
                "aliases":[{"name":"deep","endpoint":"ollama","model":"old"}],
                "fallbacks":[],
                "raw":{"model":{"provider":"ollama","base_url":"http://127.0.0.1:11434",
                                  "model":"old","context_length_env":"CTX"},
                        "model_aliases":{"deep":{"provider":"ollama","model":"old"}}}}"#,
        )
        .unwrap();
        let edits = ConfigEdits {
            main: target("ollama", "new"),
            aliases: vec![AliasTarget {
                name: "deep".into(),
                target: target("ollama", "new-deep"),
            }],
            fallbacks: vec![],
        };
        let out = apply_config_edits_over(&endpoints, &prev, &edits).unwrap();
        let model = out["raw"]["model"].as_object().unwrap();
        // same provider+base_url → the extra key survives, the model swaps
        assert_eq!(
            model.get("context_length_env"),
            Some(&serde_json::json!("CTX"))
        );
        assert_eq!(model.get("model"), Some(&serde_json::json!("new")));
        assert!(
            !model.contains_key("api_key"),
            "no placeholder added on a same-place edit"
        );
        assert_eq!(
            out["raw"]["model_aliases"]["deep"]["model"],
            serde_json::json!("new-deep")
        );
    }

    #[test]
    fn an_unknown_endpoint_is_the_one_edit_error() {
        let out = apply_config_edits_over(
            &[],
            &Value::Null,
            &ConfigEdits {
                main: target("nowhere", "m"),
                aliases: vec![],
                fallbacks: vec![],
            },
        );
        assert_eq!(out.unwrap_err(), "unknown endpoint \"nowhere\"");
    }
}
