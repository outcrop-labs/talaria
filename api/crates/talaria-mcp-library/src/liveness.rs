// Catalog LIVENESS — does a hosted entry's endpoint still answer?
//
// The mirror is every hosted server the registry lists, ~42k rows, and a good
// share of them are gone: weekend demos, expired tunnels, hosts that 404. An
// install of one fails at discovery, after the person already chose it. The
// `mcp-library-liveness` job asks each endpoint the one question an MCP
// client asks first — a JSON-RPC `initialize` POST — and records the answer
// on the row, so search sinks the dead and the default view hides them.
//
// ONE PROBE, NO CREDENTIALS. A server that wants a key answers 401/403: that
// is ALIVE (`auth`), not dead — most first-party servers are exactly this. A
// 404/405/410, a 5xx, a host that does not resolve, refuses the connection,
// fails TLS or never answers is `dead`. Anything else (a 400 for a missing
// session header, a 2xx that is not JSON-RPC, a redirect loop) is `unknown`
// and stays visible: hiding a server is a claim, and only a clear failure
// earns it.
//
// The probe dials through the SSRF guard's validating resolver and walks at
// most three redirects by hand, re-validating every hop — these URLs come
// from a public feed anyone can publish to.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use serde_json::Value;
use sqlx::PgPool;
use talaria_safe_fetch::{ValidatingResolver, assert_fetchable_url};
use talaria_scheduler::{JobName, JobSpec};

/// Rows per run: at 8 in flight and a 6s ceiling each, the worst case is
/// 300 / 8 × 6s ≈ 225s — inside the 4-minute `max_run_ms`.
pub const LIVENESS_BATCH: i64 = 300;
const LIVENESS_CONCURRENCY: usize = 8;
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_REDIRECTS: u32 = 3;
/// Enough to see a JSON-RPC answer or the first SSE event; a server that
/// streams more than this has already answered.
const MAX_BODY: usize = 64 * 1024;
const DETAIL_MAX: usize = 200;

pub const LIVENESS_EVERY_MS: u64 = 5 * 60_000;
/// After boot settles and well after the catalog sync's first page.
pub const LIVENESS_FIRST_RUN_DELAY_MS: u64 = 3 * 60_000;
pub const LIVENESS_MAX_RUN_MS: u64 = 4 * 60_000;

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"talaria-catalog","version":"1"}}}"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveState {
    Ok,
    /// Alive, wants credentials (401/403/407).
    Auth,
    Dead,
    Unknown,
}

impl LiveState {
    pub fn as_str(self) -> &'static str {
        match self {
            LiveState::Ok => "ok",
            LiveState::Auth => "auth",
            LiveState::Dead => "dead",
            LiveState::Unknown => "unknown",
        }
    }
}

/// What came back when the endpoint answered at all.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeAnswer {
    pub status: u16,
    pub content_type: Option<String>,
    /// The first `MAX_BODY` bytes (or, for SSE, up to the first data line).
    pub body: Vec<u8>,
}

/// Why it did not answer.
#[derive(Clone, Debug, PartialEq)]
pub enum ProbeFailure {
    Timeout,
    Dns(String),
    Connect(String),
    Tls(String),
    /// The SSRF guard refused the address — nothing on this instance can
    /// reach it, the gateway included.
    Blocked(String),
    TooManyRedirects,
    Other(String),
}

fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= DETAIL_MAX {
        s.to_string()
    } else {
        s.chars().take(DETAIL_MAX - 1).collect::<String>() + "…"
    }
}

fn has_sse_data(body: &[u8]) -> bool {
    String::from_utf8_lossy(body).lines().any(|l| {
        l.strip_prefix("data:")
            .is_some_and(|d| !d.trim().is_empty())
    })
}

fn rpc_result(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.contains_key("result"),
        Value::Array(a) => a.iter().any(rpc_result),
        _ => false,
    }
}

/// The verdict and the one-line reason the UI shows beside it. Pure — every
/// branch is pinned by a test.
pub fn classify_liveness(outcome: &Result<ProbeAnswer, ProbeFailure>) -> (LiveState, String) {
    let a = match outcome {
        Err(f) => {
            return match f {
                ProbeFailure::Timeout => (
                    LiveState::Dead,
                    format!("no answer within {}s", PROBE_TIMEOUT.as_secs()),
                ),
                ProbeFailure::Dns(e) => (LiveState::Dead, clip(&format!("DNS: {e}"))),
                ProbeFailure::Connect(e) => {
                    (LiveState::Dead, clip(&format!("connection failed: {e}")))
                }
                ProbeFailure::Tls(e) => (LiveState::Dead, clip(&format!("TLS failed: {e}"))),
                ProbeFailure::Blocked(e) => (LiveState::Dead, clip(&format!("not reachable: {e}"))),
                ProbeFailure::TooManyRedirects => (
                    LiveState::Unknown,
                    format!("more than {MAX_REDIRECTS} redirects"),
                ),
                ProbeFailure::Other(e) => (LiveState::Unknown, clip(e)),
            };
        }
        Ok(a) => a,
    };
    let s = a.status;
    match s {
        401 | 403 | 407 => (
            LiveState::Auth,
            format!("HTTP {s} — answers, wants credentials"),
        ),
        404 | 405 | 410 => (LiveState::Dead, format!("HTTP {s} — no MCP endpoint here")),
        500..=599 => (LiveState::Dead, format!("HTTP {s} — server error")),
        200..=299 => {
            let sse = a
                .content_type
                .as_deref()
                .is_some_and(|c| c.to_ascii_lowercase().contains("text/event-stream"));
            if (sse || a.content_type.is_none()) && has_sse_data(&a.body) {
                return (LiveState::Ok, format!("HTTP {s} — answered initialize"));
            }
            match serde_json::from_slice::<Value>(&a.body) {
                Ok(v) if rpc_result(&v) => {
                    (LiveState::Ok, format!("HTTP {s} — answered initialize"))
                }
                Ok(v) if v.get("error").is_some() => {
                    let msg = v
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("no message");
                    (
                        LiveState::Unknown,
                        clip(&format!("HTTP {s} — JSON-RPC error: {msg}")),
                    )
                }
                _ if has_sse_data(&a.body) => {
                    (LiveState::Ok, format!("HTTP {s} — answered initialize"))
                }
                _ => (
                    LiveState::Unknown,
                    format!("HTTP {s} — not a JSON-RPC answer"),
                ),
            }
        }
        _ => (LiveState::Unknown, format!("HTTP {s}")),
    }
}

// ── The probe ───────────────────────────────────────────────────────────────

fn probe_client() -> reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            // We walk the chain ourselves so every hop is re-validated.
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(ValidatingResolver))
            .user_agent("Talaria")
            .build()
            .expect("liveness client builds")
    });
    CLIENT.clone()
}

/// The causes under reqwest's own "error sending request for url (…)" —
/// that wrapper repeats the URL the UI already shows and pushes the reason
/// past the detail's clip, so it is dropped when there is anything beneath.
fn error_chain(e: &reqwest::Error) -> String {
    let mut causes: Vec<String> = Vec::new();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        let msg = s.to_string();
        if !causes.iter().any(|c| c.contains(&msg)) {
            causes.push(msg);
        }
        src = s.source();
    }
    if causes.is_empty() {
        e.to_string()
    } else {
        causes.join(": ")
    }
}

/// reqwest flattens dns/connect/tls into "error sending request"; the
/// source chain carries which it was.
fn failure_of(e: &reqwest::Error) -> ProbeFailure {
    failure_from(&error_chain(e), e.is_timeout(), e.is_connect())
}

/// The mapping itself, pure, over what a reqwest error reports — the cause
/// chain's text and its two flags — so a test can pin every arm without a
/// network. The order matters: a guard refusal and a DNS miss both surface
/// as connect errors, and a TLS failure often does too.
fn failure_from(chain: &str, is_timeout: bool, is_connect: bool) -> ProbeFailure {
    if is_timeout {
        return ProbeFailure::Timeout;
    }
    let lower = chain.to_lowercase();
    if lower.contains("refusing ") {
        ProbeFailure::Blocked(chain.to_string())
    } else if lower.contains("dns error")
        || lower.contains("failed to lookup")
        || lower.contains("name or service not known")
        || lower.contains("no such host")
    {
        ProbeFailure::Dns(chain.to_string())
    } else if lower.contains("certificate") || lower.contains("tls") || lower.contains("handshake")
    {
        ProbeFailure::Tls(chain.to_string())
    } else if is_connect {
        ProbeFailure::Connect(chain.to_string())
    } else {
        ProbeFailure::Other(chain.to_string())
    }
}

fn guard_failure(e: talaria_safe_fetch::BlockedUrlError) -> ProbeFailure {
    if e.0.starts_with("could not resolve") {
        ProbeFailure::Dns(e.0)
    } else {
        ProbeFailure::Blocked(e.0)
    }
}

/// One `initialize` POST, 6s in all, redirects walked by hand.
pub async fn probe(url: &str) -> Result<ProbeAnswer, ProbeFailure> {
    let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
    let mut target = tokio::time::timeout_at(deadline, assert_fetchable_url(url))
        .await
        .map_err(|_| ProbeFailure::Timeout)?
        .map_err(guard_failure)?;
    let mut post = true;
    let mut hops = 0u32;
    loop {
        let mut req = probe_client()
            .request(
                if post {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::GET
                },
                target.clone(),
            )
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            );
        if post {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(INITIALIZE);
        }
        let res = tokio::time::timeout_at(deadline, req.send())
            .await
            .map_err(|_| ProbeFailure::Timeout)?
            .map_err(|e| failure_of(&e))?;
        let status = res.status().as_u16();
        let location = matches!(status, 301 | 302 | 303 | 307 | 308)
            .then(|| {
                res.headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            })
            .flatten();
        if let Some(location) = location {
            if hops >= MAX_REDIRECTS {
                return Err(ProbeFailure::TooManyRedirects);
            }
            let joined = target
                .join(&location)
                .map_err(|e| ProbeFailure::Other(format!("bad redirect location: {e}")))?;
            target = tokio::time::timeout_at(deadline, assert_fetchable_url(joined.as_str()))
                .await
                .map_err(|_| ProbeFailure::Timeout)?
                .map_err(guard_failure)?;
            // fetch()'s rules: 303 always, 301/302 from a POST, become GET.
            if matches!(status, 301..=303) {
                post = false;
            }
            hops += 1;
            continue;
        }
        let content_type = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let sse = content_type
            .as_deref()
            .is_some_and(|c| c.to_ascii_lowercase().contains("text/event-stream"));
        let body = read_body(res, deadline, sse).await;
        return Ok(ProbeAnswer {
            status,
            content_type,
            body,
        });
    }
}

/// Read until the cap, the end, or the deadline — whichever is first. An
/// SSE stream may stay open after its answer, so it stops at the first data
/// line; a deadline mid-body keeps what arrived (the status already spoke).
async fn read_body(
    mut res: reqwest::Response,
    deadline: tokio::time::Instant,
    sse: bool,
) -> Vec<u8> {
    let mut body = Vec::new();
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout_at(deadline, res.chunk()).await {
        body.extend_from_slice(&chunk);
        if body.len() >= MAX_BODY {
            body.truncate(MAX_BODY);
            break;
        }
        if sse && has_sse_data(&body) {
            break;
        }
    }
    body
}

// ── The batch ───────────────────────────────────────────────────────────────

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Tally {
    ok: usize,
    auth: usize,
    dead: usize,
    unknown: usize,
}

/// One batch: up to `LIVENESS_BATCH` hosted rows — never-checked first
/// (stand-ins ahead of registry rows among those, so an operator's new entry
/// is answered in seconds), then the oldest verdict. Each verdict is written
/// as it lands, and only if the row still names the URL that was probed.
pub async fn liveness_once(pg: &PgPool) -> Result<String, String> {
    if IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return Ok("liveness batch already running in this process".into());
    }
    // Cleared on drop, not after the await: a probe that panics unwinds
    // through here into the scheduler's catch_unwind, and a flag left set
    // would answer "already running" — as a success — for the life of the
    // process, silently ending liveness on this instance.
    let _clear = InFlight;
    run_batch(pg).await
}

struct InFlight;

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::Release);
    }
}

async fn run_batch(pg: &PgPool) -> Result<String, String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select registry_name, entry->>'url' from mcp_library_entries \
         where kind = 'hosted' and entry->>'url' like 'https://%' \
         order by live_checked_at asc nulls first, (source <> 'registry') desc, registry_name \
         limit $1",
    )
    .bind(LIVENESS_BATCH)
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Ok("no hosted entries to check".into());
    }
    let n = rows.len();
    let verdicts: Vec<LiveState> = stream::iter(rows)
        .map(|(name, url)| async move {
            let (state, detail) = classify_liveness(&probe(&url).await);
            let write = sqlx::query(
                "update mcp_library_entries set live_state = $2, live_checked_at = now(), live_detail = $3 \
                 where registry_name = $1 and entry->>'url' = $4",
            )
            .bind(&name)
            .bind(state.as_str())
            .bind(&detail)
            .bind(&url)
            .execute(pg)
            .await;
            if let Err(e) = write {
                tracing::warn!("[mcp/liveness] verdict for {name} not saved: {e}");
            }
            state
        })
        .buffer_unordered(LIVENESS_CONCURRENCY)
        .collect()
        .await;
    let mut t = Tally::default();
    for v in verdicts {
        match v {
            LiveState::Ok => t.ok += 1,
            LiveState::Auth => t.auth += 1,
            LiveState::Dead => t.dead += 1,
            LiveState::Unknown => t.unknown += 1,
        }
    }
    Ok(format!(
        "checked {n}: {} ok, {} auth, {} dead, {} unknown",
        t.ok, t.auth, t.dead, t.unknown
    ))
}

/// The admin trigger: a batch now, off the request.
pub fn trigger_liveness(pg: PgPool) {
    if IN_FLIGHT.load(Ordering::Relaxed) {
        return;
    }
    tokio::spawn(async move {
        match liveness_once(&pg).await {
            Ok(report) => tracing::info!("[mcp/liveness] {report}"),
            Err(e) => tracing::warn!("[mcp/liveness] batch failed: {e}"),
        }
    });
}

pub fn running() -> bool {
    IN_FLIGHT.load(Ordering::Relaxed)
}

/// Leased: the verdicts are shared rows, one checker per interval.
pub fn mcp_library_liveness_job_spec(pg: PgPool) -> JobSpec {
    JobSpec {
        name: JobName::McpLibraryLiveness,
        every_ms: LIVENESS_EVERY_MS,
        first_run_delay_ms: Some(LIVENESS_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(LIVENESS_MAX_RUN_MS),
        per_instance: false,
        run: Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move { liveness_once(&pg).await.map(Some) })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, ct: Option<&str>, body: &str) -> Result<ProbeAnswer, ProbeFailure> {
        Ok(ProbeAnswer {
            status,
            content_type: ct.map(str::to_string),
            body: body.as_bytes().to_vec(),
        })
    }

    fn state(o: Result<ProbeAnswer, ProbeFailure>) -> LiveState {
        classify_liveness(&o).0
    }

    const RESULT: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18"}}"#;

    #[test]
    fn a_2xx_json_rpc_result_or_sse_data_is_ok() {
        assert_eq!(
            state(answer(200, Some("application/json"), RESULT)),
            LiveState::Ok
        );
        // A batch answer counts too.
        assert_eq!(
            state(answer(
                200,
                Some("application/json"),
                &format!("[{RESULT}]")
            )),
            LiveState::Ok
        );
        assert_eq!(
            state(answer(
                200,
                Some("text/event-stream; charset=utf-8"),
                &format!("event: message\ndata: {RESULT}\n\n")
            )),
            LiveState::Ok
        );
        // SSE framing without the header (some proxies drop it).
        assert_eq!(
            state(answer(200, None, &format!("data: {RESULT}\n\n"))),
            LiveState::Ok
        );
        // 202 Accepted with an SSE answer is still 2xx.
        assert_eq!(
            state(answer(202, Some("text/event-stream"), "data: {}\n")),
            LiveState::Ok
        );
        let (_, detail) = classify_liveness(&answer(200, Some("application/json"), RESULT));
        assert!(detail.contains("answered initialize"), "{detail}");
    }

    #[test]
    fn sse_framing_under_a_non_sse_content_type_is_ok() {
        // The fallback arm: a gateway that relabels the stream as JSON or
        // plain text still delivered the event — not JSON, but an answer.
        for ct in ["application/json", "text/plain", "application/octet-stream"] {
            let (st, detail) = classify_liveness(&answer(
                200,
                Some(ct),
                &format!("event: message\ndata: {RESULT}\n\n"),
            ));
            assert_eq!(st, LiveState::Ok, "{ct}");
            assert!(detail.contains("answered initialize"), "{detail}");
        }
        // The same framing with no data line proves nothing.
        assert_eq!(
            state(answer(200, Some("text/plain"), "event: message\n\n")),
            LiveState::Unknown
        );
        assert_eq!(
            state(answer(200, Some("application/json"), "data:   \n")),
            LiveState::Unknown
        );
    }

    #[test]
    fn a_2xx_that_is_not_an_answer_is_unknown() {
        // HTML landing page, empty body, a JSON-RPC error, an SSE stream with
        // no data line yet — all answered, none proved MCP.
        assert_eq!(
            state(answer(200, Some("text/html"), "<html>hi</html>")),
            LiveState::Unknown
        );
        assert_eq!(state(answer(204, None, "")), LiveState::Unknown);
        let err = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"bad session"}}"#;
        let (s, detail) = classify_liveness(&answer(200, Some("application/json"), err));
        assert_eq!(s, LiveState::Unknown);
        assert!(detail.contains("bad session"), "{detail}");
        assert_eq!(
            state(answer(200, Some("text/event-stream"), ": ping\n\n")),
            LiveState::Unknown
        );
        assert_eq!(
            state(answer(200, Some("application/json"), r#"{"ok":true}"#)),
            LiveState::Unknown
        );
    }

    #[test]
    fn credentials_challenges_are_alive() {
        for s in [401, 403, 407] {
            let (st, detail) = classify_liveness(&answer(s, Some("application/json"), "{}"));
            assert_eq!(st, LiveState::Auth, "{s}");
            assert!(detail.contains("credentials"), "{detail}");
        }
    }

    #[test]
    fn missing_endpoints_and_server_errors_are_dead() {
        for s in [404, 405, 410, 500, 502, 503, 504, 599] {
            assert_eq!(
                state(answer(s, Some("text/html"), "")),
                LiveState::Dead,
                "{s}"
            );
        }
    }

    #[test]
    fn every_other_status_is_unknown() {
        for s in [100, 301, 304, 400, 402, 406, 409, 415, 418, 422, 429, 600] {
            assert_eq!(state(answer(s, None, "")), LiveState::Unknown, "{s}");
        }
    }

    #[test]
    fn network_failures_are_dead_and_the_rest_unknown() {
        for f in [
            ProbeFailure::Timeout,
            ProbeFailure::Dns("no such host".into()),
            ProbeFailure::Connect("connection refused".into()),
            ProbeFailure::Tls("invalid peer certificate".into()),
            ProbeFailure::Blocked("refusing 127.0.0.1 — loopback".into()),
        ] {
            assert_eq!(state(Err(f.clone())), LiveState::Dead, "{f:?}");
        }
        assert_eq!(
            state(Err(ProbeFailure::TooManyRedirects)),
            LiveState::Unknown
        );
        assert_eq!(
            state(Err(ProbeFailure::Other(
                "connection closed before message completed".into()
            ))),
            LiveState::Unknown
        );
        let (_, d) = classify_liveness(&Err(ProbeFailure::Timeout));
        assert_eq!(d, "no answer within 6s");
    }

    #[test]
    fn details_are_clipped_for_the_ui() {
        let long = "x".repeat(1000);
        let (_, d) = classify_liveness(&Err(ProbeFailure::Other(long)));
        assert_eq!(d.chars().count(), DETAIL_MAX);
        assert!(d.ends_with('…'));
    }

    #[test]
    fn transport_errors_map_to_their_cause() {
        // The chains below are the shapes reqwest/hyper/rustls actually
        // print (error_chain drops the "error sending request" wrapper).
        let cases: [(&str, bool, bool, fn(&ProbeFailure) -> bool); 11] = [
            ("operation timed out", true, true, |f| {
                *f == ProbeFailure::Timeout
            }),
            // Timeout wins over anything the text says.
            ("dns error: failed to lookup", true, false, |f| {
                *f == ProbeFailure::Timeout
            }),
            (
                "client error (Connect): refusing 10.0.0.1 — private address",
                false,
                true,
                |f| matches!(f, ProbeFailure::Blocked(_)),
            ),
            (
                "client error (Connect): dns error: no record found",
                false,
                true,
                |f| matches!(f, ProbeFailure::Dns(_)),
            ),
            (
                "failed to lookup address information: Name or service not known",
                false,
                true,
                |f| matches!(f, ProbeFailure::Dns(_)),
            ),
            ("No such host is known", false, true, |f| {
                matches!(f, ProbeFailure::Dns(_))
            }),
            (
                "client error (Connect): invalid peer certificate: UnknownIssuer",
                false,
                true,
                |f| matches!(f, ProbeFailure::Tls(_)),
            ),
            (
                "client error (Connect): tls handshake eof",
                false,
                true,
                |f| matches!(f, ProbeFailure::Tls(_)),
            ),
            (
                "received fatal alert: HandshakeFailure",
                false,
                false,
                |f| matches!(f, ProbeFailure::Tls(_)),
            ),
            (
                "client error (Connect): Connection refused (os error 111)",
                false,
                true,
                |f| matches!(f, ProbeFailure::Connect(_)),
            ),
            (
                "connection closed before message completed",
                false,
                false,
                |f| matches!(f, ProbeFailure::Other(_)),
            ),
        ];
        for (chain, timeout, connect, want) in cases {
            let got = failure_from(chain, timeout, connect);
            assert!(
                want(&got),
                "{chain:?} (timeout={timeout}, connect={connect}) → {got:?}"
            );
        }
        // And the verdict each lands on: the network failures are dead, the
        // unexplained one stays visible.
        assert_eq!(
            state(Err(failure_from("dns error: no record found", false, true))),
            LiveState::Dead
        );
        assert_eq!(
            state(Err(failure_from("tls handshake eof", false, true))),
            LiveState::Dead
        );
        assert_eq!(
            state(Err(failure_from(
                "connection closed before message completed",
                false,
                false
            ))),
            LiveState::Unknown
        );
        // The chain is kept for the detail line.
        assert_eq!(
            failure_from("Connection refused (os error 111)", false, true),
            ProbeFailure::Connect("Connection refused (os error 111)".into())
        );
    }

    #[test]
    fn a_panicking_batch_does_not_wedge_the_flag() {
        // The guard is what liveness_once holds; dropping it during an
        // unwind must leave the next run free to start.
        let r = std::panic::catch_unwind(|| {
            IN_FLIGHT.store(true, Ordering::Release);
            let _clear = InFlight;
            panic!("a probe blew up");
        });
        assert!(r.is_err());
        assert!(!running(), "the flag outlived the panic");
    }

    #[test]
    fn guard_refusals_split_dns_from_blocked() {
        assert_eq!(
            guard_failure(talaria_safe_fetch::BlockedUrlError(
                "could not resolve nope.invalid".into()
            )),
            ProbeFailure::Dns("could not resolve nope.invalid".into())
        );
        assert!(matches!(
            guard_failure(talaria_safe_fetch::BlockedUrlError(
                "refusing 10.0.0.1 — private".into()
            )),
            ProbeFailure::Blocked(_)
        ));
    }

    #[test]
    fn states_spell_the_column_values() {
        let all = [
            LiveState::Ok,
            LiveState::Auth,
            LiveState::Dead,
            LiveState::Unknown,
        ];
        let spelled: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
        assert_eq!(spelled, ["ok", "auth", "dead", "unknown"]);
    }

    #[tokio::test]
    async fn an_unbound_loopback_port_is_dead() {
        // The guard refuses loopback outright unless allowlisted; either way
        // nothing answers on port 9 of this host's loopback, so either way
        // the verdict is dead.
        let out = probe("https://127.0.0.1:9/mcp").await;
        let (state, detail) = classify_liveness(&out);
        assert_eq!(state, LiveState::Dead, "{out:?}");
        // The cause, not reqwest's "error sending request for url" wrapper.
        assert!(!detail.contains("error sending request"), "{detail}");
    }

    #[tokio::test]
    async fn the_job_spec_carries_the_declared_timings() {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool builds without connecting");
        let spec = mcp_library_liveness_job_spec(pg);
        assert_eq!(spec.name.as_str(), "mcp-library-liveness");
        assert_eq!(spec.every_ms, 5 * 60_000);
        assert_eq!(spec.first_run_delay_ms, Some(3 * 60_000));
        assert_eq!(spec.max_run_ms, Some(4 * 60_000));
        assert!(!spec.per_instance, "verdicts are shared rows: leased");
    }
}
