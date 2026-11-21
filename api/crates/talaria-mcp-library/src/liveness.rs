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

use futures_util::stream::{self, Stream, StreamExt};
use serde_json::Value;
use sqlx::PgPool;
use talaria_safe_fetch::{ValidatingResolver, assert_fetchable_url};
use talaria_scheduler::{JobName, JobSpec};

/// Rows per run: at 8 in flight and a 6s ceiling each, the worst case is
/// 300 / 8 × 6s ≈ 225s — inside the 4-minute `max_run_ms`. The verdicts are
/// written `VERDICT_CHUNK` at a time, so a run commits at most six times.
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
/// is answered in seconds), then the oldest verdict. Verdicts are written as
/// they land, `VERDICT_CHUNK` at a time, and each only if the row still names
/// the URL that was probed.
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

/// Verdicts per write. A chunk is one UPDATE in one transaction, so a full
/// batch commits ceil(300 / 50) = 6 times where it used to commit once a row
/// — 300 commits every five minutes, each a WAL flush that every other
/// writer on the instance queued behind (a person's click included).
pub const VERDICT_CHUNK: usize = 50;

/// One probe's answer, waiting for its chunk's write.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub name: String,
    /// The URL that was probed: the write lands only if the row still names it.
    pub url: String,
    pub state: LiveState,
    pub detail: String,
}

/// Collects verdicts as the probes land and hands back each full chunk; what
/// is left at the end is the last, short one.
#[derive(Default)]
struct Chunker {
    pending: Vec<Verdict>,
}

impl Chunker {
    fn push(&mut self, v: Verdict) -> Option<Vec<Verdict>> {
        self.pending.push(v);
        (self.pending.len() >= VERDICT_CHUNK).then(|| std::mem::take(&mut self.pending))
    }

    fn finish(self) -> Option<Vec<Verdict>> {
        (!self.pending.is_empty()).then_some(self.pending)
    }
}

/// A chunk's rows, matched by name AND the probed URL — an entry the sync
/// re-pointed while it was being probed keeps its row untouched.
const WRITE_VERDICTS: &str = "update mcp_library_entries e \
     set live_state = v.state, live_checked_at = now(), live_detail = v.detail \
     from unnest($1::text[], $2::text[], $3::text[], $4::text[]) as v(name, state, detail, url) \
     where e.registry_name = v.name and e.entry->>'url' = v.url";

/// A chunk's four columns, bound in `registry_name` order. The UPDATE walks
/// the arrays in order (a nested loop over the unnest into the primary key),
/// so this is the order it takes its row locks in: one fixed, ascending order
/// for every chunk rather than whatever order the probes happened to land in.
/// A writer that also locks by ascending name can never form a cycle with
/// it; for one that does not, see `save_chunk`.
fn chunk_columns(chunk: &[Verdict]) -> [Vec<&str>; 4] {
    let mut rows: Vec<&Verdict> = chunk.iter().collect();
    rows.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    let mut cols: [Vec<&str>; 4] = std::array::from_fn(|_| Vec::with_capacity(rows.len()));
    for v in rows {
        cols[0].push(v.name.as_str());
        cols[1].push(v.state.as_str());
        cols[2].push(v.detail.as_str());
        cols[3].push(v.url.as_str());
    }
    cols
}

/// What each chunk's own transaction read back for `synchronous_commit`, in
/// order: the live test's proof that a chunk never waits on its WAL flush.
#[cfg(test)]
static CHUNK_SYNCHRONOUS_COMMIT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// One chunk: one UPDATE in one transaction that does not wait for its WAL
/// flush (`synchronous_commit = off`, for this transaction only). A crash
/// can lose the last fraction of a second of verdicts, and that costs
/// nothing: every verdict is derived again on the row's next pass, and an
/// unwritten one leaves the row first in line for it. Rows written.
async fn write_chunk(pg: &PgPool, chunk: &[Verdict]) -> Result<u64, sqlx::Error> {
    let [names, states, details, urls] = chunk_columns(chunk);
    let mut tx = pg.begin().await?;
    sqlx::query("set local synchronous_commit = off")
        .execute(&mut *tx)
        .await?;
    #[cfg(test)]
    {
        let seen: String = sqlx::query_scalar("show synchronous_commit")
            .fetch_one(&mut *tx)
            .await?;
        CHUNK_SYNCHRONOUS_COMMIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(seen);
    }
    let written = sqlx::query(WRITE_VERDICTS)
        .bind(&names)
        .bind(&states)
        .bind(&details)
        .bind(&urls)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(written)
}

/// Postgres broke a lock cycle by aborting this transaction.
fn is_deadlock(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|code| code == "40P01")
}

/// A chunk written, or its failure logged: one lost chunk never fails the
/// batch, as one lost row never did. A chunk holds up to fifty row locks
/// until it commits, where a verdict used to hold one, so the catalog sync
/// (a page of upserts in the registry's order in one transaction, a prune
/// that deletes many rows in one statement) can meet it in a deadlock. Postgres
/// aborts one side; if that is the chunk, it is tried once more rather than
/// dropped. Rows written.
async fn save_chunk(pg: &PgPool, chunk: &[Verdict]) -> u64 {
    let mut outcome = write_chunk(pg, chunk).await;
    if outcome.as_ref().is_err_and(is_deadlock) {
        outcome = write_chunk(pg, chunk).await;
    }
    match outcome {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("[mcp/liveness] {} verdicts not saved: {e}", chunk.len());
            0
        }
    }
}

/// Every verdict of a run, written as it lands: each full chunk on a task of
/// its own beside the stream, the short last one after it, `seen` called on
/// each verdict first (the run's tally). The job's only writer, and the one
/// the live test drives. Rows written.
async fn write_as_they_land(
    pg: &PgPool,
    verdicts: impl Stream<Item = Verdict>,
    mut seen: impl FnMut(&Verdict),
) -> u64 {
    let mut verdicts = std::pin::pin!(verdicts);
    let mut chunker = Chunker::default();
    let mut writes = Vec::new();
    while let Some(v) = verdicts.next().await {
        seen(&v);
        // Each full chunk is written beside the probes, not between them: a
        // probe is not polled while this loop awaits, and its 6s deadline
        // keeps running, so a slow write here would turn live servers dead.
        // A run cut off at `max_run_ms` keeps every chunk already handed off.
        if let Some(chunk) = chunker.push(v) {
            let pg = pg.clone();
            writes.push(tokio::spawn(async move { save_chunk(&pg, &chunk).await }));
        }
    }
    let mut written = match chunker.finish() {
        Some(rest) => save_chunk(pg, &rest).await,
        None => 0,
    };
    for w in writes {
        match w.await {
            Ok(n) => written += n,
            Err(e) => tracing::warn!("[mcp/liveness] a verdict write did not finish: {e}"),
        }
    }
    written
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
    let probes = stream::iter(rows)
        .map(|(name, url)| async move {
            let (state, detail) = classify_liveness(&probe(&url).await);
            Verdict {
                name,
                url,
                state,
                detail,
            }
        })
        .buffer_unordered(LIVENESS_CONCURRENCY);
    let mut t = Tally::default();
    write_as_they_land(pg, probes, |v| match v.state {
        LiveState::Ok => t.ok += 1,
        LiveState::Auth => t.auth += 1,
        LiveState::Dead => t.dead += 1,
        LiveState::Unknown => t.unknown += 1,
    })
    .await;
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

    fn verdict(i: usize) -> Verdict {
        Verdict {
            name: format!("entry-{i}"),
            url: format!("https://entry-{i}.test/mcp"),
            state: LiveState::Dead,
            detail: format!("detail {i}"),
        }
    }

    /// The chunks a run hands to the writer for `n` verdicts, in order.
    fn chunks_of(n: usize) -> Vec<Vec<Verdict>> {
        let mut c = Chunker::default();
        let mut out: Vec<Vec<Verdict>> = (0..n).filter_map(|i| c.push(verdict(i))).collect();
        out.extend(c.finish());
        out
    }

    #[test]
    fn verdicts_go_out_fifty_at_a_time_and_each_exactly_once() {
        let sizes = |n| chunks_of(n).iter().map(Vec::len).collect::<Vec<_>>();
        assert_eq!(sizes(0), Vec::<usize>::new());
        assert_eq!(sizes(1), [1]);
        assert_eq!(sizes(49), [49]);
        assert_eq!(sizes(50), [50]);
        assert_eq!(sizes(51), [50, 1]);
        assert_eq!(sizes(300), [50; 6]);
        assert_eq!(sizes(301), [50, 50, 50, 50, 50, 50, 1]);
        // In landing order, none twice and none dropped.
        let flat: Vec<Verdict> = chunks_of(137).into_iter().flatten().collect();
        assert_eq!(flat, (0..137).map(verdict).collect::<Vec<_>>());
        // A full batch commits at most six times.
        assert_eq!(VERDICT_CHUNK, 50);
        assert_eq!(chunks_of(LIVENESS_BATCH as usize).len(), 6);
    }

    #[test]
    fn a_chunk_binds_its_rows_in_name_order_each_row_whole() {
        // Landed out of order, as probes do.
        let chunk: Vec<Verdict> = [3, 11, 0, 7, 2].into_iter().map(verdict).collect();
        let [names, states, details, urls] = chunk_columns(&chunk);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "one lock order for every chunk");
        assert_eq!(names.len(), chunk.len());
        // Every row's four fields stay together.
        for (i, name) in names.iter().enumerate() {
            let v = chunk.iter().find(|v| v.name == *name).unwrap();
            assert_eq!(
                (states[i], details[i], urls[i]),
                (v.state.as_str(), v.detail.as_str(), v.url.as_str())
            );
        }
    }

    #[test]
    fn the_write_matches_name_and_probed_url_from_four_text_arrays() {
        assert!(WRITE_VERDICTS.contains(
            "from unnest($1::text[], $2::text[], $3::text[], $4::text[]) as v(name, state, detail, url)"
        ));
        assert!(
            WRITE_VERDICTS.contains("where e.registry_name = v.name and e.entry->>'url' = v.url")
        );
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

// Live-DB proof that a run's verdicts land in chunks, through the job's own
// writer: N verdicts leave at most ceil(N / 50) distinct xmin values (one
// commit per chunk), each row the verdict it was given, a row re-pointed
// since its probe untouched, every chunk's transaction running with
// synchronous_commit off, and the connection's back to `on` after. `#[ignore]`d — it needs
// a database — and CI's live-tests job runs it
// (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-mcp-library --lib -- --ignored liveness_live
//
// Like tests/catalog_live.rs, it builds its own schema (a copy of the
// mirror's table) and points a one-connection pool at it alone, so the real
// mirror is never read or written.
#[cfg(test)]
mod liveness_live {
    use std::str::FromStr;

    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::*;

    const N: usize = 120;

    #[tokio::test]
    #[ignore]
    async fn liveness_live_n_verdicts_commit_once_a_chunk() {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (a migrated database) to run the ignored live tests");
        let admin = PgPool::connect(&url).await.expect("the database");
        let schema = format!("mcp_liveness_live_{}", std::process::id());
        for sql in [
            format!("drop schema if exists {schema} cascade"),
            format!("create schema {schema}"),
            format!(
                "create table {schema}.mcp_library_entries \
                 (like public.mcp_library_entries including all)"
            ),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                .execute(&admin)
                .await
                .expect(&sql);
        }
        let opts = PgConnectOptions::from_str(&url)
            .expect("DATABASE_URL parses")
            .options([("search_path", schema.as_str())]);
        // One connection: the synchronous_commit read at the end is the
        // same session every chunk ran on.
        let pg = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("scratch pool");
        sqlx::query(
            "insert into mcp_library_entries (registry_name, title, kind, entry) \
             select 'live/' || g, 'Live ' || g, 'hosted', \
                    jsonb_build_object('url', 'https://live-' || g || '.test/mcp') \
             from generate_series(1, $1) g",
        )
        .bind(N as i32)
        .execute(&pg)
        .await
        .unwrap();

        let states = [
            LiveState::Ok,
            LiveState::Auth,
            LiveState::Dead,
            LiveState::Unknown,
        ];
        let verdicts: Vec<Verdict> = (1..=N)
            .map(|i| Verdict {
                name: format!("live/{i}"),
                // Entry 7 was re-pointed while it was being probed.
                url: if i == 7 {
                    "https://moved.test/mcp".into()
                } else {
                    format!("https://live-{i}.test/mcp")
                },
                state: states[i % 4],
                detail: format!("detail {i}"),
            })
            .collect();
        CHUNK_SYNCHRONOUS_COMMIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        let mut seen = 0;
        let written = write_as_they_land(&pg, stream::iter(verdicts), |_| seen += 1).await;
        assert_eq!(seen, N, "the tally sees every verdict");
        assert_eq!(written, (N - 1) as u64, "every row but the re-pointed one");
        let inside = std::mem::take(
            &mut *CHUNK_SYNCHRONOUS_COMMIT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        assert_eq!(
            inside,
            vec!["off"; N.div_ceil(VERDICT_CHUNK)],
            "each chunk commits without waiting on its flush"
        );

        let commits: i64 = sqlx::query_scalar(
            "select count(distinct xmin::text) from mcp_library_entries \
             where live_checked_at is not null",
        )
        .fetch_one(&pg)
        .await
        .unwrap();
        assert!(
            commits as usize <= N.div_ceil(VERDICT_CHUNK),
            "{commits} commits for {N} verdicts"
        );
        assert_eq!(commits, 3);

        let rows: Vec<(String, Option<String>, Option<String>, bool)> = sqlx::query_as(
            "select registry_name, live_state, live_detail, live_checked_at is not null \
             from mcp_library_entries",
        )
        .fetch_all(&pg)
        .await
        .unwrap();
        assert_eq!(rows.len(), N);
        for (name, state, detail, checked) in rows {
            let i: usize = name.trim_start_matches("live/").parse().unwrap();
            if i == 7 {
                assert_eq!((state, detail, checked), (None, None, false), "{name}");
            } else {
                assert_eq!(state.as_deref(), Some(states[i % 4].as_str()), "{name}");
                assert_eq!(detail, Some(format!("detail {i}")), "{name}");
                assert!(checked, "{name}");
            }
        }

        let sync: String = sqlx::query_scalar("show synchronous_commit")
            .fetch_one(&pg)
            .await
            .unwrap();
        assert_eq!(
            sync, "on",
            "synchronous_commit = off stayed inside the chunks"
        );

        pg.close().await;
        let drop = format!("drop schema if exists {schema} cascade");
        sqlx::query(sqlx::AssertSqlSafe(drop.as_str()))
            .execute(&admin)
            .await
            .unwrap();
    }
}
