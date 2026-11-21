// Catalog LIVENESS — does a hosted entry's endpoint still answer?
//
// The mirror is every hosted server the registry lists, ~42k rows, and a good
// share of them are gone: weekend demos, expired tunnels, hosts that 404. An
// install of one fails at discovery, after the person already chose it. The
// `mcp-library-liveness` job asks each endpoint the one question an MCP
// client asks first — a JSON-RPC `initialize` POST — and records the answer
// on the row, so search sinks the dead and the default view hides them.
//
// WHAT A RUN WRITES. The verdict and its detail live on the entry row and
// change there only when the probe says something new: an entry whose
// endpoint answered the same way as last time is not rewritten. When it was
// last asked lives in `mcp_library_liveness`, one narrow row per entry
// whose only index is its primary key, so the stamp every probe leaves is a
// HOT update in place. Stamping the entry row itself rewrote a ~1.2 kB row on
// a random heap page and all six of its indexes for every probe, changed or
// not: ~22 kB of WAL a verdict, the bulk of a production instance's WAL.
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
/// is answered in seconds), then the oldest stamp. Verdicts are written as
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

/// Verdicts per write. A chunk is one transaction, so a full batch commits
/// ceil(300 / 50) = 6 times where it used to commit once a row — 300 commits
/// every five minutes, each a WAL flush that every other writer on the
/// instance queued behind (a person's click included).
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

/// A chunk's verdicts onto the entries, matched by name AND the probed URL —
/// an entry the sync re-pointed while it was being probed keeps its row
/// untouched — and only where the verdict or its detail changed. An UPDATE
/// whose WHERE fails takes no lock and writes nothing, so a run in which no
/// endpoint changed its answer rewrites no entry row.
const WRITE_VERDICTS: &str = "update mcp_library_entries e \
     set live_state = v.state, live_detail = v.detail \
     from unnest($1::text[], $2::text[], $3::text[], $4::text[]) as v(name, state, detail, url) \
     where e.registry_name = v.name and e.entry->>'url' = v.url \
       and (e.live_state, e.live_detail) is distinct from (v.state, v.detail)";

/// The probe's stamp, for every verdict whose entry still names the probed
/// URL: a plain UPDATE of the stamps already on file (`checked_at` is no
/// index's column, so each is a HOT update on its own page). Not an INSERT …
/// ON CONFLICT DO UPDATE, which writes a row-lock record before every update.
const RESTAMP: &str = "update mcp_library_liveness l set checked_at = now() \
     from unnest($1::text[], $2::text[]) as v(name, url) \
     join mcp_library_entries e on e.registry_name = v.name and e.entry->>'url' = v.url \
     where l.registry_name = v.name";

/// Then the first stamp of an entry never asked before (or asked again since
/// the sync changed it, which drops its stamp).
const FIRST_STAMPS: &str = "insert into mcp_library_liveness (registry_name, checked_at) \
     select v.name, now() \
     from unnest($1::text[], $2::text[]) as v(name, url) \
     join mcp_library_entries e on e.registry_name = v.name and e.entry->>'url' = v.url \
     where not exists (select 1 from mcp_library_liveness l where l.registry_name = v.name) \
     order by v.name \
     on conflict do nothing";

/// Stamps whose entry is gone (the sync deletes entries without a foreign
/// key's lock on them). Every run sweeps; on a run after no deletion it
/// matches nothing and takes no transaction id.
const SWEEP_ORPHANS: &str = "delete from mcp_library_liveness l \
     where not exists (select 1 from mcp_library_entries e \
                       where e.registry_name = l.registry_name)";

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
/// order, beside the `search_path` it ran under: the live tests' proof that a
/// chunk never waits on its WAL flush, each test reading only its own
/// schema's chunks while the others run beside it.
#[cfg(test)]
static CHUNK_SYNCHRONOUS_COMMIT: std::sync::Mutex<Vec<(String, String)>> =
    std::sync::Mutex::new(Vec::new());

/// One chunk, in one transaction that does not wait for its WAL flush
/// (`synchronous_commit = off`, for this transaction only): the changed
/// verdicts onto their entries, then the stamps, each in name order. A crash
/// can lose the last fraction of a second of verdicts, and that costs
/// nothing: every verdict is derived again on the row's next pass, and an
/// unwritten stamp leaves the row first in line for it. Verdicts landed (the
/// entry still named the probed URL), changed or not.
async fn write_chunk(pg: &PgPool, chunk: &[Verdict]) -> Result<u64, sqlx::Error> {
    let [names, states, details, urls] = chunk_columns(chunk);
    let mut tx = pg.begin().await?;
    sqlx::query("set local synchronous_commit = off")
        .execute(&mut *tx)
        .await?;
    #[cfg(test)]
    {
        let seen: (String, String) = sqlx::query_as(
            "select current_setting('search_path'), current_setting('synchronous_commit')",
        )
        .fetch_one(&mut *tx)
        .await?;
        CHUNK_SYNCHRONOUS_COMMIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(seen);
    }
    // Entries before stamps, each in name order. The catalog sync writes an
    // entry and resets its stamp in one statement; should the two meet in a
    // deadlock, Postgres aborts one side and `save_chunk` tries a chunk again.
    sqlx::query(WRITE_VERDICTS)
        .bind(&names)
        .bind(&states)
        .bind(&details)
        .bind(&urls)
        .execute(&mut *tx)
        .await?;
    let restamped = sqlx::query(RESTAMP)
        .bind(&names)
        .bind(&urls)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let first = sqlx::query(FIRST_STAMPS)
        .bind(&names)
        .bind(&urls)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(restamped + first)
}

/// The orphan sweep, in a transaction of its own that does not wait for its
/// flush either: a stamp is telemetry, and one a crash loses is swept again.
async fn sweep_orphans(pg: &PgPool) -> Result<u64, sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("set local synchronous_commit = off")
        .execute(&mut *tx)
        .await?;
    let swept = sqlx::query(SWEEP_ORPHANS)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(swept)
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
/// the live tests drive. Verdicts landed.
pub(crate) async fn write_as_they_land(
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

/// A run's rows: never-checked first (stand-ins ahead of registry rows among
/// those), then the oldest stamp, hosted https endpoints only. An entry with
/// no stamp is one never asked, or one the sync changed since.
pub(crate) const SELECT_BATCH: &str = "select e.registry_name, e.entry->>'url' \
     from mcp_library_entries e \
     left join mcp_library_liveness l on l.registry_name = e.registry_name \
     where e.kind = 'hosted' and e.entry->>'url' like 'https://%' \
     order by l.checked_at asc nulls first, (e.source <> 'registry') desc, e.registry_name \
     limit $1";

async fn run_batch(pg: &PgPool) -> Result<String, String> {
    run_batch_with(pg, |_name, url| async move {
        classify_liveness(&probe(&url).await)
    })
    .await
}

/// One batch, asking each row's endpoint through `prober` (name, url) →
/// (verdict, detail). The job's prober is `probe` read by
/// `classify_liveness`; the live census hands in a stand-in, so what it
/// measures is the job's own selection and writer, with no network.
async fn run_batch_with<P, F>(pg: &PgPool, prober: P) -> Result<String, String>
where
    P: Fn(String, String) -> F,
    F: std::future::Future<Output = (LiveState, String)>,
{
    let rows: Vec<(String, String)> = sqlx::query_as(SELECT_BATCH)
        .bind(LIVENESS_BATCH)
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Ok("no hosted entries to check".into());
    }
    let n = rows.len();
    let probes = stream::iter(rows)
        .map(|(name, url)| {
            let answer = prober(name.clone(), url.clone());
            async move {
                let (state, detail) = answer.await;
                Verdict {
                    name,
                    url,
                    state,
                    detail,
                }
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
    if let Err(e) = sweep_orphans(pg).await {
        tracing::warn!("[mcp/liveness] orphan stamps not swept: {e}");
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
        // Only a changed verdict rewrites its entry, and no write stamps it.
        assert!(
            WRITE_VERDICTS
                .contains("(e.live_state, e.live_detail) is distinct from (v.state, v.detail)")
        );
        assert!(!WRITE_VERDICTS.contains("checked_at"));
        // The stamps follow the same URL guard, and never by an upsert.
        for sql in [RESTAMP, FIRST_STAMPS] {
            assert!(sql.contains("e.registry_name = v.name and e.entry->>'url' = v.url"));
            assert!(!sql.contains("do update"));
        }
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

/// The scratch copy of the stamp table a live test or the census makes in
/// `schema`: `public`'s columns and primary key, and its storage options too
/// (`LIKE` copies no fillfactor), so the copy packs its pages as the
/// migration does.
#[cfg(test)]
pub(crate) async fn create_stamp_copy(admin: &PgPool, schema: &str) {
    let opts: Option<String> = sqlx::query_scalar(
        "select array_to_string(reloptions, ', ') from pg_class \
         where oid = 'public.mcp_library_liveness'::regclass",
    )
    .fetch_one(admin)
    .await
    .expect("public.mcp_library_liveness (a migrated database)");
    let with = opts.map(|o| format!(" with ({o})")).unwrap_or_default();
    let sql = format!(
        "create table {schema}.mcp_library_liveness \
         (like public.mcp_library_liveness including all){with}"
    );
    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .execute(admin)
        .await
        .expect(&sql);
}

// Live-DB proof of what a run writes, through the job's own writer and
// selection: N verdicts commit at most ceil(N / 50) times, each chunk's
// transaction with synchronous_commit off and the connection back to `on`
// after; a verdict that did not change rewrites no entry row, its stamp is a
// HOT update; a flip rewrites exactly its own row; a row re-pointed since its
// probe keeps both its entry and its stamp; the never-asked go first and
// orphan stamps are swept. `#[ignore]`d — they need a database — and CI's
// live-tests job runs them (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-mcp-library --lib -- --ignored liveness_live
//
// Like tests/catalog_live.rs, each builds its own schema (copies of the
// mirror's table and the stamp table) and points a one-connection pool at it
// alone, so the real mirror is never read or written.
#[cfg(test)]
mod liveness_live {
    use std::collections::HashMap;
    use std::str::FromStr;

    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::*;

    const N: usize = 120;
    const STATES: [LiveState; 4] = [
        LiveState::Ok,
        LiveState::Auth,
        LiveState::Dead,
        LiveState::Unknown,
    ];

    /// Copies of the mirror and its stamp table (fillfactor and all) in a
    /// schema of their own, and a one-connection pool that sees only it: the
    /// synchronous_commit read and the table statistics are that session's.
    async fn scratch(tag: &str) -> (PgPool, PgPool, String) {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (a migrated database) to run the ignored live tests");
        let admin = PgPool::connect(&url).await.expect("the database");
        let schema = format!("mcp_liveness_live_{tag}_{}", std::process::id());
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
        create_stamp_copy(&admin, &schema).await;
        let opts = PgConnectOptions::from_str(&url)
            .expect("DATABASE_URL parses")
            .options([("search_path", schema.as_str())]);
        let pg = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("scratch pool");
        (admin, pg, schema)
    }

    async fn drop_scratch(admin: &PgPool, pg: PgPool, schema: &str) {
        pg.close().await;
        let drop = format!("drop schema if exists {schema} cascade");
        sqlx::query(sqlx::AssertSqlSafe(drop.as_str()))
            .execute(admin)
            .await
            .unwrap();
    }

    /// This schema's chunk readings of synchronous_commit, taken out of the
    /// shared hook: the other live tests write theirs beside them.
    fn taken(schema: &str) -> Vec<String> {
        let mut all = CHUNK_SYNCHRONOUS_COMMIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *all)
            .into_iter()
            .partition(|(path, _)| path == schema);
        *all = rest;
        mine.into_iter().map(|(_, s)| s).collect()
    }

    async fn sync_after(pg: &PgPool) -> String {
        sqlx::query_scalar("show synchronous_commit")
            .fetch_one(pg)
            .await
            .unwrap()
    }

    async fn xmins(pg: &PgPool, table: &str) -> HashMap<String, String> {
        let sql = format!("select registry_name, xmin::text from {table}");
        sqlx::query_as::<_, (String, String)>(sqlx::AssertSqlSafe(sql.as_str()))
            .fetch_all(pg)
            .await
            .unwrap()
            .into_iter()
            .collect()
    }

    /// (n_tup_upd, n_tup_hot_upd) of the scratch stamp table, this session's
    /// own counts flushed first.
    async fn stamp_updates(pg: &PgPool) -> (i64, i64) {
        sqlx::query("select pg_stat_force_next_flush()")
            .execute(pg)
            .await
            .unwrap();
        sqlx::query_as(
            "select n_tup_upd, n_tup_hot_upd from pg_stat_user_tables \
             where relid = 'mcp_library_liveness'::regclass",
        )
        .fetch_one(pg)
        .await
        .unwrap()
    }

    fn changed(before: &HashMap<String, String>, after: &HashMap<String, String>) -> Vec<String> {
        let mut out: Vec<String> = after
            .iter()
            .filter(|(n, x)| before.get(*n) != Some(*x))
            .map(|(n, _)| n.clone())
            .collect();
        out.sort();
        out
    }

    #[tokio::test]
    #[ignore]
    async fn liveness_live_n_verdicts_commit_once_a_chunk() {
        let (admin, pg, schema) = scratch("chunks").await;
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

        let verdicts: Vec<Verdict> = (1..=N)
            .map(|i| Verdict {
                name: format!("live/{i}"),
                // Entry 7 was re-pointed while it was being probed.
                url: if i == 7 {
                    "https://moved.test/mcp".into()
                } else {
                    format!("https://live-{i}.test/mcp")
                },
                state: STATES[i % 4],
                detail: format!("detail {i}"),
            })
            .collect();
        let mut seen = 0;
        let written = write_as_they_land(&pg, stream::iter(verdicts), |_| seen += 1).await;
        assert_eq!(seen, N, "the tally sees every verdict");
        assert_eq!(written, (N - 1) as u64, "every row but the re-pointed one");
        assert_eq!(
            taken(&schema),
            vec!["off"; N.div_ceil(VERDICT_CHUNK)],
            "each chunk commits without waiting on its flush"
        );

        // Every first stamp is an insert in its chunk's transaction.
        let commits: i64 =
            sqlx::query_scalar("select count(distinct xmin::text) from mcp_library_liveness")
                .fetch_one(&pg)
                .await
                .unwrap();
        assert!(
            commits as usize <= N.div_ceil(VERDICT_CHUNK),
            "{commits} commits for {N} verdicts"
        );
        assert_eq!(commits, 3);

        let rows: Vec<(String, Option<String>, Option<String>, bool)> = sqlx::query_as(
            "select e.registry_name, e.live_state, e.live_detail, l.registry_name is not null \
             from mcp_library_entries e \
             left join mcp_library_liveness l on l.registry_name = e.registry_name",
        )
        .fetch_all(&pg)
        .await
        .unwrap();
        assert_eq!(rows.len(), N);
        for (name, state, detail, stamped) in rows {
            let i: usize = name.trim_start_matches("live/").parse().unwrap();
            if i == 7 {
                assert_eq!((state, detail, stamped), (None, None, false), "{name}");
            } else {
                assert_eq!(state.as_deref(), Some(STATES[i % 4].as_str()), "{name}");
                assert_eq!(detail, Some(format!("detail {i}")), "{name}");
                assert!(stamped, "{name}");
            }
        }
        // The live column the previous release stamped stays as it was.
        let old: i64 = sqlx::query_scalar(
            "select count(*) from mcp_library_entries where live_checked_at is not null",
        )
        .fetch_one(&pg)
        .await
        .unwrap();
        assert_eq!(old, 0, "live_checked_at is no longer written");

        assert_eq!(
            sync_after(&pg).await,
            "on",
            "synchronous_commit = off stayed inside the chunks"
        );
        drop_scratch(&admin, pg, &schema).await;
    }

    #[tokio::test]
    #[ignore]
    async fn liveness_live_unchanged_verdicts_write_no_entry_rows() {
        let (admin, pg, schema) = scratch("unchanged").await;
        // A mirror at steady state: every entry carries its verdict, and its
        // stamp from an hour ago sits in name order, as the backfill leaves
        // it. A pass asks one entry in six (as a run asks a slice of each
        // page), names of a realistic length.
        const M: i32 = 720;
        let name = |i: usize| format!("io.example.live-{i:04}/an-mcp-server-with-a-realistic-name");
        sqlx::query(
            "insert into mcp_library_entries (registry_name, title, kind, entry, live_state, live_detail) \
             select 'io.example.live-' || lpad(g::text, 4, '0') || '/an-mcp-server-with-a-realistic-name', \
                    'Live ' || g, 'hosted', \
                    jsonb_build_object('url', 'https://live-' || g || '.test/mcp'), \
                    (array['ok', 'auth', 'dead', 'unknown'])[1 + g % 4], 'detail ' || g \
             from generate_series(1, $1) g",
        )
        .bind(M)
        .execute(&pg)
        .await
        .unwrap();
        sqlx::query(
            "insert into mcp_library_liveness (registry_name, checked_at) \
             select registry_name, now() - interval '1 hour' from mcp_library_entries \
             order by registry_name",
        )
        .execute(&pg)
        .await
        .unwrap();
        let probed: Vec<usize> = (1..=M as usize).filter(|i| i % 6 == 0).collect();
        assert_eq!(probed.len(), N);
        let verdict = |i: usize, url: String, state: LiveState, detail: String| Verdict {
            name: name(i),
            url,
            state,
            detail,
        };
        // Landed out of order, as probes do.
        let mut order = probed.clone();
        order.sort_by_key(|i| (i * 37) % 101);
        let same: Vec<Verdict> = order
            .iter()
            .map(|&i| {
                verdict(
                    i,
                    format!("https://live-{i}.test/mcp"),
                    STATES[i % 4],
                    format!("detail {i}"),
                )
            })
            .collect();

        // The same answers as last time: no entry row is written, every
        // stamp is re-stamped in place, three async commits.
        let entries0 = xmins(&pg, "mcp_library_entries").await;
        let stamps0 = xmins(&pg, "mcp_library_liveness").await;
        let (upd0, hot0) = stamp_updates(&pg).await;
        let written = write_as_they_land(&pg, stream::iter(same), |_| {}).await;
        assert_eq!(written, N as u64);
        let (upd1, hot1) = stamp_updates(&pg).await;
        let entries1 = xmins(&pg, "mcp_library_entries").await;
        let stamps1 = xmins(&pg, "mcp_library_liveness").await;
        assert_eq!(
            changed(&entries0, &entries1),
            Vec::<String>::new(),
            "an unchanged verdict rewrites no entry row"
        );
        assert_eq!(upd1 - upd0, N as i64, "every probed entry is re-stamped");
        assert_eq!(hot1 - hot0, upd1 - upd0, "every re-stamp is a HOT update");
        let restamped = changed(&stamps0, &stamps1);
        let mut want: Vec<String> = probed.iter().map(|&i| name(i)).collect();
        want.sort();
        assert_eq!(restamped, want, "the probed stamps, and only those");
        let commits: std::collections::HashSet<&String> =
            want.iter().map(|n| &stamps1[n]).collect();
        assert!(commits.len() <= N.div_ceil(VERDICT_CHUNK), "{commits:?}");
        assert_eq!(taken(&schema), vec!["off"; N.div_ceil(VERDICT_CHUNK)]);
        assert_eq!(sync_after(&pg).await, "on");

        // One verdict flips and another entry was re-pointed mid-probe: the
        // flip rewrites exactly its own entry row, the re-pointed entry keeps
        // its row and its stamp.
        let (flip, moved) = (probed[0], probed[1]);
        let checked_at = |n: String| {
            let pg = pg.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "select checked_at::text from mcp_library_liveness where registry_name = $1",
                )
                .bind(n)
                .fetch_one(&pg)
                .await
                .unwrap()
            }
        };
        let moved_stamp = checked_at(name(moved)).await;
        let next: Vec<Verdict> = probed
            .iter()
            .map(|&i| {
                if i == flip {
                    let s = STATES[(i + 1) % 4];
                    verdict(i, format!("https://live-{i}.test/mcp"), s, "flipped".into())
                } else if i == moved {
                    verdict(
                        i,
                        "https://moved.test/mcp".into(),
                        LiveState::Dead,
                        "moved".into(),
                    )
                } else {
                    verdict(
                        i,
                        format!("https://live-{i}.test/mcp"),
                        STATES[i % 4],
                        format!("detail {i}"),
                    )
                }
            })
            .collect();
        let written = write_as_they_land(&pg, stream::iter(next), |_| {}).await;
        assert_eq!(
            written,
            (N - 1) as u64,
            "every verdict but the re-pointed one"
        );
        let entries2 = xmins(&pg, "mcp_library_entries").await;
        let stamps2 = xmins(&pg, "mcp_library_liveness").await;
        assert_eq!(changed(&entries1, &entries2), vec![name(flip)]);
        assert_eq!(stamps2[&name(moved)], stamps1[&name(moved)]);
        assert_eq!(checked_at(name(moved)).await, moved_stamp);
        let (state, detail): (Option<String>, Option<String>) = sqlx::query_as(
            "select live_state, live_detail from mcp_library_entries where registry_name = $1",
        )
        .bind(name(flip))
        .fetch_one(&pg)
        .await
        .unwrap();
        assert_eq!(state.as_deref(), Some(STATES[(flip + 1) % 4].as_str()));
        assert_eq!(detail.as_deref(), Some("flipped"));
        taken(&schema);
        drop_scratch(&admin, pg, &schema).await;
    }

    #[tokio::test]
    #[ignore]
    async fn liveness_live_a_run_asks_the_unstamped_first_and_sweeps_orphans() {
        let (admin, pg, schema) = scratch("run").await;
        // b was asked an hour ago, a a minute ago, c never; a registry row
        // and an operator stand-in both unasked: the stand-in goes first.
        // A package and a plain-http endpoint are never asked.
        sqlx::query(
            "insert into mcp_library_entries (registry_name, title, kind, source, entry) values \
               ('run/a', 'A', 'hosted', 'registry', '{\"url\":\"https://a.test/mcp\"}'), \
               ('run/b', 'B', 'hosted', 'registry', '{\"url\":\"https://b.test/mcp\"}'), \
               ('run/c', 'C', 'hosted', 'registry', '{\"url\":\"https://c.test/mcp\"}'), \
               ('op:d.test', 'D', 'hosted', 'operator', '{\"url\":\"https://d.test/mcp\"}'), \
               ('run/pkg', 'P', 'package', 'registry', '{\"url\":\"pkg:npm/p\"}'), \
               ('run/http', 'H', 'hosted', 'registry', '{\"url\":\"http://h.test/mcp\"}')",
        )
        .execute(&pg)
        .await
        .unwrap();
        sqlx::query(
            "insert into mcp_library_liveness (registry_name, checked_at) values \
               ('run/a', now() - interval '1 minute'), ('run/b', now() - interval '1 hour'), \
               ('run/gone', now() - interval '2 hours')",
        )
        .execute(&pg)
        .await
        .unwrap();
        let order: Vec<String> = sqlx::query_scalar(SELECT_BATCH)
            .bind(LIVENESS_BATCH)
            .fetch_all(&pg)
            .await
            .unwrap();
        assert_eq!(order, ["op:d.test", "run/c", "run/b", "run/a"]);

        let report = run_batch_with(&pg, |_name, _url| async {
            (
                LiveState::Auth,
                "HTTP 401 — answers, wants credentials".to_string(),
            )
        })
        .await
        .unwrap();
        assert!(report.starts_with("checked 4: 0 ok, 4 auth"), "{report}");
        let stamped: Vec<String> = sqlx::query_scalar(
            "select registry_name from mcp_library_liveness order by registry_name",
        )
        .fetch_all(&pg)
        .await
        .unwrap();
        assert_eq!(
            stamped,
            ["op:d.test", "run/a", "run/b", "run/c"],
            "every asked entry stamped, the orphan swept"
        );
        assert_eq!(
            sweep_orphans(&pg).await.unwrap(),
            0,
            "nothing left to sweep"
        );
        taken(&schema);
        drop_scratch(&admin, pg, &schema).await;
    }
}

// Live CENSUS of the job's writes — a measurement, not a behavior test: one
// simulated hour (12 runs) of the job's own selection and writer over a
// production-sized synthetic mirror, printed for the pull request that
// changes them. The shape is a production instance's with none of its
// content: 65,133 entries, 45,685 of them hosted https endpoints, ~1 kB of
// entry each, every probe-able row already carrying a verdict and a stamp
// spread over one rotation (152 runs). A stand-in prober flips a fixed share
// of the verdicts it is asked for (0%, 3%, 10%), with no network.
//
// CHECKPOINT is the clock: one before every run at stock settings (one run
// per 5-minute checkpoint), one every third run under a 15-minute
// `checkpoint_timeout`. The first touch of a page after a checkpoint is a
// full-page image, and those are what a slow disk pays for.
//
// Per run it reads: write transactions (the `pg_snapshot_xmax` delta, which
// takes no xid itself), how many of them committed with synchronous_commit
// off (CHUNK_SYNCHRONOUS_COMMIT), WAL bytes (the `pg_current_wal_lsn` delta),
// `pg_stat_wal`'s records and full-page images, and the entry rows rewritten
// (rows whose xmin moved). The WAL counters are the cluster's, so it wants a
// database nothing else is connected to, and a role allowed to CHECKPOINT. It
// returns at once without TALARIA_WRITE_CENSUS=1, so CI's --ignored step
// passes over it:
//
//   TALARIA_WRITE_CENSUS=1 DATABASE_URL=postgres://… \
//     cargo test -p talaria-mcp-library --lib -- --ignored --nocapture liveness_live_census
#[cfg(test)]
mod liveness_census {
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::*;
    use crate::catalog::{FEATURED_SQL, SEARCH_PAGE_SQL};
    use crate::featured::{FeaturedConfig, merged_domains};

    const ENTRIES: i32 = 65_133;
    const HOSTED_HTTPS: i32 = 45_685;
    const RUNS: usize = 12;
    const SCHEMA: &str = "mcp_liveness_census";
    const STATES: [LiveState; 4] = [
        LiveState::Ok,
        LiveState::Auth,
        LiveState::Dead,
        LiveState::Unknown,
    ];

    fn detail_of(s: LiveState) -> &'static str {
        match s {
            LiveState::Ok => "HTTP 200 — answered initialize",
            LiveState::Auth => "HTTP 401 — answers, wants credentials",
            LiveState::Dead => "HTTP 404 — no MCP endpoint here",
            LiveState::Unknown => "HTTP 400",
        }
    }

    /// FNV-1a: fixed and dependency-free, so every census flips the same rows.
    fn fnv(bytes: &[u8], seed: u64) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64 ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    /// The census runs only when asked.
    fn census_url() -> Option<String> {
        if std::env::var("TALARIA_WRITE_CENSUS").ok().as_deref() != Some("1") {
            eprintln!("census skipped: set TALARIA_WRITE_CENSUS=1 and DATABASE_URL to run it");
            return None;
        }
        Some(std::env::var("DATABASE_URL").expect("DATABASE_URL for the census"))
    }

    #[derive(Clone, Copy, Default)]
    struct Mark {
        xmax: i64,
        lsn: i64,
        records: i64,
        fpi: i64,
    }

    /// The cluster's counters, read on the one connection that wrote: its
    /// own WAL statistics are flushed first so `pg_stat_wal` includes them.
    async fn mark(pg: &PgPool) -> Mark {
        sqlx::query("select pg_stat_force_next_flush()")
            .execute(pg)
            .await
            .unwrap();
        let (xmax, lsn, records, fpi): (i64, i64, i64, i64) = sqlx::query_as(
            "select pg_snapshot_xmax(pg_current_snapshot())::text::int8, \
               pg_wal_lsn_diff(pg_current_wal_lsn(), '0/0')::int8, \
               w.wal_records, w.wal_fpi from pg_stat_wal w",
        )
        .fetch_one(pg)
        .await
        .unwrap();
        Mark {
            xmax,
            lsn,
            records,
            fpi,
        }
    }

    /// (n_tup_upd, n_tup_hot_upd) of a scratch table, after a flush.
    async fn updates(pg: &PgPool, table: &str) -> (i64, i64) {
        sqlx::query("select pg_stat_force_next_flush()")
            .execute(pg)
            .await
            .unwrap();
        sqlx::query_as(
            "select coalesce(n_tup_upd, 0), coalesce(n_tup_hot_upd, 0) \
             from pg_stat_user_tables where relid = to_regclass($1)",
        )
        .bind(table)
        .fetch_optional(pg)
        .await
        .unwrap()
        .unwrap_or((0, 0))
    }

    async fn xmins(pg: &PgPool) -> HashMap<String, String> {
        sqlx::query_as::<_, (String, String)>(
            "select registry_name, xmin::text from mcp_library_entries",
        )
        .fetch_all(pg)
        .await
        .unwrap()
        .into_iter()
        .collect()
    }

    async fn exec(pg: &PgPool, sql: &str) {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(pg)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    /// A fresh scratch schema holding the mirror at steady state, and a
    /// one-connection pool that sees only it. Whether the stamp lives in
    /// its own table is the migrated database's answer: the scratch copy
    /// follows `public`.
    async fn seed(admin: &PgPool, url: &str) -> (PgPool, bool) {
        exec(admin, &format!("drop schema if exists {SCHEMA} cascade")).await;
        exec(admin, &format!("create schema {SCHEMA}")).await;
        exec(
            admin,
            &format!(
                "create table {SCHEMA}.mcp_library_entries \
                 (like public.mcp_library_entries including all)"
            ),
        )
        .await;
        let (side,): (bool,) =
            sqlx::query_as("select to_regclass('public.mcp_library_liveness') is not null")
                .fetch_one(admin)
                .await
                .unwrap();
        if side {
            create_stamp_copy(admin, SCHEMA).await;
        }
        let opts = PgConnectOptions::from_str(url)
            .expect("DATABASE_URL parses")
            .options([("search_path", SCHEMA)]);
        let pg = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("census pool");
        sqlx::query(SEED_ENTRIES)
            .bind(ENTRIES)
            .bind(HOSTED_HTTPS)
            .execute(&pg)
            .await
            .expect("seed the entries");
        if side {
            // The migration's own backfill, in rotation order.
            exec(
                &pg,
                "insert into mcp_library_liveness (registry_name, checked_at) \
                 select registry_name, live_checked_at from mcp_library_entries \
                  where live_checked_at is not null \
                  order by live_checked_at, (source <> 'registry') desc, registry_name \
                 on conflict do nothing",
            )
            .await;
            exec(&pg, "vacuum (analyze) mcp_library_liveness").await;
        }
        exec(&pg, "vacuum (analyze) mcp_library_entries").await;
        (pg, side)
    }

    /// The synthetic mirror. Words come from a fixed vocabulary picked by
    /// hash, so the text search has realistic hits; the physical order is
    /// the names' md5, unrelated to the rotation; each probe-able row's
    /// stamp is one of the rotation's 5-minute steps.
    const SEED_ENTRIES: &str = "\
        with v as (select string_to_array('github slack notion postgres weather search calendar \
           email files docs browser memory sql vector maps finance crypto stripe payments jira linear \
           asana trello figma design image video audio speech translate news sports music spotify \
           youtube reddit discord telegram sms twilio mailchimp hubspot salesforce zendesk intercom \
           shopify inventory shipping travel flights hotels booking food recipes health fitness legal \
           contracts invoices accounting tax recruiting jobs learning courses math science climate \
           energy solar home automation security identity vault secrets kubernetes docker cloud \
           storage backup sync drive photos camera vision agent prompt embeddings chat assistant \
           voice meeting analytics metrics logs tracing incident status uptime monitor scraper \
           crawler feeds podcast books wiki knowledge notes todo tasks projects tracking crm sales \
           marketing seo ads social content writing blog markdown pdf sheets slides charts diagrams \
           code git review testing deploy build package python rust terminal shell', ' ') as w), \
        g as ( \
          select i, \
            'io.census.' || substr(md5('ns' || i), 1, 8) || '/' || substr(md5('nm' || i), 1, 12) \
              || '-mcp' as name, \
            case when i <= $2 then 'https' when i <= $2 + 1500 then 'http' else 'pkg' end as shape, \
            v.w[1 + abs(hashtext('a' || i)) % cardinality(v.w)] as w1, \
            v.w[1 + abs(hashtext('b' || i)) % cardinality(v.w)] as w2, \
            (select string_agg(v.w[1 + abs(hashtext(k || '-' || i)) % cardinality(v.w)], ' ') \
               from generate_series(1, 27) k) as descr \
          from generate_series(1, $1) i, v), \
        r as ( \
          select g.*, \
            w1 || '-' || substr(md5('d' || i), 1, 6) || '.com' as dom, \
            case when shape = 'https' then row_number() over ( \
              partition by shape = 'https' order by md5(name || 'r')) - 1 end as rot, \
            abs(hashtext(name || 's')) % 100 as bucket \
          from g), \
        e as ( \
          select r.*, \
            case shape when 'https' then 'https://mcp.' || dom || '/mcp' \
                       when 'http' then 'http://mcp.' || dom || '/mcp' \
                       else 'pkg:npm/@' || w1 || '/' || substr(md5('p' || i), 1, 10) end as url, \
            case when i % 20 = 0 then 0 when i % 5 = 0 then 1 else 2 end::int2 as tier, \
            case when shape <> 'https' then null when bucket < 12 then 'ok' \
                 when bucket < 47 then 'auth' when bucket < 92 then 'dead' else 'unknown' end as state \
          from r) \
        insert into mcp_library_entries (registry_name, source, title, description, domain, domains, \
          tier, kind, entry, registry_updated_at, synced_at, live_state, live_checked_at, live_detail) \
        select name, case when i % 500 = 0 then 'well-known' else 'registry' end, \
          initcap(w1) || ' ' || initcap(w2), descr, dom, \
          array[dom, 'mcp.' || dom, w1 || '.io'], tier, \
          case shape when 'pkg' then 'package' else 'hosted' end, \
          jsonb_build_object('registryName', name, 'title', initcap(w1) || ' ' || initcap(w2), \
            'description', descr, 'url', url, 'domain', dom, 'icon', null, \
            'tier', case tier when 0 then 'first-party' when 1 then 'verified' else 'community' end, \
            'requiredHeaders', jsonb_build_array(jsonb_build_object('name', 'Authorization', \
              'description', 'A bearer token from your ' || initcap(w1) || ' account settings', \
              'isRequired', true, 'isSecret', true, \
              'placeholder', 'Bearer …', 'default', null, 'choices', null, 'value', null, \
              'variables', null)), \
            'package', null, \
            'remotes', jsonb_build_array(jsonb_build_object('type', 'streamable-http', 'url', url)), \
            'version', '1.' || (i % 17) || '.' || (i % 5), \
            'repository', 'https://github.com/' || w1 || '/' || substr(md5('p' || i), 1, 10)), \
          now() - (i % 900) * interval '1 hour', now(), state, \
          case when state is null then null \
               else now() - (1 + rot / 300) * interval '5 minutes' end, \
          case state when 'ok' then 'HTTP 200 — answered initialize' \
                     when 'auth' then 'HTTP 401 — answers, wants credentials' \
                     when 'dead' then 'HTTP 404 — no MCP endpoint here' \
                     when 'unknown' then 'HTTP 400' end \
        from e order by md5(name)";

    #[derive(Default)]
    struct Hour {
        checkpoints: usize,
        xids: i64,
        asynchronous: usize,
        wal: i64,
        records: i64,
        fpi: i64,
        rewritten: usize,
        entry_upd: i64,
        entry_hot: i64,
        stamp_upd: i64,
        stamp_hot: i64,
    }

    #[tokio::test]
    #[ignore]
    async fn liveness_live_census() {
        let Some(url) = census_url() else { return };
        let admin = PgPool::connect(&url).await.expect("the database");
        let (timeout,): (String,) = sqlx::query_as("show checkpoint_timeout")
            .fetch_one(&admin)
            .await
            .unwrap();
        let (compression,): (String,) = sqlx::query_as("show wal_compression")
            .fetch_one(&admin)
            .await
            .unwrap();
        // One run is 5 minutes: a checkpoint per run at the stock 5min, one
        // per three runs at 15min.
        let every = match timeout.as_str() {
            "15min" => 3,
            "5min" => 1,
            other => panic!("checkpoint_timeout {other}: the census knows 5min and 15min"),
        };
        println!(
            "census liveness: checkpoint_timeout {timeout}, wal_compression {compression}, \
             {ENTRIES} entries, {HOSTED_HTTPS} hosted https, {RUNS} runs of {LIVENESS_BATCH}"
        );
        let mut last: Option<PgPool> = None;
        for flip in [0u64, 30, 100] {
            if let Some(pg) = last.take() {
                pg.close().await;
            }
            let (pg, side) = seed(&admin, &url).await;
            let stored: Vec<(String, String)> = sqlx::query_as(
                "select registry_name, live_state from mcp_library_entries \
                 where live_state is not null",
            )
            .fetch_all(&pg)
            .await
            .unwrap();
            let states: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(
                stored
                    .into_iter()
                    .map(|(n, s)| {
                        let i = STATES.iter().position(|x| x.as_str() == s).unwrap();
                        (n, i)
                    })
                    .collect(),
            ));
            let mut hour = Hour::default();
            for run in 0..RUNS {
                if run % every == 0 {
                    exec(&pg, "checkpoint").await;
                    hour.checkpoints += 1;
                }
                let before_rows = xmins(&pg).await;
                let (eu0, eh0) = updates(&pg, "mcp_library_entries").await;
                let (su0, sh0) = updates(&pg, "mcp_library_liveness").await;
                CHUNK_SYNCHRONOUS_COMMIT
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clear();
                let m0 = mark(&pg).await;
                let st = states.clone();
                let report = run_batch_with(&pg, move |name: String, _url: String| {
                    let mut st = st.lock().unwrap();
                    let cur = st.get(&name).copied().unwrap_or(2);
                    let now = if fnv(name.as_bytes(), run as u64) % 1000 < flip {
                        (cur + 1) % STATES.len()
                    } else {
                        cur
                    };
                    st.insert(name, now);
                    let s = STATES[now];
                    std::future::ready((s, detail_of(s).to_string()))
                })
                .await
                .expect("a run");
                let m1 = mark(&pg).await;
                let (eu1, eh1) = updates(&pg, "mcp_library_entries").await;
                let (su1, sh1) = updates(&pg, "mcp_library_liveness").await;
                let after_rows = xmins(&pg).await;
                let rewritten = after_rows
                    .iter()
                    .filter(|(n, x)| before_rows.get(*n) != Some(*x))
                    .count();
                let asynchronous = CHUNK_SYNCHRONOUS_COMMIT
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .filter(|(path, s)| path == SCHEMA && s.as_str() == "off")
                    .count();
                println!(
                    "  flip {:>4.1}% run {:>2}: {report}; xids {}, async {asynchronous}, wal {} B, \
                     records {}, fpi {}, entry rows rewritten {rewritten} (upd {} hot {}), \
                     stamp upd {} hot {}",
                    flip as f64 / 10.0,
                    run + 1,
                    m1.xmax - m0.xmax,
                    m1.lsn - m0.lsn,
                    m1.records - m0.records,
                    m1.fpi - m0.fpi,
                    eu1 - eu0,
                    eh1 - eh0,
                    su1 - su0,
                    sh1 - sh0,
                );
                hour.xids += m1.xmax - m0.xmax;
                hour.asynchronous += asynchronous;
                hour.wal += m1.lsn - m0.lsn;
                hour.records += m1.records - m0.records;
                hour.fpi += m1.fpi - m0.fpi;
                hour.rewritten += rewritten;
                hour.entry_upd += eu1 - eu0;
                hour.entry_hot += eh1 - eh0;
                hour.stamp_upd += su1 - su0;
                hour.stamp_hot += sh1 - sh0;
            }
            println!(
                "census liveness per hour | flip {:.1}% | side table {side} | checkpoints {} | \
                 write xacts {} | async {} | WAL {:.2} MB ({} B) | records {} | FPI {} | \
                 entry rows rewritten {} (upd {}, HOT {}) | stamp upd {} (HOT {})",
                flip as f64 / 10.0,
                hour.checkpoints,
                hour.xids,
                hour.asynchronous,
                hour.wal as f64 / 1e6,
                hour.wal,
                hour.records,
                hour.fpi,
                hour.rewritten,
                hour.entry_upd,
                hour.entry_hot,
                hour.stamp_upd,
                hour.stamp_hot,
            );
            last = Some(pg);
        }

        // The read side on the same table: the picker's two keystroke
        // queries, the shelf and the run's own selection.
        let pg = last.expect("a census pool");
        let domains = merged_domains(&FeaturedConfig::default());
        for (label, query, limit) in [
            ("search page, empty query", "", 60i64),
            ("search page, one word", "weather", 60i64),
        ] {
            let sql = format!("explain (analyze, buffers) {SEARCH_PAGE_SQL}");
            let mut lines: Vec<String> = Vec::new();
            for _ in 0..3 {
                lines = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
                    .bind(query)
                    .bind(None::<i16>)
                    .bind(None::<&str>)
                    .bind(limit)
                    .bind(false)
                    .fetch_all(&pg)
                    .await
                    .unwrap();
            }
            println!("census explain | {label}\n{}", lines.join("\n"));
        }
        let sql = format!("explain (analyze, buffers) {FEATURED_SQL}");
        let mut lines: Vec<String> = Vec::new();
        for _ in 0..3 {
            lines = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
                .bind(&domains)
                .fetch_all(&pg)
                .await
                .unwrap();
        }
        println!("census explain | featured\n{}", lines.join("\n"));
        let sql = format!("explain (analyze, buffers) {SELECT_BATCH}");
        for _ in 0..3 {
            lines = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
                .bind(LIVENESS_BATCH)
                .fetch_all(&pg)
                .await
                .unwrap();
        }
        println!("census explain | run selection\n{}", lines.join("\n"));
        pg.close().await;
        exec(&admin, &format!("drop schema if exists {SCHEMA} cascade")).await;
    }
}
