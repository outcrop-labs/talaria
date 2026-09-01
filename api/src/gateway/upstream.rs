// The outbound hop — port of buildUpstream + fetchUpstream (llm-gateway.ts):
// provider base/key, request_defaults merged UNDER the client body, the
// credential seal, the learned-param pre-strip, then the POST with its two
// adaptations (dev hostname fallback, parameter-rejection recovery) and the
// live stat ring.

use crate::gateway::params::{
    ContractDrop, classify_param, contract_capability, prestrip_learned, record_capability,
    rejected_param, remember_param, warn_contract_drop,
};
use crate::gateway::provider::{http, native_base, openrouter_us_pool, resolve_endpoint_key};
use crate::gateway::registry::ResolvedRoute;
use crate::gateway::vault::{SecretVault, seal_content};
use crate::state::AppState;
use serde_json::{Map, Value};
use sqlx::PgPool;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The ceiling on ONE upstream call when the caller names no budget of its own
/// (UPSTREAM_TIMEOUT_MS). Ten minutes is right for a long completion a human
/// is waiting on; per-request timeouts ride on the caller.
pub const UPSTREAM_TIMEOUT_MS: u64 = 600_000;

/// JS truthiness for the body fields the route branches on (`stream` is the
/// one that matters: false/absent/0/"" are non-streaming, anything else —
/// including a client's string "true" — is a stream request).
pub fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n
            .as_f64()
            .map(|f| f.is_finite() && f != 0.0)
            .unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// Objects merge recursively; everything else — arrays included — replaces.
pub fn deep_merge(base: &Value, extra: &Value) -> Value {
    match (base, extra) {
        (Value::Object(b), Value::Object(e)) => {
            let mut out = b.clone();
            for (k, v) in e {
                let merged = match out.get(k) {
                    Some(prev) => deep_merge(prev, v),
                    None => v.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        (_, e) => e.clone(),
    }
}

/// One assembled upstream request. `vault` and `contract_drops` ride the call
/// the way they ride UpstreamCall in TS — the caller reads drops AFTER the
/// fetch resolves (a live 400 can add one mid-call).
pub struct UpstreamCall {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
    pub vault: SecretVault,
    pub contract_drops: Vec<ContractDrop>,
}

/// Build the outbound request: provider base/key + request_defaults merged
/// UNDER the client body (client wins), model swapped to the upstream id.
pub async fn build_upstream(
    state: &AppState,
    route: &ResolvedRoute,
    client_body: &Value,
) -> Result<UpstreamCall, String> {
    let ep = &route.endpoint;
    let base = match &ep.base_url {
        Some(b) => b.clone(),
        None => native_base(&ep.provider)
            .map(str::to_string)
            .ok_or_else(|| format!("endpoint \"{}\" has no base URL", ep.name))?,
    };
    let base = base.strip_suffix('/').unwrap_or(&base).to_string();
    let key = resolve_endpoint_key(state, ep).await;

    let mut headers: Vec<(String, String)> =
        vec![("content-type".into(), "application/json".into())];
    if let Some(k) = &key {
        headers.push(("authorization".into(), format!("Bearer {k}")));
        // Anthropic's OpenAI-compat layer accepts x-api-key; harmless elsewhere.
        if ep.provider == "anthropic" {
            headers.push(("x-api-key".into(), k.clone()));
        }
    }

    let mut defaults = match &ep.request_defaults {
        Value::Object(_) => ep.request_defaults.clone(),
        _ => Map::new().into(),
    };
    // No-train routing on an OpenRouter endpoint: the US pool is fetched LIVE
    // from OpenRouter's provider catalog (briefly cached), never maintained by
    // hand — a stored `only` list is only the offline fallback.
    if ep.provider == "openrouter"
        && defaults
            .get("provider")
            .and_then(|p| p.get("data_collection"))
            .and_then(|d| d.as_str())
            == Some("deny")
        && let Some(pool) = openrouter_us_pool().await
        && let Some(prov) = defaults.get_mut("provider").and_then(|p| p.as_object_mut())
    {
        prov.insert(
            "only".into(),
            Value::Array(pool.into_iter().map(Value::String).collect()),
        );
    }
    let mut body = deep_merge(&defaults, client_body);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".into(), Value::String(route.upstream_model.clone()));
    }
    if js_truthy(&body["stream"]) {
        // include_usage FIRST, existing stream_options spread over it: a caller
        // who explicitly sent include_usage:false keeps their word.
        if let Some(obj) = body.as_object_mut() {
            let mut merged = Map::new();
            merged.insert("include_usage".into(), Value::Bool(true));
            if let Some(prev) = obj.get("stream_options").and_then(|v| v.as_object()) {
                for (k, v) in prev {
                    merged.insert(k.clone(), v.clone());
                }
            }
            obj.insert("stream_options".into(), Value::Object(merged));
        }
    }

    // ── CREDENTIALS DO NOT LEAVE THIS PROCESS ──────────────────────────────────
    // THE LAST POINT AT WHICH WE STILL OWN THE BYTES: every gateway call is
    // assembled here, so no caller can forget the seal. seal_content, not a
    // string-only map — an image turn's content is an array of parts, and its
    // text part is as credential-prone as any prose turn.
    let mut vault = SecretVault::default();
    if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for m in messages.iter_mut() {
            let Some(obj) = m.as_object_mut() else {
                continue;
            };
            if let Some(content) = obj.get_mut("content") {
                *content = seal_content(content, &mut vault);
            }
        }
    }

    // Pre-strip parameters this endpoint+model has already rejected (learned
    // live from the 400s in fetch_upstream — dynamic specs, no tables).
    let mut drops = Vec::new();
    if let Some(obj) = body.as_object_mut() {
        drops = prestrip_learned(&state.pg, &ep.name, &route.upstream_model, obj).await;
    }

    Ok(UpstreamCall {
        url: format!("{base}/chat/completions"),
        headers,
        body,
        vault,
        contract_drops: drops,
    })
}

/// What fetch_upstream hands back. `Synthesized` is the not-a-strippable-
/// parameter 400 relayed as-is (status + raw body + the upstream's own
/// content-type) — the route then treats it exactly like a live failure.
pub enum Reply {
    Live(reqwest::Response),
    Synthesized {
        status: u16,
        text: String,
        content_type: String,
    },
}

impl Reply {
    pub fn status(&self) -> u16 {
        match self {
            Reply::Live(r) => r.status().as_u16(),
            Reply::Synthesized { status, .. } => *status,
        }
    }

    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status())
    }

    /// The upstream's content-type, `application/json` when it sent none.
    pub fn content_type(&self) -> String {
        match self {
            Reply::Live(r) => r
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string(),
            Reply::Synthesized { content_type, .. } => content_type.clone(),
        }
    }

    pub async fn text(self) -> String {
        match self {
            Reply::Live(r) => r.text().await.unwrap_or_default(),
            Reply::Synthesized { text, .. } => text,
        }
    }
}

/// Dev-mode hostname fallback: docker-internal bare names (ollama, searxng)
/// don't resolve from the host — retry the same hop against localhost.
fn localhost_fallback(url: &str) -> Option<String> {
    let mut u = url::Url::parse(url).ok()?;
    let host = u.host_str()?.to_string();
    // Ported from the TS regex `[^/:]+`: anything with a dot or colon is a
    // real address, not a compose service name.
    if host.contains('.') || host.contains(':') || host == "localhost" {
        return None;
    }
    u.set_host(Some("localhost")).ok()?;
    Some(u.to_string())
}

async fn send(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
) -> reqwest::Result<reqwest::Response> {
    match send_once(url, headers, body).await {
        Ok(r) => Ok(r),
        Err(e) => match localhost_fallback(url) {
            Some(alt) => send_once(&alt, headers, body).await,
            None => Err(e),
        },
    }
}

async fn send_once(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
) -> reqwest::Result<reqwest::Response> {
    // A FRESH timeout per attempt, like the fresh AbortSignal per attempt in
    // TS — a retry must not inherit the first attempt's spent clock.
    let mut req = http()
        .post(url)
        .timeout(std::time::Duration::from_millis(UPSTREAM_TIMEOUT_MS))
        .json(body);
    for (k, v) in headers {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            req = req.header(name, value);
        }
    }
    req.send().await
}

/// POST to the upstream with parameter-rejection recovery: a 400 naming a
/// parameter we sent strips it and retries (≤4), remembering per
/// endpoint:model. Contract-bearing strips are recorded and reported — read
/// `call.contract_drops` after this resolves, before trusting a 200 to be the
/// shape that was asked for. Err = the hop could not be made at all.
pub async fn fetch_upstream(
    pg: &PgPool,
    call: &mut UpstreamCall,
    route: Option<&ResolvedRoute>,
) -> Result<Reply, String> {
    let started = Instant::now();
    let model = call
        .body
        .get("model")
        .and_then(|m| m.as_str())
        .or(route.map(|r| r.upstream_model.as_str()))
        .unwrap_or("?")
        .to_string();
    let out = fetch_inner(pg, call, route).await;
    let ok = out.as_ref().map(Reply::is_ok).unwrap_or(false);
    record_gateway_stat(started.elapsed().as_millis() as i64, ok, &model);
    out
}

async fn fetch_inner(
    pg: &PgPool,
    call: &mut UpstreamCall,
    route: Option<&ResolvedRoute>,
) -> Result<Reply, String> {
    for _attempt in 0..4 {
        let res = send(&call.url, &call.headers, &call.body)
            .await
            .map_err(|e| e.to_string())?;
        if res.status().as_u16() != 400 {
            return Ok(Reply::Live(res));
        }
        // Capture the header before the body read consumes the response.
        let content_type = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let text = res.text().await.unwrap_or_default();
        let Some(param) = rejected_param(&text) else {
            // Not a strippable-parameter rejection — relay the original.
            return Ok(Reply::Synthesized {
                status: 400,
                text,
                content_type,
            });
        };
        let present = call.body.get(&param).is_some();
        if !present || classify_param(&param) == crate::gateway::params::ParamClass::Protected {
            return Ok(Reply::Synthesized {
                status: 400,
                text,
                content_type,
            });
        }
        if let Some(obj) = call.body.as_object_mut() {
            obj.remove(&param);
        }
        // A contract parameter is still stripped — a completed call the caller
        // knows is unconstrained beats a 400 it can do nothing with — but never
        // quietly: caller (typed), capability store (live 400s only — a
        // remembered strip must not restamp `at` on every boot), log (once).
        if let Some(cap) = contract_capability(&param) {
            let drop = ContractDrop {
                param: param.clone(),
                capability: cap,
                endpoint: route
                    .map(|r| r.endpoint.name.clone())
                    .unwrap_or_else(|| "?".into()),
                model: call
                    .body
                    .get("model")
                    .and_then(|m| m.as_str())
                    .or(route.map(|r| r.upstream_model.as_str()))
                    .unwrap_or("?")
                    .to_string(),
                source: "rejected",
            };
            call.contract_drops.push(drop.clone());
            warn_contract_drop(&drop);
            if let Some(r) = route {
                let pg = pg.clone();
                let key =
                    crate::gateway::params::capability_key(&r.endpoint.name, &r.upstream_model);
                let detail_ep = r.endpoint.name.clone();
                let p = param.clone();
                tokio::spawn(async move {
                    record_capability(&pg, &key, cap, &p, &detail_ep).await;
                });
            }
        }
        if let Some(r) = route {
            remember_param(pg, &r.endpoint.name, &r.upstream_model, &param);
        }
    }
    let res = send(&call.url, &call.headers, &call.body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Reply::Live(res))
}

// ── Live gateway stats (in-memory, per process) ──────────────────────────────
// A small ring of recent upstream calls for the live dashboard's pulse —
// deliberately not a table; usage_events is the durable record. The pulse
// reader is `gateway_pulse` just below (model-fitness's `LatencyReading`
// extends it); the ring IS the data.

/// Written by `record_gateway_stat`, read by `gateway_pulse`.
struct GatewayStat {
    ts: i64,
    ms: i64,
    ok: bool,
    /// Recorded for parity with TS's diagnostic buffer; no reader on this side
    /// of the port yet.
    #[allow(dead_code)]
    model: String,
}

const STATS_MAX: usize = 500;

fn ring() -> &'static Mutex<VecDeque<GatewayStat>> {
    static R: OnceLock<Mutex<VecDeque<GatewayStat>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub fn record_gateway_stat(ms: i64, ok: bool, model: &str) {
    if let Ok(mut r) = ring().lock() {
        r.push_back(GatewayStat {
            ts: crate::gateway::params::now_ms(),
            ms,
            ok,
            model: model.to_string(),
        });
        while r.len() > STATS_MAX {
            r.pop_front();
        }
    }
}

/// The pulse's shape — port of llm-gateway.ts `GatewayPulse`.
pub struct GatewayPulse {
    /// Upstream calls in the last 15 minutes (this app process).
    pub requests: i64,
    pub errors: i64,
    /// Time-to-first-byte percentiles over those calls, ms.
    pub p50: Option<i64>,
    pub p95: Option<i64>,
}

/// What the ring says about the last fifteen minutes — port of `gatewayPulse`.
/// The model-fitness page reads this beside a run's price so a bad number has
/// a context, and the sweep writes it into a `LatencyReading` per model.
pub fn gateway_pulse() -> GatewayPulse {
    let cutoff = crate::gateway::params::now_ms() - 15 * 60_000;
    let ring = ring().lock().expect("the stat ring is not contended");
    let recent: Vec<&GatewayStat> = ring.iter().filter(|s| s.ts > cutoff).collect();
    let mut times: Vec<i64> = recent.iter().map(|s| s.ms).collect();
    times.sort_unstable();
    // TS's percentile is the nearest-rank index, not an interpolation: the
    // p95 of eleven calls is the eleventh, not a blend of the tenth and
    // eleventh. An empty ring answers null, never zero — zero would read as
    // "instant" rather than "no calls".
    let pct = |p: i64| -> Option<i64> {
        (!times.is_empty()).then(|| {
            let idx = ((p as usize * times.len()) / 100).min(times.len() - 1);
            times[idx]
        })
    };
    GatewayPulse {
        requests: recent.len() as i64,
        errors: recent.iter().filter(|s| !s.ok).count() as i64,
        p50: pct(50),
        p95: pct(95),
    }
}

/// `pub(crate)` for one cross-module reason: the fitness probes' latency test
/// writes this same process-global ring and takes the turnstile below.
#[cfg(test)]
pub(crate) mod pulse_tests {
    use super::*;

    /// ONE RING AT A TIME. The stat ring is process-global and two test
    /// modules write it — this one, and the fitness probes' latency test,
    /// which seeds the ring to read a pulse back. The exact nearest-rank
    /// assertions below are only stable on a ring nobody else is writing, so
    /// every test that records stats holds this turnstile for its whole body.
    pub(crate) static STAT_RING_TURNSTILE: Mutex<()> = Mutex::new(());

    /// The ring is process-global and outlives every test in the binary; the
    /// absolute percentile assertions below need one nobody has written to, so
    /// drain whatever earlier tests left before recording ours. The turnstile
    /// keeps new writes out while this test reads.
    fn drain_ring() {
        ring()
            .lock()
            .expect("the stat ring is not contended")
            .clear();
    }

    /// Nearest-rank, not interpolation: the p50 of three calls is the second,
    /// the p95 the third. An untouched ring answers null — "no calls", never
    /// "instant".
    #[test]
    fn the_pulse_is_nearest_rank_over_the_recent_ring() {
        let _ring = STAT_RING_TURNSTILE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drain_ring();
        let before = gateway_pulse();
        record_gateway_stat(100, true, "probe-a");
        record_gateway_stat(300, false, "probe-a");
        record_gateway_stat(200, true, "probe-a");
        let pulse = gateway_pulse();
        assert_eq!(pulse.requests, before.requests + 3);
        assert_eq!(pulse.errors, before.errors + 1);
        assert_eq!(pulse.p50, Some(200));
        assert_eq!(pulse.p95, Some(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deep_merge_recurses_objects_replaces_everything_else() {
        let base = json!({"a": 1, "nested": {"x": 1, "y": 2}, "arr": [1, 2], "s": "base"});
        let extra = json!({"nested": {"y": 3, "z": 4}, "arr": [9], "s": null});
        let out = deep_merge(&base, &extra);
        assert_eq!(out["a"], json!(1)); // base survives
        assert_eq!(out["nested"], json!({"x": 1, "y": 3, "z": 4})); // recursive
        assert_eq!(out["arr"], json!([9])); // arrays replace, never merge
        assert_eq!(out["s"], Value::Null); // null replaces (client wins)
    }

    #[test]
    fn truthiness_is_js_truthiness() {
        assert!(js_truthy(&json!("true")));
        assert!(js_truthy(&json!("x")));
        assert!(js_truthy(&json!(1)));
        assert!(js_truthy(&json!({})));
        assert!(!js_truthy(&Value::Null));
        assert!(!js_truthy(&json!(false)));
        assert!(!js_truthy(&json!(0)));
        assert!(!js_truthy(&json!("")));
    }

    #[test]
    fn dev_fallback_rewrites_only_bare_hostnames() {
        assert_eq!(
            localhost_fallback("http://ollama:11434/v1/chat/completions").as_deref(),
            Some("http://localhost:11434/v1/chat/completions")
        );
        assert_eq!(localhost_fallback("https://api.z.ai/v1/x"), None); // real host
        assert_eq!(localhost_fallback("http://localhost:8080/x"), None); // already local
        assert_eq!(localhost_fallback("https://127.0.0.1:9/x"), None); // dotted
        assert_eq!(localhost_fallback("not a url"), None);
    }
}
