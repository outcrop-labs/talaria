// Capability keys and effort defaults for a FLEET PERSONA — port of
// harness/persona.ts. A persona ("assistant-operations") is not a catalog
// model: `routing_for` answers nothing for it. It is BACKED by real
// endpoint:upstream targets recorded in its agent version's config, and this
// module is that mapping — the pool a call on the persona could land on, and
// the effort its config names.
//
// It never guesses. Every ambiguity resolves to NO KEYS: a half-written
// target, a missing main, a tier the agent does not have. The two-pass claim
// order (base ids first) is load-bearing and pinned in the tests.

use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One enabled agent's current version as read from the database. `config` is
/// raw jsonb that outlives the code that wrote it — a malformed row must land
/// every caller on the unknown path, never panic.
pub struct PersonaRow {
    /// `agent_defs.model` — the routable base id.
    pub agent: String,
    pub config: serde_json::Value,
}

/// The half of agent-defs' ModelTarget that names a capability key. `effort`
/// rides the same records but is read by `persona_effort_index`, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTarget {
    pub endpoint: String,
    pub model: String,
}

fn as_record(raw: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    raw.as_object()
}

/// A stored target is only a target when it names BOTH halves of a capability
/// key. A half-written one (endpoint set, model blank) would produce a key
/// some other half-written row could also produce, and the two would silently
/// pool each other's facts.
fn read_target(raw: &serde_json::Value) -> Option<ModelTarget> {
    let t = as_record(raw)?;
    let endpoint = t.get("endpoint").and_then(|v| v.as_str())?.trim();
    let model = t.get("model").and_then(|v| v.as_str())?.trim();
    if endpoint.is_empty() || model.is_empty() {
        return None;
    }
    Some(ModelTarget {
        endpoint: endpoint.to_string(),
        model: model.to_string(),
    })
}

fn read_target_list(raw: &serde_json::Value) -> Vec<ModelTarget> {
    raw.as_array()
        .map(|a| a.iter().filter_map(read_target).collect())
        .unwrap_or_default()
}

/// The pool a call on this persona-and-tier could actually land on: the
/// tier's own target first, then the agent's fallbacks. THE FALLBACKS BELONG
/// IN THE POOL — the runner moves to them when the primary errors, so "which
/// model answers" is not knowable in advance, and including them only ever
/// withholds (never grants) a capability verdict. The safe direction.
fn pool_for(primary: ModelTarget, fallbacks: &[ModelTarget]) -> Vec<ModelTarget> {
    let mut pool = Vec::with_capacity(1 + fallbacks.len());
    pool.push(primary);
    pool.extend(fallbacks.iter().cloned());
    pool
}

struct Parsed {
    agent: String,
    main: Option<ModelTarget>,
    aliases: Vec<serde_json::Value>,
    fallbacks: Vec<ModelTarget>,
}

fn parse_rows(rows: &[PersonaRow]) -> Vec<Parsed> {
    rows.iter()
        .map(|row| {
            let config = as_record(&row.config);
            Parsed {
                agent: row.agent.clone(),
                main: config.and_then(|c| c.get("main")).and_then(read_target),
                aliases: config
                    .and_then(|c| c.get("aliases"))
                    .and_then(|a| a.as_array())
                    .cloned()
                    .unwrap_or_default(),
                fallbacks: config
                    .and_then(|c| c.get("fallbacks"))
                    .map(read_target_list)
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// Every routable persona id → the targets a call on it could land on.
///
/// Two passes, and the order is load-bearing: base ids are claimed first so an
/// agent literally named "x-opus" (its own config) wins over the reading of
/// that string as the "opus" tier of "x". NO MAIN, NO KEYS — never fall back
/// to the fallbacks, which only serve when the primary has already failed.
pub fn persona_index(rows: &[PersonaRow]) -> HashMap<String, Vec<ModelTarget>> {
    let parsed = parse_rows(rows);
    let mut by_id = HashMap::new();
    for row in &parsed {
        if let Some(main) = &row.main {
            by_id.insert(row.agent.clone(), pool_for(main.clone(), &row.fallbacks));
        }
    }
    for row in &parsed {
        for raw in &row.aliases {
            let alias = as_record(raw);
            let name = alias
                .and_then(|a| a.get("name"))
                .and_then(|n| n.as_str())
                .map(str::trim)
                .unwrap_or("");
            let Some(target) = read_target(raw) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let id = format!("{}-{}", row.agent, name);
            // An agent's own id outranks another's tier.
            if by_id.contains_key(&id) {
                continue;
            }
            by_id.insert(id, pool_for(target, &row.fallbacks));
        }
    }
    by_id
}

/// The configured effort off one stored target record, or None. Same posture
/// as `read_target`: a missing or malformed field is no default, never a guess.
fn read_effort(raw: &serde_json::Value) -> Option<String> {
    let e = as_record(raw)?.get("effort")?.as_str()?.trim();
    (!e.is_empty()).then(|| e.to_string())
}

/// The CONFIGURED default effort per routable persona id (base and tiers).
/// Same two-pass claim order as `persona_index`. Ids with no configured
/// effort are absent from the map.
pub fn persona_effort_index(rows: &[PersonaRow]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let parsed: Vec<(String, bool, Option<String>, Vec<serde_json::Value>)> = rows
        .iter()
        .map(|row| {
            let config = as_record(&row.config);
            let main = config.and_then(|c| c.get("main"));
            (
                row.agent.clone(),
                main.and_then(read_target).is_some(),
                main.and_then(read_effort),
                config
                    .and_then(|c| c.get("aliases"))
                    .and_then(|a| a.as_array())
                    .cloned()
                    .unwrap_or_default(),
            )
        })
        .collect();
    for (agent, has_main, main_effort, _) in &parsed {
        if *has_main && main_effort.is_some() {
            out.insert(agent.clone(), main_effort.clone().unwrap());
        }
    }
    for (agent, _, _, aliases) in &parsed {
        for raw in aliases {
            let alias = as_record(raw);
            let name = alias
                .and_then(|a| a.get("name"))
                .and_then(|n| n.as_str())
                .map(str::trim)
                .unwrap_or("");
            if name.is_empty() {
                continue;
            }
            if read_target(raw).is_none() {
                continue;
            }
            let id = format!("{agent}-{name}");
            if out.contains_key(&id) {
                continue;
            }
            if let Some(effort) = read_effort(raw) {
                out.insert(id, effort);
            }
        }
    }
    out
}

// ── The cached database read ─────────────────────────────────────────────────

/// Long enough that a busy install reads the table roughly once a minute;
/// short enough that an admin re-pointing an agent sees records follow within
/// a minute. Nothing here is authoritative — a stale entry costs one run of a
/// narrower prompt.
const TTL: Duration = Duration::from_secs(60);
/// A failed read is retried far sooner than a good one is refreshed, but not
/// on every ask: a database that is down would otherwise be hit once per
/// question by a subsystem whose answer is "no keys" either way.
const RETRY: Duration = Duration::from_secs(5);

struct Snapshot {
    at: Instant,
    ok: bool,
    by_id: HashMap<String, Vec<ModelTarget>>,
    efforts: HashMap<String, String>,
}

fn snapshot_cell() -> &'static Mutex<Option<Arc<Snapshot>>> {
    static CELL: OnceLock<Mutex<Option<Arc<Snapshot>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

fn inflight() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// THIS NEVER FAILS. Resolving a persona is a lookup that makes a run BETTER,
/// not a precondition for one; a database blip must not turn a working harness
/// into a failure. A failed read is an empty index — exactly the state a
/// fresh self-host is in anyway.
async fn load(pg: &PgPool) -> Snapshot {
    let rows: Result<Vec<(String, serde_json::Value)>, _> = sqlx::query_as(
        "select d.model as agent, v.config as config \
         from agent_defs d \
         join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.enabled",
    )
    .fetch_all(pg)
    .await;
    match rows {
        Ok(rows) => {
            let rows: Vec<PersonaRow> = rows
                .into_iter()
                .map(|(agent, config)| PersonaRow { agent, config })
                .collect();
            Snapshot {
                at: Instant::now(),
                ok: true,
                by_id: persona_index(&rows),
                efforts: persona_effort_index(&rows),
            }
        }
        Err(_) => Snapshot {
            at: Instant::now(),
            ok: false,
            by_id: HashMap::new(),
            efforts: HashMap::new(),
        },
    }
}

async fn index(pg: &PgPool) -> Arc<Snapshot> {
    let fresh = |s: &Snapshot| s.at.elapsed() < if s.ok { TTL } else { RETRY };
    if let Ok(cell) = snapshot_cell().lock()
        && let Some(s) = cell.as_ref()
        && fresh(s)
    {
        return s.clone();
    }
    // One load in flight: concurrent askers share it (the inflight map's
    // dedup), and the re-check after the guard covers whoever queued first.
    let _guard = inflight().lock().await;
    if let Ok(cell) = snapshot_cell().lock()
        && let Some(s) = cell.as_ref()
        && fresh(s)
    {
        return s.clone();
    }
    let snap = Arc::new(load(pg).await);
    if let Ok(mut cell) = snapshot_cell().lock() {
        *cell = Some(snap.clone());
    }
    snap
}

/// The TARGETS behind one routable persona id (tier included). Empty when the
/// id is not a persona, its config is missing or malformed, or it names a tier
/// the agent does not have — an unresolvable tier returns nothing rather than
/// falling back to the agent's main, because inheriting the wrong model's
/// metadata is worse than inheriting none.
pub async fn persona_targets_for(pg: &PgPool, model: &str) -> Vec<ModelTarget> {
    let snap = index(pg).await;
    snap.by_id.get(model).cloned().unwrap_or_default()
}

/// The runner's default `persona_keys` edge: the capability keys a fleet
/// persona inherits from the model actually serving it — the pool behind the
/// id (tier target plus the agent's fallbacks), folded to endpoint:upstream
/// keys and deduped in pool order. Empty for anything that is not a live
/// persona, which leaves the caller on the unknown path.
pub async fn persona_capability_keys(pg: &PgPool, model: &str) -> Vec<String> {
    let snap = index(pg).await;
    let mut seen: Vec<String> = Vec::new();
    for t in snap.by_id.get(model).into_iter().flatten() {
        let key = crate::capability::capability_key(&t.endpoint, &t.model);
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen
}

/// The agent-configured default reasoning effort for a routable persona id
/// (base or tier), or None. The answer is the CONFIGURED string, unvalidated;
/// the caller holds it against the model's live published levels, which is
/// where staleness is decided.
pub async fn persona_configured_effort(pg: &PgPool, model: &str) -> Option<String> {
    let snap = index(pg).await;
    snap.efforts.get(model).cloned()
}

/// Drop the cached index — for tests.
pub fn clear_persona_cache() {
    if let Ok(mut cell) = snapshot_cell().lock() {
        *cell = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(agent: &str, config: serde_json::Value) -> PersonaRow {
        PersonaRow {
            agent: agent.into(),
            config,
        }
    }

    fn target(ep: &str, m: &str) -> ModelTarget {
        ModelTarget {
            endpoint: ep.into(),
            model: m.into(),
        }
    }

    #[test]
    fn base_ids_claim_first_and_main_is_required() {
        let rows = vec![
            row(
                "x",
                json!({"main": {"endpoint": "a", "model": "m"}, "fallbacks": [{"endpoint": "b", "model": "f"}]}),
            ),
            // An agent literally named "x-opus": its own id must beat "x"'s
            // "opus" tier.
            row("x-opus", json!({"main": {"endpoint": "c", "model": "own"}})),
            // No main: NO keys — never the fallbacks.
            row("y", json!({"fallbacks": [{"endpoint": "b", "model": "f"}]})),
            // Malformed config entirely: unknown path.
            row("z", json!("nonsense")),
        ];
        let idx = persona_index(&rows);
        assert_eq!(idx.get("x").unwrap(), &[target("a", "m"), target("b", "f")]);
        assert_eq!(idx.get("x-opus").unwrap(), &[target("c", "own")]);
        assert!(!idx.contains_key("y"));
        assert!(!idx.contains_key("z"));
    }

    #[test]
    fn tiers_pool_their_target_behind_the_agents_fallbacks() {
        let rows = vec![row(
            "x",
            json!({
                "main": {"endpoint": "a", "model": "m"},
                "aliases": [{"name": "opus", "endpoint": "a", "model": "big"}],
                "fallbacks": [{"endpoint": "b", "model": "f"}]
            }),
        )];
        let idx = persona_index(&rows);
        assert_eq!(
            idx.get("x-opus").unwrap(),
            &[target("a", "big"), target("b", "f")]
        );
        // A tier the agent does not have: nothing, not main.
        assert!(!idx.contains_key("x-turbo"));
        // A blank or half-written alias is no tier.
        let rows = vec![row(
            "x",
            json!({
                "main": {"endpoint": "a", "model": "m"},
                "aliases": [{"name": "  ", "endpoint": "a", "model": "big"}, {"name": "ok", "endpoint": "a"}]
            }),
        )];
        let idx = persona_index(&rows);
        assert!(!idx.contains_key("x-"));
        assert!(!idx.contains_key("x-ok"));
    }

    #[test]
    fn efforts_follow_the_same_two_pass_claim() {
        let rows = vec![
            row(
                "x",
                json!({
                    "main": {"endpoint": "a", "model": "m", "effort": "high"},
                    "aliases": [{"name": "opus", "endpoint": "a", "model": "big", "effort": "max"}]
                }),
            ),
            // Main with no effort: base id absent from the map, tier still read.
            row(
                "w",
                json!({
                    "main": {"endpoint": "a", "model": "m"},
                    "aliases": [{"name": "lite", "endpoint": "a", "model": "sm", "effort": "low"}]
                }),
            ),
            // No main: the BASE id claims nothing (hasMain gates it), but the
            // tier pass has no such gate in TS — an alias with a valid target
            // and a configured effort claims its tier id even when the agent's
            // main is unset or malformed.
            row(
                "v",
                json!({"aliases": [{"name": "opus", "endpoint": "a", "model": "big", "effort": "high"}]}),
            ),
        ];
        let eff = persona_effort_index(&rows);
        assert_eq!(eff.get("x").map(String::as_str), Some("high"));
        assert_eq!(eff.get("x-opus").map(String::as_str), Some("max"));
        assert!(!eff.contains_key("w"));
        assert_eq!(eff.get("w-lite").map(String::as_str), Some("low"));
        assert!(!eff.contains_key("v"));
        assert_eq!(eff.get("v-opus").map(String::as_str), Some("high"));
    }

    #[test]
    fn blank_effort_is_no_effort_and_whitespace_trims() {
        let rows = vec![row(
            "x",
            json!({"main": {"endpoint": "a", "model": "m", "effort": "   "}}),
        )];
        assert!(persona_effort_index(&rows).is_empty());
        let rows = vec![row(
            "x",
            json!({"main": {"endpoint": " a ", "model": " m ", "effort": " high "}}),
        )];
        let eff = persona_effort_index(&rows);
        assert_eq!(eff.get("x").map(String::as_str), Some("high"));
        assert_eq!(persona_index(&rows).get("x").unwrap(), &[target("a", "m")]);
    }
}
