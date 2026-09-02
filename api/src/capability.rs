// Model capability records — the read half, the catalog refresh's ranked
// merge, and the forget button. A stored
// fact is data, never a verdict: UNKNOWN IS NOT FALSE is the whole contract,
// and `missing_capabilities` is the only question the model-identity plane
// asks (role-assignment fitness). The probe/gateway writers live with the
// surfaces that own them.
//
// The precedence rule is the point: a catalog
// refresh is a daily cadence across every model an endpoint serves, and under
// last-write-wins it would erase a probe result with marketing copy. Rank:
// probe > declared > catalog > learned; equal rank still wins (a re-probe can
// correct a probe).

use crate::gateway::settings::{get_setting, set_setting};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const KEY: &str = "model_capabilities";

/// `endpoint:model` — capability is a property of the ENDPOINT serving the
/// model, not of the model name. One id behind two providers genuinely differs
/// in what it can hold.
pub fn capability_key(endpoint: &str, upstream_model: &str) -> String {
    format!("{endpoint}:{upstream_model}")
}

/// The capability union, in declaration order.
pub const ALL_CAPABILITIES: [&str; 9] = [
    "json",
    "json-strict",
    "tools",
    "tool-select",
    "search",
    "vision",
    "long-context",
    "code",
    "instruction-following",
];

const ALL_SOURCES: [&str; 4] = ["probe", "declared", "catalog", "learned"];

fn source_rank(source: &str) -> i64 {
    match source {
        "probe" => 3,
        "declared" => 2,
        "catalog" => 1,
        _ => 0,
    }
}

/// May `next` replace `prev`? Yes when nothing is there, when it is at least
/// as authoritative, or when what is there has expired.
pub fn outranks(next: &str, prev: Option<&str>) -> bool {
    match prev {
        None => true,
        Some(p) => source_rank(next) >= source_rank(p),
    }
}

/// Learned and catalog facts expire (30 days); probe and declared facts do
/// not. An unparseable `at` counts as expired rather than eternal — dropping a
/// re-derivable fact costs one re-discovery.
const LEARNED_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// ms since the epoch for a stored ISO string, None when unparseable — the
/// parse accepts exactly the ISO shape the house formatter writes (its
/// inverse, no clock crate); anything else is expired.
fn parse_iso_ms(at: &str) -> Option<u64> {
    crate::agent_auth::iso_to_epoch_ms(at).map(|ms| ms.max(0) as u64)
}

/// One stored fact, parsed defensively. `app_settings` is JSON that outlives
/// the code that wrote it — a hand-edited row or a shape from a previous
/// version must land the caller on the unknown path, never panic. The parsed
/// view keeps the wire shape (value, source, at, detail?, score?) so the merge
/// writes back what it read.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityFact {
    pub value: bool,
    pub source: String,
    pub at: String,
    pub detail: Option<String>,
    pub score: Option<f64>,
}

impl CapabilityFact {
    fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("value".into(), serde_json::Value::Bool(self.value));
        o.insert(
            "source".into(),
            serde_json::Value::String(self.source.clone()),
        );
        o.insert("at".into(), serde_json::Value::String(self.at.clone()));
        if let Some(d) = &self.detail {
            o.insert("detail".into(), serde_json::Value::String(d.clone()));
        }
        if let Some(s) = self.score {
            o.insert(
                "score".into(),
                serde_json::Number::from_f64(s)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        serde_json::Value::Object(o)
    }

    fn is_expired(&self, now: u64) -> bool {
        if self.source != "learned" && self.source != "catalog" {
            return false;
        }
        match parse_iso_ms(&self.at) {
            Some(at) => now.saturating_sub(at) > LEARNED_TTL_MS,
            None => true,
        }
    }
}

/// Anything that isn't a well-formed, unexpired fact is not a fact.
pub fn read_fact(raw: &serde_json::Value, now: u64) -> Option<CapabilityFact> {
    let f = raw.as_object()?;
    let value = f.get("value")?.as_bool()?;
    let source = f.get("source")?.as_str()?;
    if !ALL_SOURCES.contains(&source) {
        return None;
    }
    let at = f.get("at")?.as_str()?.to_string();
    let fact = CapabilityFact {
        value,
        source: source.to_string(),
        at,
        detail: f.get("detail").and_then(|d| d.as_str()).map(String::from),
        score: f.get("score").and_then(|s| s.as_f64()),
    };
    (!fact.is_expired(now)).then_some(fact)
}

/// The stored entry for one key, parsed. Unknown capability ids (a newer
/// build's union) are dropped from THIS view but preserved on write.
fn entry_of<'a>(
    all: &'a serde_json::Value,
    key: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    all.get(key).and_then(|v| v.as_object())
}

/// Everything currently known about this endpoint:model, expired facts
/// filtered out so no caller thinks about the TTL. Unrecognized capability
/// ids are skipped (a rolling deploy's newer build may know more than we do).
pub async fn get_capabilities(pg: &PgPool, key: &str) -> HashMap<String, CapabilityFact> {
    let all = get_setting(pg, KEY, serde_json::json!({})).await;
    let now = now_ms();
    let mut out = HashMap::new();
    if let Some(entry) = entry_of(&all, key) {
        for (cap, raw) in entry {
            if !ALL_CAPABILITIES.contains(&cap.as_str()) {
                continue;
            }
            if let Some(fact) = read_fact(raw, now) {
                out.insert(cap.clone(), fact);
            }
        }
    }
    out
}

/// The capabilities known to be MISSING (a recorded `value: false`).
/// UNKNOWN IS NOT FALSE: only a fact that positively says no counts.
pub async fn missing_capabilities(pg: &PgPool, key: &str, required: &[&str]) -> Vec<String> {
    let facts = get_capabilities(pg, key).await;
    required
        .iter()
        .filter(|cap| facts.get(**cap).map(|f| !f.value).unwrap_or(false))
        .map(|cap| cap.to_string())
        .collect()
}

/// Serialize writes in process: every write
/// here is a read-modify-write of ONE app_settings row, and unserialized
/// concurrent writes lose facts outright. The cross-process race is accepted
/// (same writers, different processes). A tokio mutex (not std) because the
/// guard is held across the awaited settings read/write — an std guard there
/// would make every caller's future !Send.
fn write_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// MANY FACTS, MANY KEYS, ONE WRITE, respecting SOURCE_RANK — the merge the
/// catalog refresh uses. A weaker source never displaces a stronger one;
/// returns how many facts were actually taken.
pub async fn merge_capabilities(
    pg: &PgPool,
    batch: &[(String, Vec<(String, CapabilityFact)>)],
) -> Result<usize, sqlx::Error> {
    if batch.is_empty() {
        return Ok(0);
    }
    let _guard = write_lock().lock().await;
    let mut all = get_setting(pg, KEY, serde_json::json!({})).await;
    let now = now_ms();
    let mut written = 0;
    for (key, facts) in batch {
        // Clone the entry, merge into it, put it back — unknown capability ids
        // (a newer build's) and other keys ride through untouched.
        let mut entry = entry_of(&all, key).cloned().unwrap_or_default();
        for (cap, fact) in facts {
            if !ALL_CAPABILITIES.contains(&cap.as_str()) {
                continue;
            }
            let prev = entry.get(cap).and_then(|raw| read_fact(raw, now));
            if !outranks(&fact.source, prev.as_ref().map(|p| p.source.as_str())) {
                continue;
            }
            entry.insert(cap.clone(), fact.to_json());
            written += 1;
        }
        if let Some(obj) = all.as_object_mut() {
            obj.insert(key.clone(), serde_json::Value::Object(entry));
        }
    }
    set_setting(pg, KEY, &all).await?;
    Ok(written)
}

/// THE FORGET BUTTON. Drops one key's whole entry — every capability, every
/// source — and leaves every other key's facts untouched. Nothing recorded
/// under the key is not a churn: the row is not rewritten at all, so a forget
/// on a fresh install is free and a forget racing a probe write cannot lose
/// the probe (same write lock, one read-modify-write either way).
pub async fn forget_capabilities(pg: &PgPool, key: &str) -> Result<(), sqlx::Error> {
    let _guard = write_lock().lock().await;
    let mut all = get_setting(pg, KEY, serde_json::json!({})).await;
    if !all.get(key).is_some() {
        return Ok(());
    }
    if let Some(obj) = all.as_object_mut() {
        obj.remove(key);
    }
    set_setting(pg, KEY, &all).await?;
    Ok(())
}

/// Tests pin the stored JSON shape and the rank/TTL arithmetic.
#[cfg(test)]
mod tests {
    use super::*;

    fn fact(source: &str, value: bool, at: &str) -> CapabilityFact {
        CapabilityFact {
            value,
            source: source.into(),
            at: at.into(),
            detail: Some("d".into()),
            score: None,
        }
    }

    #[test]
    fn read_fact_validates_shape_and_source() {
        let now = 1_800_000_000_000;
        // Well-formed and unexpired (probe never expires).
        assert!(read_fact(
            &serde_json::json!({"value": false, "source": "probe", "at": "2020-01-01T00:00:00.000Z"}),
            now
        )
        .is_some());
        // Missing value / wrong type / unknown source / non-object: not facts.
        for bad in [
            serde_json::json!({"source": "probe", "at": "x"}),
            serde_json::json!({"value": "yes", "source": "probe", "at": "x"}),
            serde_json::json!({"value": true, "source": "vibe", "at": "x"}),
            serde_json::json!({"value": true, "source": "probe"}),
            serde_json::json!(null),
            serde_json::json!([1]),
        ] {
            assert!(read_fact(&bad, now).is_none(), "{bad:?}");
        }
        // A learned fact older than the TTL is gone; one fresh stays.
        let old = "2020-01-01T00:00:00.000Z";
        let fresh = "2100-01-01T00:00:00.000Z";
        assert!(
            read_fact(
                &serde_json::json!({"value": true, "source": "learned", "at": old}),
                now
            )
            .is_none()
        );
        assert!(
            read_fact(
                &serde_json::json!({"value": true, "source": "learned", "at": fresh}),
                now
            )
            .is_some()
        );
        // An unparseable `at` on an expiring source is expired.
        assert!(
            read_fact(
                &serde_json::json!({"value": true, "source": "catalog", "at": "nonsense"}),
                now
            )
            .is_none()
        );
    }

    #[test]
    fn rank_gates_replacement_and_equal_wins() {
        assert!(outranks("catalog", None));
        assert!(outranks("probe", Some("declared")));
        assert!(outranks("declared", Some("declared")));
        assert!(!outranks("catalog", Some("declared")));
        assert!(!outranks("learned", Some("catalog")));
    }

    #[test]
    fn fact_json_keeps_wire_order_and_optionals() {
        let f = fact("catalog", true, "2026-01-01T00:00:00.000Z");
        let s = f.to_json().to_string();
        assert_eq!(
            s,
            "{\"value\":true,\"source\":\"catalog\",\"at\":\"2026-01-01T00:00:00.000Z\",\"detail\":\"d\"}"
        );
        // score omitted when absent, present when set.
        assert!(!s.contains("score"));
        let mut g = f.clone();
        g.detail = None;
        g.score = Some(0.5);
        assert_eq!(
            g.to_json().to_string(),
            "{\"value\":true,\"source\":\"catalog\",\"at\":\"2026-01-01T00:00:00.000Z\",\"score\":0.5}"
        );
    }

    #[test]
    fn ttl_constant_is_thirty_days() {
        assert_eq!(LEARNED_TTL_MS, 30 * 24 * 60 * 60 * 1000);
    }
}
