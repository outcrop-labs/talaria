// The gateway's parameter learner — port of ui/src/server/harness/gateway-params.ts
// plus the I/O half that lives in llm-gateway.ts (the store under
// `gateway_unsupported_params`, the pre-strip pass, the capability fact a
// contract drop records).
//
// The learner reads an upstream 400 ("`temperature` is deprecated",
// "Unsupported parameter: 'top_p'"), strips the named parameter, retries, and
// remembers per endpoint:model. Model specs rot; the provider is always
// current. What the classification decides is WHICH parameters may be
// forgotten:
//
//   cosmetic  — removing it changes how good the answer is. Strip and forget.
//   contract  — removing it changes the SHAPE of what comes back. Still strip
//               (a completed call beats a 400) but never in silence: the drop
//               is reported to the caller and recorded as a capability fact.
//   protected — never strip. model/messages/stream: stripping either of the
//               first two is nonsense, and dropping `stream` yields a valid
//               single JSON body fed to an SSE pump that hangs forever.
//
// Learnings carry a 30-day TTL — a one-way ratchet could never re-discover a
// provider that fixed support.

use crate::gateway::settings::{get_setting, set_setting};
use regex::Regex;
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── ISO-8601 ↔ epoch ms ──────────────────────────────────────────────────────
// JS Date.parse/toISOString on exactly the shape we store (always Z, always
// 3-digit ms when we wrote it). Anything else parses as None, which the
// callers treat exactly like Date.parse's NaN: corrupt, drop it.

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn iso_to_epoch_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let mut i = 19;
    let mut frac_ms = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let digits = s.get(start..i)?;
        if digits.is_empty() {
            return None;
        }
        let mut take = digits.len().min(3);
        frac_ms = digits[..take].parse().ok()?;
        while take < 3 {
            frac_ms *= 10;
            take += 1;
        }
    }
    if b.get(i) != Some(&b'Z') {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((((days * 24 + hour) * 60 + min) * 60 + sec) * 1000 + frac_ms)
}

/// `new Date(ms).toISOString()` — always 4-digit year, 3-digit milliseconds.
pub fn epoch_to_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    let sod = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{frac:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

// ── Classification ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamClass {
    Cosmetic,
    Contract,
    Protected,
}

/// contract parameter -> the capability its rejection proves absent.
pub fn contract_capability(param: &str) -> Option<&'static str> {
    Some(match param {
        "response_format" | "guided_json" | "guided_regex" | "guided_choice"
        | "response_schema" => "json",
        "tools" | "tool_choice" | "parallel_tool_calls" | "functions" | "function_call" => "tools",
        _ => return None,
    })
}

/// Everything unrecognized classifies as cosmetic, deliberately — that is the
/// behavior that has worked for every tunable since the learner shipped, and
/// the blast radius of being wrong is bounded (quality, not shape).
pub fn classify_param(param: &str) -> ParamClass {
    if matches!(param, "model" | "messages" | "stream") {
        ParamClass::Protected
    } else if contract_capability(param).is_some() {
        ParamClass::Contract
    } else {
        ParamClass::Cosmetic
    }
}

// ── Reading the rejection out of a 400 ───────────────────────────────────────

/// The parameter an upstream 400 is complaining about, or None. Three
/// phrasings in production (Anthropic's compat layer, OpenAI's own, the loose
/// middle ground) plus the provider-authoritative `"param": "..."` field —
/// kept narrow on purpose: a pattern that matched more would strip parameters
/// merely MENTIONED in an error about something else. Port of rejectedParam,
/// patterns verbatim and in order.
pub fn rejected_param(err_text: &str) -> Option<String> {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [
            // The provider's own answer before any prose — authoritative where
            // every pattern below is a guess at English. Dotted paths reduce
            // to the root: what we can stop sending is `response_format`, not
            // `response_format.json_schema.strict`.
            r#"(?i)"param"\s*:\s*"([a-z_]+)(?:\.[A-Za-z0-9_]+)*""#,
            r#"(?i)[`"']([a-z_]+)[`"'] is (?:deprecated|not supported|unsupported)"#,
            r#"(?i)unsupported parameter[:\s`"']+([a-z_]+)"#,
            // OpenAI's other two phrasings — a parameter the ratchet cannot
            // NAME is one it never stops sending, so the endpoint 400s on
            // every call forever.
            r#"(?i)unrecognized request argument supplied[:\s`"']+([a-z_]+)"#,
            r#"(?i)unknown parameter[:\s`"']+([a-z_]+)"#,
            // The value complaint — a PARAMETER complaint wearing different
            // words ("Unsupported value: 'temperature' does not support 0.2
            // with this model").
            r#"(?i)unsupported value[:\s]*[`"']([a-z_]+)[`"']"#,
            r#"(?i)[`"']([a-z_]+)[`"'][^.]{0,40}(?:deprecated|not supported)"#,
            // The field-path shape (Anthropic validation errors:
            // "response_format.type: Input should be 'json_schema'"). Root of
            // the path only; classify_param still refuses protected ones.
            r#"(?i)(?:^|[\s"'{,])([a-z_]{3,})(?:\.[A-Za-z0-9_]+)*\s*:\s*(?:Input should be|Extra inputs are not permitted|Empty schema|Field required)"#,
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    });
    for re in res {
        if let Some(c) = re.captures(err_text)
            && let Some(m) = c.get(1)
        {
            return Some(m.as_str().to_string());
        }
    }
    None
}

// ── The persisted learned store ──────────────────────────────────────────────

pub const LEARNED_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const SETTINGS_KEY: &str = "gateway_unsupported_params";

/// endpoint:model -> parameter -> epoch ms learned. (TS calls this
/// LearnedParamMap; the param maps are HashMaps, so serialization order of the
/// stored blob differs from TS's insertion order — irrelevant, it is read back
/// by key.)
pub type LearnedMap = HashMap<String, HashMap<String, i64>>;

/// Parse the stored value, tolerating both shapes (the predecessor stored
/// `Record<string, string[]>`) and any hand-edited garbage — this runs on the
/// path to every upstream call, and anything unrecognized is dropped rather
/// than thrown, which costs at most one re-learned 400. Returns the map and
/// whether the read normalized something (legacy stamp, expiry, corruption)
/// that the caller should write back once.
pub fn read_learned_params(raw: &Value, now: i64) -> (LearnedMap, bool) {
    let mut by_key = LearnedMap::new();
    let mut changed = false;
    let Value::Object(obj) = raw else {
        return (by_key, changed);
    };

    for (key, entry) in obj {
        let mut params: HashMap<String, i64> = HashMap::new();
        match entry {
            Value::Array(list) => {
                // Legacy: bare names, no timestamps. Adopt at `now` — the only
                // honest reading, and persisting it is what starts the clock.
                for p in list.iter().filter_map(|v| v.as_str()) {
                    if !p.is_empty() {
                        params.insert(p.to_string(), now);
                    }
                }
                changed = true;
            }
            Value::Object(map) => {
                for (param, at) in map {
                    // Only a string parses (TS: typeof at === 'string' ?
                    // Date.parse : NaN) — a number here is a corrupt entry,
                    // not the legacy shape, and re-stamping it fresh would
                    // grant an unknown writer a permanent strip. Drop.
                    let ms = at.as_str().and_then(iso_to_epoch_ms);
                    match ms {
                        None => changed = true,
                        Some(ms) if now - ms > LEARNED_TTL_MS => changed = true,
                        Some(ms) => {
                            params.insert(param.clone(), ms);
                        }
                    }
                }
            }
            _ => {
                changed = true;
                continue;
            }
        }
        if !params.is_empty() {
            by_key.insert(key.clone(), params);
        } else {
            changed = true;
        }
    }
    (by_key, changed)
}

/// Serialize for `app_settings`. Expired entries drop on the way out, so a
/// store written for any reason is also a store pruned.
pub fn write_learned_params(by_key: &LearnedMap, now: i64) -> Value {
    let mut out = Map::new();
    for (key, params) in by_key {
        let mut entry = Map::new();
        for (param, at) in params {
            if now - at > LEARNED_TTL_MS {
                continue;
            }
            entry.insert(param.clone(), Value::String(epoch_to_iso(*at)));
        }
        if !entry.is_empty() {
            out.insert(key.clone(), Value::Object(entry));
        }
    }
    Value::Object(out)
}

/// The parameters still worth pre-stripping for this key, dropping any that
/// aged out — deleted, not merely ignored, so the next persist stops carrying
/// them and the next 400 may re-learn from a provider that fixed support.
/// Returns survivors plus whether anything was forgotten.
pub fn active_learned_params(by_key: &mut LearnedMap, key: &str, now: i64) -> (Vec<String>, bool) {
    let Some(entry) = by_key.get_mut(key) else {
        return (Vec::new(), false);
    };
    let mut expired = false;
    entry.retain(|_, at| {
        let live = now - *at <= LEARNED_TTL_MS;
        if !live {
            expired = true;
        }
        live
    });
    let params: Vec<String> = entry.keys().cloned().collect();
    if entry.is_empty() {
        by_key.remove(key);
    }
    (params, expired)
}

// ── The in-process store (I/O half — llm-gateway.ts lines 324-366) ──────────

struct Store {
    loaded: bool,
    by_key: LearnedMap,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Store {
            loaded: false,
            by_key: LearnedMap::new(),
        })
    })
}

/// Read the persisted learnings once per process. Concurrent first calls may
/// both read — the merge is only-if-absent, so the in-memory stamp (by
/// definition the newer one) always wins, same as the TS lazy-promise merge.
pub async fn load_learned_params(pg: &PgPool) {
    let needs_load = !store().lock().map(|s| s.loaded).unwrap_or(false);
    if !needs_load {
        return;
    }
    let raw = get_setting(pg, SETTINGS_KEY, json!({})).await;
    let (by_key, changed) = read_learned_params(&raw, now_ms());
    let mut persist = false;
    if let Ok(mut s) = store().lock() {
        for (key, params) in by_key {
            let into = s.by_key.entry(key).or_default();
            for (param, at) in params {
                into.entry(param).or_insert(at);
            }
        }
        if !s.loaded {
            s.loaded = true;
            persist = changed;
        }
    }
    if persist {
        spawn_persist(pg.clone());
    }
}

fn snapshot() -> Option<LearnedMap> {
    store().lock().ok().map(|s| s.by_key.clone())
}

fn spawn_persist(pg: PgPool) {
    let Some(by_key) = snapshot() else { return };
    tokio::spawn(async move {
        let _ = set_setting(&pg, SETTINGS_KEY, &write_learned_params(&by_key, now_ms())).await;
    });
}

/// The pre-strip pass in buildUpstream: remove parameters this endpoint:model
/// has already rejected, and say so for contract-bearing ones — after the
/// first 400 this is the ONLY path that removes them, so a caller that trusted
/// the pre-strip would go on handing prose to a JSON parser for thirty days.
pub async fn prestrip_learned(
    pg: &PgPool,
    endpoint: &str,
    upstream_model: &str,
    body: &mut Map<String, Value>,
) -> Vec<ContractDrop> {
    load_learned_params(pg).await;
    let key = capability_key(endpoint, upstream_model);
    let (params, expired) = {
        let Ok(mut s) = store().lock() else {
            return Vec::new();
        };
        active_learned_params(&mut s.by_key, &key, now_ms())
    };
    if expired {
        spawn_persist(pg.clone());
    }
    let mut drops = Vec::new();
    for p in params {
        if !body.contains_key(&p) {
            continue; // never sent it — nothing was dropped
        }
        body.remove(&p);
        if let Some(cap) = contract_capability(&p) {
            let drop = ContractDrop {
                param: p.clone(),
                capability: cap,
                endpoint: endpoint.to_string(),
                model: upstream_model.to_string(),
                source: "remembered",
            };
            warn_contract_drop(&drop);
            drops.push(drop);
        }
    }
    drops
}

/// A 400 taught us: remember the strip (in-memory + fire-and-forget persist).
pub fn remember_param(pg: &PgPool, endpoint: &str, upstream_model: &str, param: &str) {
    if let Ok(mut s) = store().lock() {
        let key = capability_key(endpoint, upstream_model);
        let entry = s.by_key.entry(key).or_default();
        if entry.contains_key(param) {
            return;
        }
        entry.insert(param.to_string(), now_ms());
    }
    spawn_persist(pg.clone());
}

/// Clear learned strips — one endpoint:model, or all (the release valve on the
/// ratchet). Awaits its write so the calling route can report whether the
/// reset landed.
pub async fn forget_learned_params(pg: &PgPool, key: Option<&str>) -> Result<(), sqlx::Error> {
    load_learned_params(pg).await;
    if let Ok(mut s) = store().lock() {
        match key {
            None => s.by_key.clear(),
            Some(k) => {
                s.by_key.remove(k);
            }
        }
    }
    if let Ok(mut w) = warned().lock() {
        w.clear();
    }
    let Some(by_key) = snapshot() else {
        return Ok(());
    };
    set_setting(pg, SETTINGS_KEY, &write_learned_params(&by_key, now_ms())).await
}

// ── The signal a dropped contract parameter sends to the caller ──────────────

#[derive(Debug, Clone)]
pub struct ContractDrop {
    pub param: String,
    /// The capability the drop proves absent — "json" or "tools".
    pub capability: &'static str,
    pub endpoint: String,
    /// The upstream model id, as capability_key spells it.
    pub model: String,
    /// "rejected": the upstream 400'd on this very call and we retried without
    /// the parameter. "remembered": an earlier 400 taught us. The caller
    /// treats them identically; observability doesn't.
    pub source: &'static str,
}

pub fn capability_key(endpoint: &str, upstream_model: &str) -> String {
    format!("{endpoint}:{upstream_model}")
}

// One line per endpoint:model:param per process — a contract drop is a
// standing condition, not an event; a log line per call would bury the one
// that matters.
fn warned() -> &'static Mutex<HashSet<String>> {
    static W: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn warn_contract_drop(drop: &ContractDrop) {
    let id = format!(
        "{}:{}",
        capability_key(&drop.endpoint, &drop.model),
        drop.param
    );
    if let Ok(mut w) = warned().lock()
        && !w.insert(id.clone())
    {
        return;
    }
    tracing::warn!(
        "[gateway] {} rejected \"{}\" for model {} — requests are being sent WITHOUT it, so replies are no longer constrained (capability {} recorded false). Callers expecting structured output must repair or fall back. Clear with forgetLearnedParams(\"{}\").",
        drop.endpoint,
        drop.param,
        drop.model,
        drop.capability,
        capability_key(&drop.endpoint, &drop.model)
    );
}

// ── The capability fact a rejected contract parameter records ────────────────
// Port of recordCapability (harness/capability.ts) reduced to the write this
// path makes — value:false, source:"learned", expiring — preserving every
// other entry under the key and every capability id this build does not
// recognize (a rolling deploy's other process may know more than we do).

const CAPABILITY_KEY: &str = "model_capabilities";
const KNOWN_CAPABILITIES: &[&str] = &[
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

/// The stored `at` unparseable, or a learned/catalog fact past its TTL — an
/// expired fact is not a fact, which is the release valve on the ratchet.
fn fact_live(v: &Value, now: i64) -> bool {
    let Some(o) = v.as_object() else { return false };
    if !o.get("value").map(|v| v.is_boolean()).unwrap_or(false) {
        return false;
    }
    let source = o.get("source").and_then(|s| s.as_str()).unwrap_or_default();
    if !matches!(source, "probe" | "declared" | "catalog" | "learned") {
        return false;
    }
    if source != "learned" && source != "catalog" {
        return true; // probe/declared facts do not expire
    }
    o.get("at")
        .and_then(|a| a.as_str())
        .and_then(iso_to_epoch_ms)
        .map(|at| now - at <= LEARNED_TTL_MS)
        .unwrap_or(false)
}

// Every write is a read-modify-write of ONE app_settings row; the writes are
// serialized so concurrent ones cannot tail-drop each other's facts.
fn capability_writes() -> &'static tokio::sync::Mutex<()> {
    static W: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    W.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub async fn record_capability(
    pg: &PgPool,
    key: &str,
    capability: &str,
    param: &str,
    endpoint_name: &str,
) {
    let _guard = capability_writes().lock().await;
    let mut all = get_setting(pg, CAPABILITY_KEY, json!({})).await;
    let now = now_ms();
    let entry = match all.get_mut(key) {
        Some(Value::Object(e)) => e,
        _ => {
            all.as_object_mut()
                .map(|o| o.insert(key.to_string(), Value::Object(Map::new())));
            match all.get_mut(key) {
                Some(Value::Object(e)) => e,
                _ => return,
            }
        }
    };
    // Opportunistic cleanup while we hold the row — but only under capability
    // ids THIS build recognizes, and only facts that are malformed or expired.
    entry.retain(|k, v| !(KNOWN_CAPABILITIES.contains(&k.as_str()) && !fact_live(v, now)));
    entry.insert(
        capability.to_string(),
        json!({
            "value": false,
            "source": "learned",
            "at": epoch_to_iso(now),
            "detail": format!("{endpoint_name} rejected \"{param}\" with a 400; the call was retried without it."),
        }),
    );
    let _ = set_setting(pg, CAPABILITY_KEY, &all).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_protects_the_contract_and_the_transport() {
        assert_eq!(classify_param("model"), ParamClass::Protected);
        assert_eq!(classify_param("messages"), ParamClass::Protected);
        assert_eq!(classify_param("stream"), ParamClass::Protected);
        assert_eq!(classify_param("response_format"), ParamClass::Contract);
        assert_eq!(classify_param("functions"), ParamClass::Contract);
        assert_eq!(classify_param("guided_json"), ParamClass::Contract);
        assert_eq!(contract_capability("response_format"), Some("json"));
        assert_eq!(contract_capability("tool_choice"), Some("tools"));
        assert_eq!(classify_param("temperature"), ParamClass::Cosmetic);
        assert_eq!(classify_param("whatever_next"), ParamClass::Cosmetic);
    }

    #[test]
    fn rejected_param_reads_every_production_phrasing() {
        let cases: &[(&str, &str)] = &[
            // The authoritative field, dotted path reduced to root.
            (
                r#"{"error":{"message":"…","type":"invalid_request_error","param":"reasoning_effort"}}"#,
                "reasoning_effort",
            ),
            (
                r#"{"error":{"param":"response_format.json_schema.strict"}}"#,
                "response_format",
            ),
            ("`temperature` is deprecated for this model", "temperature"),
            ("Unsupported parameter: 'top_p'", "top_p"),
            (
                "Unrecognized request argument supplied: reasoning",
                "reasoning",
            ),
            ("Unknown parameter: 'reasoning'.", "reasoning"),
            (
                "Unsupported value: 'temperature' does not support 0.2 with this model.",
                "temperature",
            ),
            ("'seed' is not supported by this model", "seed"),
            (
                "response_format.type: Input should be 'json_schema'",
                "response_format",
            ),
            (
                "response_format.json_schema.strict: Field required",
                "response_format",
            ),
            ("x_api_key: Extra inputs are not permitted", "x_api_key"),
        ];
        for (text, want) in cases {
            assert_eq!(rejected_param(text).as_deref(), Some(*want), "text: {text}");
        }
        // A rejection about something we cannot name, or mere prose, is null.
        assert_eq!(rejected_param("rate limited, try again"), None);
        assert_eq!(rejected_param("bad gateway"), None);
    }

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn iso_helpers_round_trip_every_written_shape() {
        assert_eq!(epoch_to_iso(0), "1970-01-01T00:00:00.000Z");
        for ms in [0i64, 1, 999, 1_759_459_200_123, NOW] {
            let iso = epoch_to_iso(ms);
            assert_eq!(iso_to_epoch_ms(&iso), Some(ms), "{iso}");
        }
        assert_eq!(
            iso_to_epoch_ms("2026-08-29T12:00:00Z"),
            Some(1_788_004_800_000)
        );
        assert_eq!(iso_to_epoch_ms("2026-13-01T00:00:00.000Z"), None); // invalid month, like Date.parse
        assert_eq!(iso_to_epoch_ms("not a date"), None);
        assert_eq!(
            iso_to_epoch_ms("2026-08-29 12:00:00.000Z"),
            Some(1_788_004_800_000)
        ); // space tolerated
    }

    #[test]
    fn read_tolerates_legacy_arrays_and_drops_corruption() {
        // Legacy: bare names, stamped now, flagged for write-back.
        let (m, changed) = read_learned_params(
            &json!({"ep:model": ["temperature", "top_p"], "ep:other": {"seed": epoch_to_iso(NOW - 1000)}}),
            NOW,
        );
        assert!(changed);
        assert_eq!(m.get("ep:model").unwrap().get("temperature"), Some(&NOW));
        assert_eq!(m.get("ep:other").unwrap().get("seed"), Some(&(NOW - 1000)));
        // Corrupt timestamp (a number, or unparseable) drops; expired drops.
        let (m, changed) = read_learned_params(
            &json!({
                "ep:m": {"temperature": 12345, "bad": "garbage", "old": epoch_to_iso(NOW - LEARNED_TTL_MS - 1)},
                "ep:notobj": "??"
            }),
            NOW,
        );
        assert!(changed);
        // TS (gateway-params.ts:272): an entry whose every param dropped is
        // REMOVED, not left as an empty object.
        assert!(!m.contains_key("ep:m"));
        assert!(!m.contains_key("ep:notobj"));
        // Garbage shapes at the top level change nothing.
        let (m, changed) = read_learned_params(&json!([]), NOW);
        assert!(!changed && m.is_empty());
        let (m, changed) = read_learned_params(&Value::Null, NOW);
        assert!(!changed && m.is_empty());
    }

    #[test]
    fn write_prunes_and_active_deletes() {
        let mut m = LearnedMap::new();
        m.insert(
            "k".into(),
            HashMap::from([
                ("temperature".into(), NOW - 1000),
                ("old".into(), NOW - LEARNED_TTL_MS - 1),
            ]),
        );
        let out = write_learned_params(&m, NOW);
        assert_eq!(out["k"]["temperature"], json!(epoch_to_iso(NOW - 1000)));
        assert!(out["k"].get("old").is_none());
        // active_learned_params deletes the expired entry from the map itself.
        let (params, expired) = active_learned_params(&mut m, "k", NOW);
        assert!(expired);
        assert_eq!(params, vec!["temperature".to_string()]);
        assert!(!m["k"].contains_key("old"));
        // Empty entry removes the key entirely.
        let mut m = LearnedMap::new();
        m.insert(
            "k".into(),
            HashMap::from([("old".into(), NOW - LEARNED_TTL_MS - 1)]),
        );
        let (params, expired) = active_learned_params(&mut m, "k", NOW);
        assert!(expired && params.is_empty() && !m.contains_key("k"));
        // No entry: nothing expired.
        let (params, expired) = active_learned_params(&mut m, "absent", NOW);
        assert!(!expired && params.is_empty());
    }

    #[test]
    fn fact_expiry_follows_the_source() {
        let old = json!({"value": true, "source": "learned", "at": epoch_to_iso(NOW - LEARNED_TTL_MS - 1)});
        assert!(!fact_live(&old, NOW));
        let declared_old = json!({"value": true, "source": "declared", "at": epoch_to_iso(0)});
        assert!(fact_live(&declared_old, NOW)); // a human's word does not expire
        let learned_live = json!({"value": false, "source": "learned", "at": epoch_to_iso(NOW)});
        assert!(fact_live(&learned_live, NOW));
        let malformed = json!({"value": "yes", "source": "learned", "at": epoch_to_iso(NOW)});
        assert!(!fact_live(&malformed, NOW));
    }
}
