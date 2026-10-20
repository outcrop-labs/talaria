// The gateway's per-call record — `mcp_tool_calls`, one row per tools/call
// the gateway answers, so "which agent called which tool, how often, and did
// it fail" has an answer that is not a log grep.
//
// WHAT A ROW HOLDS, AND WHAT IT NEVER DOES. Who (agent, and the acting human
// on a personal assistant), where (server id + name), what (the tool name,
// capped at 200 chars), how it went (an outcome from a closed set, the HTTP
// status the gateway answered with, the latency), and — on a failure — the
// one sentence the caller or the upstream was given, capped at 300 chars.
// Never the call's arguments, never its result, never a header or a token:
// arguments carry credentials the spend boundary just put there, and results
// carry whatever the tool read. A tool that reports `isError: true` is
// recorded as an error with a fixed sentence, because its text IS a result.
//
// AN UPSTREAM'S SENTENCE CAN ECHO THE ARGUMENTS. A validator that rejects an
// argument quotes it back ("Expected 'a' | 'b', received 'ghp_…'"), so the
// error column is the one place arguments could leak in. Two rules close it:
//   · a call the spend boundary put a credential into keeps NO upstream- or
//     dispatcher-authored text — only the JSON-RPC code and a fixed sentence
//     (`SPENT_REDACTED`): the real value is exactly what the reply may quote;
//   · every other error has each argument string the agent sent (6+ chars)
//     replaced with `«argument»` before it is stored (`scrub_arguments`).
//
// THE HOT PATH NEVER WAITS ON THIS. `CallLog::finish` spawns the insert on a
// cloned pool and returns. The insert is ONE statement — the owner lookup
// (the acting human) and, for a refusal, the server-id lookup ride inside it
// as indexed subqueries — and at most `INSERT_CONCURRENCY` of them hold a
// pool connection at once, so a burst of calls cannot starve the gateway of
// connections; past `INSERT_BACKLOG` waiting rows, new rows are dropped with
// a warning. A failed insert warns and is gone — losing a log row is the
// right trade against slowing or failing a tool call. A streamed answer is
// recorded when its stream ENDS (`TappedStream`), so latency is the whole call
// and the outcome is what the upstream actually replied, without buffering
// the relay.
//
// Only tools/call is recorded: initialize, tools/list, ping and notifications
// are protocol chatter whose volume would drown the calls. Inside a JSON-RPC
// batch every tools/call member is its own row (`recorded_members`) — the
// gateway refuses a batch carrying one (it would skip the tool gate and the
// spend boundary), and that refusal is recorded per member.
//
// Retention: the leased `mcp-calls-prune` job deletes rows older than
// app_settings `mcp_calls_retention_days` (default 30) every six hours; the
// admin route PUT /api/mcp/calls/retention sets it.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use std::time::Instant;

use futures_util::Stream;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model, subject_proven};

const LOG: &str = "[mcp/calls]";

/// The longest error sentence a row keeps, in characters.
pub const ERROR_MAX_CHARS: usize = 300;
/// The longest tool name a row keeps, in characters. The name is the
/// agent's to choose, so the agent must not choose how much a row stores.
pub const TOOL_MAX_CHARS: usize = 200;
/// The app_settings key the prune job reads, and its default in days.
pub const RETENTION_KEY: &str = "mcp_calls_retention_days";
pub const DEFAULT_RETENTION_DAYS: i64 = 30;
/// The longest retention the setting can ask for (ten years).
pub const MAX_RETENTION_DAYS: i64 = 3650;
/// The list route's page: newest first, 100 unless asked, never over 500.
pub const DEFAULT_LIMIT: i64 = 100;
pub const MAX_LIMIT: i64 = 500;
/// How many tools a server's summary names.
pub const TOP_TOOLS: usize = 5;

/// The sentence a tool-reported failure is recorded with — its own text is a
/// result, and results are never kept.
pub const TOOL_FAILED: &str =
    "the tool reported an error (isError: true); its text is a result and is not kept";
/// An SSE answer that closed without a reply to the call.
pub const STREAM_NO_REPLY: &str = "the upstream's event stream ended without a reply to this call";
/// The upstream's body broke off mid-answer.
pub const STREAM_BROKE: &str = "the upstream's answer broke off mid-stream";
/// The caller went away before the answer finished.
pub const CALLER_GONE: &str = "the caller disconnected before the answer finished";
/// What replaces an upstream's own sentence on a call that carried a spent
/// credential: the reply may quote the real value back.
pub const SPENT_REDACTED: &str =
    "its message is not kept: the call carried a spent credential the reply may quote";
/// What an echoed argument becomes in a stored sentence.
pub const ARGUMENT_MARK: &str = "«argument»";
/// The gateway refused a batch that carries a tools/call.
pub const BATCH_REFUSED: &str = "a JSON-RPC batch carrying tools/call or tools/list is not accepted here; send each request on its own";
/// Access resolution failed inside the gateway before the call went anywhere.
pub const ACCESS_FAILED: &str =
    "the gateway could not resolve this agent's access to the server (internal error)";

/// How a call went. A closed set: the routes validate a filter against it and
/// the UI colors by it, so a new outcome is a deliberate change in three places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The tool answered.
    Ok,
    /// A JSON-RPC error, a tool-reported failure, or a house refusal inside an
    /// in-process dispatcher — the call reached something that said no.
    Error,
    /// The gateway refused: no access to the server, or the tool is outside
    /// the allowed set.
    Denied,
    /// Granted, but the account it would speak as is not connected.
    NotConnected,
    /// The upstream could not be reached, refused at HTTP level, or broke off.
    UpstreamError,
    /// The credential spend boundary failed; nothing was forwarded.
    SpendError,
    /// The caller hung up before the answer finished — the client's choice,
    /// so it is counted on its own, never as the server's error.
    Cancelled,
}

impl Outcome {
    pub const ALL: [Outcome; 7] = [
        Outcome::Ok,
        Outcome::Error,
        Outcome::Denied,
        Outcome::NotConnected,
        Outcome::UpstreamError,
        Outcome::SpendError,
        Outcome::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Denied => "denied",
            Outcome::NotConnected => "not_connected",
            Outcome::UpstreamError => "upstream_error",
            Outcome::SpendError => "spend_error",
            Outcome::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Outcome> {
        Outcome::ALL.into_iter().find(|o| o.as_str() == s)
    }

    /// The outcomes the summary counts as errors. Refusals, missing
    /// connections and cancellations are counted on their own: they are
    /// policy, setup and the client's choice, not a server misbehaving.
    pub fn is_error(self) -> bool {
        matches!(
            self,
            Outcome::Error | Outcome::UpstreamError | Outcome::SpendError
        )
    }

    /// Did the call reach the server? Only these carry a latency worth a
    /// percentile: the gateway answers a refusal or a missing connection
    /// itself in a few milliseconds, which would drag a server's p50 down.
    pub fn reached_server(self) -> bool {
        !matches!(self, Outcome::Denied | Outcome::NotConnected)
    }

    /// The `reached_server` complement as wire strings, for SQL.
    pub fn gateway_answered_strs() -> Vec<&'static str> {
        Outcome::ALL
            .into_iter()
            .filter(|o| !o.reached_server())
            .map(Outcome::as_str)
            .collect()
    }

    /// The `is_error` set as wire strings, for SQL `= any($n::text[])`.
    pub fn error_strs() -> Vec<&'static str> {
        Outcome::ALL
            .into_iter()
            .filter(|o| o.is_error())
            .map(Outcome::as_str)
            .collect()
    }
}

/// `s` trimmed and at most `max` characters — a longer one keeps its first
/// `max - 1` and an ellipsis, so a truncated value says it was truncated.
/// Counted in chars, never bytes, so multibyte text is never cut
/// mid-character.
fn truncate_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The error sentence as a row keeps it: trimmed, at most `ERROR_MAX_CHARS`.
pub fn truncate_error(s: &str) -> String {
    truncate_chars(s, ERROR_MAX_CHARS)
}

/// The method of a single JSON-RPC request; None for a batch, a non-object,
/// or an unparsed body.
fn method_of(rpc: Option<&Value>) -> Option<&str> {
    rpc?.as_object()?.get("method")?.as_str()
}

/// Is this a request the record keeps? Exactly a single tools/call — a
/// batch's members are handed over one by one (`recorded_members`).
pub fn is_recorded(rpc: Option<&Value>) -> bool {
    method_of(rpc) == Some("tools/call")
}

/// The tools/calls a body carries: the body itself when it is one, each
/// tools/call member when it is a batch, nothing otherwise.
pub fn recorded_members(rpc: Option<&Value>) -> Vec<&Value> {
    match rpc {
        Some(Value::Array(members)) => members.iter().filter(|m| is_recorded(Some(m))).collect(),
        Some(single) if is_recorded(Some(single)) => vec![single],
        _ => Vec::new(),
    }
}

/// Does this batch carry a request the gateway must inspect — a tools/call
/// (the tool gate, the spend boundary) or a tools/list (the catalog filter)?
/// Those only read a single-object body, so a batch carrying one would ride
/// past them; the gateway refuses it instead.
pub fn batch_needs_refusal(rpc: Option<&Value>) -> bool {
    match rpc {
        Some(Value::Array(members)) => members
            .iter()
            .any(|m| matches!(method_of(Some(m)), Some("tools/call" | "tools/list"))),
        _ => false,
    }
}

/// The called tool's name, when the call names one as a string — capped at
/// `TOOL_MAX_CHARS`.
pub fn tool_of(rpc: Option<&Value>) -> Option<String> {
    rpc?.pointer("/params/name")?
        .as_str()
        .map(|t| truncate_chars(t, TOOL_MAX_CHARS))
}

/// Argument strings shorter than this are never scrubbed: "a", "true" or
/// "en" appear in ordinary sentences, and blanking them would mangle every
/// message while hiding nothing worth hiding.
const SCRUB_MIN_CHARS: usize = 6;
/// At most this much argument text is kept to scrub with, so a megabyte
/// argument costs a bounded copy (a sentence is 300 chars after all).
const SCRUB_BUDGET_BYTES: usize = 64 * 1024;

/// Every string value under the call's `params.arguments`, longest first
/// (so a value containing another is replaced whole), within the budget.
pub fn argument_strings(rpc: Option<&Value>) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>, budget: &mut usize) {
        match v {
            Value::String(s) if s.trim().chars().count() >= SCRUB_MIN_CHARS => {
                let s = s.trim();
                if s.len() <= *budget {
                    *budget -= s.len();
                    out.push(s.to_string());
                }
            }
            Value::Array(items) => items.iter().for_each(|i| walk(i, out, budget)),
            Value::Object(map) => map.values().for_each(|i| walk(i, out, budget)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    let mut budget = SCRUB_BUDGET_BYTES;
    if let Some(args) = rpc.and_then(|r| r.pointer("/params/arguments")) {
        walk(args, &mut out, &mut budget);
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out.dedup();
    out
}

/// `sentence` with every argument string the call carried replaced by
/// `ARGUMENT_MARK` — the error column never holds what the agent sent.
pub fn scrub_arguments(sentence: &str, args: &[String]) -> String {
    let mut out = sentence.to_string();
    for a in args {
        if out.contains(a.as_str()) {
            out = out.replace(a.as_str(), ARGUMENT_MARK);
        }
    }
    out
}

/// The sentence a spent call keeps in place of the upstream's own.
fn spent_sentence(code: Option<&Value>) -> String {
    match code {
        Some(code) => format!("JSON-RPC error {code}; {SPENT_REDACTED}"),
        None => format!("the call failed; {SPENT_REDACTED}"),
    }
}

/// An upstream- or dispatcher-authored failure sentence as the row may keep
/// it: on a call that carried a spent credential, only a fixed sentence.
pub fn upstream_text(text: &str, spent: bool) -> String {
    if spent {
        spent_sentence(None)
    } else {
        truncate_error(text)
    }
}

/// What one JSON-RPC reply (or a house `{error: "…"}` body) says about the
/// call it answers: an `error` member is an error carrying its message, a
/// result flagged `isError` is a tool-reported failure, anything else is ok.
/// `spent` (the call carried a spent credential) keeps only the code.
pub fn classify_reply(reply: &Value, spent: bool) -> (Outcome, Option<String>) {
    match reply.get("error") {
        Some(Value::Object(e)) if spent => {
            return (Outcome::Error, Some(spent_sentence(e.get("code"))));
        }
        Some(Value::Null) | None => {}
        Some(_) if spent => return (Outcome::Error, Some(spent_sentence(None))),
        Some(Value::String(s)) => return (Outcome::Error, Some(truncate_error(s))),
        Some(Value::Object(e)) => {
            let msg = match (e.get("message").and_then(Value::as_str), e.get("code")) {
                (Some(m), _) if !m.trim().is_empty() => m.to_string(),
                (_, Some(code)) => format!("JSON-RPC error {code}"),
                _ => "JSON-RPC error".to_string(),
            };
            return (Outcome::Error, Some(truncate_error(&msg)));
        }
        Some(other) => return (Outcome::Error, Some(truncate_error(&other.to_string()))),
    }
    if reply.pointer("/result/isError") == Some(&Value::Bool(true)) {
        return (Outcome::Error, Some(TOOL_FAILED.to_string()));
    }
    (Outcome::Ok, None)
}

/// An in-process dispatcher's answer (the Workbench, a stdio package): the
/// HTTP status it chose and its body, if any. A JSON-RPC body speaks for
/// itself; a bodiless or opaque failure status is an error naming the status.
pub fn classify_answer(status: u16, out: Option<&Value>, spent: bool) -> (Outcome, Option<String>) {
    match out {
        Some(v) if v.get("error").is_some() || v.get("result").is_some() => {
            classify_reply(v, spent)
        }
        _ if status >= 400 => (
            Outcome::Error,
            Some(format!("the dispatcher answered HTTP {status}")),
        ),
        _ => (Outcome::Ok, None),
    }
}

/// Does this message reply to the call (`id` when the call carried one)?
fn is_reply_to(msg: &Value, id: Option<&Value>) -> bool {
    (msg.get("result").is_some() || msg.get("error").is_some())
        && id.is_none_or(|id| msg.get("id") == Some(id))
}

/// How a streamed ANSWER ended, as the tap saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The upstream finished its body.
    Done,
    /// The upstream's body yielded an error part-way.
    Broke,
    /// The stream was dropped before it finished — the caller hung up.
    Abandoned,
}

/// The outcome of a relayed 2xx answer from the bytes the tap kept. `seen`
/// is the whole body, or — when `truncated` — only its tail (the cap keeps
/// the end, which is where an SSE reply sits). SSE framing is read line by
/// line (the last `data:` message replying to the call wins); a JSON body is
/// parsed whole. A truncated JSON body is a large one, and a large answer is a
/// result, so it is ok; an SSE stream that ended cleanly without a reply is
/// the upstream failing to answer; a caller that hung up first cancelled it.
pub fn classify_stream(
    seen: &[u8],
    truncated: bool,
    content_type: &str,
    id: Option<&Value>,
    ending: Ending,
    spent: bool,
) -> (Outcome, Option<String>) {
    let text = String::from_utf8_lossy(seen);
    let reply = if content_type.contains("text/event-stream") {
        // A truncated tail starts mid-line; its first line is a fragment.
        let lines = text.split('\n').skip(usize::from(truncated));
        lines
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .filter(|m| is_reply_to(m, id))
            .last()
    } else if truncated {
        None
    } else {
        serde_json::from_str::<Value>(text.trim())
            .ok()
            .filter(|m| is_reply_to(m, id))
    };
    if let Some(reply) = reply {
        return classify_reply(&reply, spent);
    }
    match ending {
        Ending::Broke => (Outcome::UpstreamError, Some(STREAM_BROKE.to_string())),
        Ending::Abandoned => (Outcome::Cancelled, Some(CALLER_GONE.to_string())),
        Ending::Done if content_type.contains("text/event-stream") && !truncated => {
            (Outcome::Error, Some(STREAM_NO_REPLY.to_string()))
        }
        Ending::Done => (Outcome::Ok, None),
    }
}

/// One tools/call in flight at the gateway: who is calling what, since when.
/// Cheap to clone (the pool and the argument strings are Arcs), so a branch
/// that defers the record — the streamed relay — carries its own copy.
#[derive(Clone)]
pub struct CallLog {
    pg: PgPool,
    subject: AgentSubject,
    server_name: String,
    server_id: Option<String>,
    tool: Option<String>,
    id: Option<Value>,
    /// The argument strings as the agent SENT them (before the spend
    /// boundary), scrubbed out of any stored sentence. Held in memory for
    /// the call's lifetime only; never written.
    args: Arc<Vec<String>>,
    /// The spend boundary put a credential into this call's arguments.
    spent: bool,
    started: Instant,
}

impl CallLog {
    /// The log for this request, or None when it is not a recorded method.
    /// `started` is when the gateway began answering, so a row's latency is
    /// the agent's own wait at the gateway — resolution, spend and dispatch.
    pub fn begin(
        pg: &PgPool,
        subject: &AgentSubject,
        server_name: &str,
        rpc: Option<&Value>,
        started: Instant,
    ) -> Option<CallLog> {
        if !is_recorded(rpc) {
            return None;
        }
        Some(CallLog {
            pg: pg.clone(),
            subject: subject.clone(),
            server_name: server_name.to_string(),
            server_id: None,
            tool: tool_of(rpc),
            id: rpc.and_then(|r| r.get("id")).cloned(),
            args: Arc::new(argument_strings(rpc)),
            spent: false,
            started,
        })
    }

    /// The registry id of the server, once resolution knows it. A refusal
    /// never learns it; the insert looks it up by name.
    pub fn with_server(mut self, id: &str) -> CallLog {
        self.server_id = Some(id.to_string());
        self
    }

    /// The spend boundary substituted a credential into this call: from here
    /// on no upstream-authored sentence is kept for it.
    pub fn mark_spent(&mut self) {
        self.spent = true;
    }

    /// Did the spend boundary put a credential into this call?
    pub fn spent(&self) -> bool {
        self.spent
    }

    /// The JSON-RPC id the call carried — what a streamed reply is matched on.
    pub fn rpc_id(&self) -> Option<&Value> {
        self.id.as_ref()
    }

    /// Write the row, off the hot path. Never blocks, never fails the call.
    /// `error` is scrubbed of the call's argument strings, then capped.
    pub fn finish(&self, outcome: Outcome, status: Option<u16>, error: Option<&str>) {
        let latency_ms = i32::try_from(self.started.elapsed().as_millis()).unwrap_or(i32::MAX);
        let row = CallRow {
            server_id: self.server_id.clone(),
            server_name: self.server_name.clone(),
            agent: subject_model(&self.subject).to_string(),
            // A legacy shared-key caller is identified but not proven to BE
            // that assistant, so it never names an owner (the same rule
            // assistant_owner_for applies).
            owner_eligible: subject_proven(&self.subject),
            tool: self.tool.clone(),
            outcome,
            status: status.and_then(|s| i16::try_from(s).ok()),
            latency_ms: Some(latency_ms),
            error: error
                .map(|e| truncate_error(&scrub_arguments(e, &self.args)))
                .filter(|e| !e.is_empty()),
        };
        // The relay's tap finishes from Drop, which can in principle run
        // outside a runtime; a record is never worth a panic.
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("{LOG} no runtime to record a call on; dropped");
            return;
        };
        if QUEUED.fetch_add(1, Ordering::Relaxed) >= INSERT_BACKLOG {
            QUEUED.fetch_sub(1, Ordering::Relaxed);
            tracing::warn!("{LOG} {INSERT_BACKLOG} call records already waiting; one dropped");
            return;
        }
        let pg = self.pg.clone();
        rt.spawn(async move {
            // Waiting here is off the hot path; only the permit-holders
            // touch the pool.
            let permit = INSERTS.acquire().await;
            let res = insert(&pg, row).await;
            drop(permit);
            QUEUED.fetch_sub(1, Ordering::Relaxed);
            if let Err(e) = res {
                tracing::warn!("{LOG} could not record a call: {e}");
            }
        });
    }
}

/// How many inserts may hold a pool connection at once — the record's share
/// of the pool, whatever the call rate.
const INSERT_CONCURRENCY: usize = 4;
/// How many rows may wait for a permit before new ones are dropped.
const INSERT_BACKLOG: usize = 10_000;
static INSERTS: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(INSERT_CONCURRENCY));
static QUEUED: AtomicUsize = AtomicUsize::new(0);

/// The row as it is inserted (user_id and a refusal's server id are resolved
/// inside the statement).
struct CallRow {
    server_id: Option<String>,
    server_name: String,
    agent: String,
    owner_eligible: bool,
    tool: Option<String>,
    outcome: Outcome,
    status: Option<i16>,
    latency_ms: Option<i32>,
    error: Option<String>,
}

/// One statement, one connection: the server id falls back to the name's
/// row (a refusal never resolved it — and one naming no server at all has
/// nothing to be attributed to, so nothing is inserted), and the acting human
/// is the personal assistant's owner by the agent's model.
async fn insert(pg: &PgPool, row: CallRow) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into mcp_tool_calls \
           (server_id, server_name, agent, user_id, method, tool, outcome, status, latency_ms, error) \
         select coalesce($1::text, (select s.id::text from mcp_servers s where s.name = $2::text)), \
                $2::text, $3::text, \
                case when $4::bool then (select d.owner_user_id::text from agent_defs d \
                                          where d.model = $3::text and d.owner_user_id is not null \
                                          limit 1) end, \
                'tools/call', $5::text, $6::text, $7::smallint, $8::int4, $9::text \
         where coalesce($1::text, (select s.id::text from mcp_servers s where s.name = $2::text)) is not null",
    )
    .bind(&row.server_id)
    .bind(&row.server_name)
    .bind(&row.agent)
    .bind(row.owner_eligible)
    .bind(&row.tool)
    .bind(row.outcome.as_str())
    .bind(row.status)
    .bind(row.latency_ms)
    .bind(&row.error)
    .execute(pg)
    .await?;
    Ok(())
}

// ── The streamed relay's tap ────────────────────────────────────────────────

/// Keep at most this many bytes of a streamed answer. Twice this is held
/// before the front is dropped, so the copy is amortized; what survives is the
/// TAIL, which is where an SSE reply sits.
const TAP_KEEP: usize = 64 * 1024;

struct Tap {
    log: CallLog,
    status: u16,
    content_type: String,
    buf: Vec<u8>,
    truncated: bool,
    ending: Ending,
}

impl Tap {
    fn observe(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > 2 * TAP_KEEP {
            let cut = self.buf.len() - TAP_KEEP;
            self.buf.drain(..cut);
            self.truncated = true;
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        let (outcome, error) = classify_stream(
            &self.buf,
            self.truncated,
            &self.content_type,
            self.log.rpc_id(),
            self.ending,
            self.log.spent(),
        );
        self.log
            .finish(outcome, Some(self.status), error.as_deref());
    }
}

/// A relayed body that records its call when it ends — passes every chunk
/// through untouched, keeps a bounded tail to classify, and writes the row
/// when the upstream finishes, breaks, or the caller drops the stream.
pub struct TappedStream<S> {
    inner: Pin<Box<S>>,
    tap: Option<Tap>,
}

impl<S> TappedStream<S> {
    pub fn new(inner: S, log: CallLog, status: u16, content_type: &str) -> TappedStream<S> {
        TappedStream {
            inner: Box::pin(inner),
            tap: Some(Tap {
                log,
                status,
                content_type: content_type.to_string(),
                buf: Vec::new(),
                truncated: false,
                // Until the upstream says otherwise, a drop is the caller's.
                ending: Ending::Abandoned,
            }),
        }
    }
}

impl<S, B, E> Stream for TappedStream<S>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
{
    type Item = Result<B, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = self.inner.as_mut().poll_next(cx);
        match &polled {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Some(tap) = self.tap.as_mut() {
                    tap.observe(chunk.as_ref());
                }
            }
            Poll::Ready(Some(Err(_))) => {
                if let Some(mut tap) = self.tap.take() {
                    tap.ending = Ending::Broke;
                }
            }
            Poll::Ready(None) => {
                // Record now rather than whenever the body is dropped.
                if let Some(mut tap) = self.tap.take() {
                    tap.ending = Ending::Done;
                }
            }
            Poll::Pending => {}
        }
        polled
    }
}

// ── Reads: the admin list and the summary ───────────────────────────────────

/// The list route's filters, already validated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallFilter {
    /// A server id or name.
    pub server: Option<String>,
    pub agent: Option<String>,
    pub tool: Option<String>,
    pub outcome: Option<Outcome>,
    pub since_ms: Option<i64>,
    pub limit: i64,
}

/// The page size a request asked for, clamped to 1..=500 (100 when absent).
pub fn clamp_limit(asked: Option<i64>) -> i64 {
    asked.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// A query-string value as a filter: absent and blank are the same thing.
pub fn filter_value(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The raw query strings as a filter, or the 400 sentence for the first bad one.
pub fn parse_filter(
    server: Option<&str>,
    agent: Option<&str>,
    tool: Option<&str>,
    outcome: Option<&str>,
    since: Option<&str>,
    limit: Option<&str>,
) -> Result<CallFilter, String> {
    let outcome = match filter_value(outcome) {
        None => None,
        Some(o) => Some(Outcome::parse(&o).ok_or_else(|| {
            format!(
                "outcome must be one of {}",
                Outcome::ALL.map(Outcome::as_str).join(", ")
            )
        })?),
    };
    let since_ms = parse_since(since)?;
    let limit = match filter_value(limit) {
        None => None,
        Some(l) => Some(
            l.parse::<i64>()
                .map_err(|_| "limit must be a whole number (1-500)".to_string())?,
        ),
    };
    Ok(CallFilter {
        server: filter_value(server),
        agent: filter_value(agent),
        tool: filter_value(tool),
        outcome,
        since_ms,
        limit: clamp_limit(limit),
    })
}

/// `since=<epoch ms>`, or the 400 sentence.
pub fn parse_since(since: Option<&str>) -> Result<Option<i64>, String> {
    match filter_value(since) {
        None => Ok(None),
        Some(s) => s
            .parse::<i64>()
            .ok()
            .filter(|ms| *ms >= 0)
            .map(Some)
            .ok_or_else(|| "since must be a time in epoch milliseconds".to_string()),
    }
}

/// The newest calls matching the filter, as the wire carries them.
pub async fn list_calls(pg: &PgPool, f: &CallFilter) -> Result<Vec<Value>, sqlx::Error> {
    type Row = (
        i64,
        i64,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        String,
        Option<i16>,
        Option<i32>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "select c.id, (trunc(extract(epoch from c.at) * 1000))::bigint, c.server_id, c.server_name, \
                c.agent, c.user_id, u.name, c.method, c.tool, c.outcome, c.status, c.latency_ms, c.error \
         from mcp_tool_calls c \
         left join users u on u.id::text = c.user_id \
         where ($1::text is null or c.server_id = $1 or c.server_name = $1) \
           and ($2::text is null or c.agent = $2) \
           and ($3::text is null or c.tool = $3) \
           and ($4::text is null or c.outcome = $4) \
           and ($5::int8 is null or c.at >= to_timestamp($5::int8 / 1000.0)) \
         order by c.at desc, c.id desc \
         limit $6::int8",
    )
    .bind(&f.server)
    .bind(&f.agent)
    .bind(&f.tool)
    .bind(f.outcome.map(Outcome::as_str))
    .bind(f.since_ms)
    .bind(f.limit)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                at,
                server_id,
                server_name,
                agent,
                user_id,
                user_name,
                method,
                tool,
                outcome,
                status,
                latency,
                error,
            )| {
                json!({
                    "id": id,
                    "at": epoch_ms_to_iso(at),
                    "serverId": server_id,
                    "serverName": server_name,
                    "agent": agent,
                    "userId": user_id,
                    "userName": user_name,
                    "method": method,
                    "tool": tool,
                    "outcome": outcome,
                    "status": status,
                    "latencyMs": latency,
                    "error": error,
                })
            },
        )
        .collect())
}

/// One server's window, as the aggregate query reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerAgg {
    pub server_id: String,
    pub server_name: String,
    pub calls: i64,
    pub errors: i64,
    pub denied: i64,
    pub not_connected: i64,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
}

/// One (server, tool) pair's window.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolAgg {
    pub server_id: String,
    pub tool: String,
    pub calls: i64,
    pub errors: i64,
}

/// One agent's window.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentAgg {
    pub agent: String,
    pub calls: i64,
    pub errors: i64,
}

/// The summary wire from the three aggregates — pure, so the ranking and the
/// rounding are pinned without a database. Servers rank by calls (then name),
/// each naming its `TOP_TOOLS` busiest tools (calls, then errors, then name);
/// agents rank by calls (then name); totals add the servers up.
pub fn summary_wire(
    since_ms: i64,
    mut servers: Vec<ServerAgg>,
    mut tools: Vec<ToolAgg>,
    mut agents: Vec<AgentAgg>,
) -> Value {
    servers.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then_with(|| a.server_name.cmp(&b.server_name))
    });
    tools.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then_with(|| b.errors.cmp(&a.errors))
            .then_with(|| a.tool.cmp(&b.tool))
    });
    agents.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.agent.cmp(&b.agent)));
    let ms = |v: Option<f64>| v.map(|v| v.round() as i64);
    let (mut calls, mut errors, mut denied, mut not_connected) = (0, 0, 0, 0);
    let servers_wire: Vec<Value> = servers
        .iter()
        .map(|s| {
            calls += s.calls;
            errors += s.errors;
            denied += s.denied;
            not_connected += s.not_connected;
            let top: Vec<Value> = tools
                .iter()
                .filter(|t| t.server_id == s.server_id)
                .take(TOP_TOOLS)
                .map(|t| json!({ "tool": t.tool, "calls": t.calls, "errors": t.errors }))
                .collect();
            json!({
                "serverId": s.server_id,
                "serverName": s.server_name,
                "calls": s.calls,
                "errors": s.errors,
                "denied": s.denied,
                "notConnected": s.not_connected,
                "p50Ms": ms(s.p50_ms),
                "p95Ms": ms(s.p95_ms),
                "topTools": top,
            })
        })
        .collect();
    json!({
        "since": epoch_ms_to_iso(since_ms),
        "totals": {
            "calls": calls,
            "errors": errors,
            "denied": denied,
            "notConnected": not_connected,
        },
        "servers": servers_wire,
        "agents": agents
            .iter()
            .map(|a| json!({ "agent": a.agent, "calls": a.calls, "errors": a.errors }))
            .collect::<Vec<_>>(),
    })
}

/// The summary since `since_ms`, optionally narrowed to one server (id or
/// name) — the card's one line reads it that way.
pub async fn summary(
    pg: &PgPool,
    since_ms: i64,
    server: Option<&str>,
) -> Result<Value, sqlx::Error> {
    let errs = Outcome::error_strs();
    // The window and the server narrowing every aggregate shares: $1 since,
    // $2 server id-or-name, $3 the error outcomes. A macro, not a const:
    // sqlx takes only static SQL, and concat! keeps it static.
    macro_rules! window {
        () => {
            " from mcp_tool_calls where at >= to_timestamp($1::int8 / 1000.0) \
               and ($2::text is null or server_id = $2 or server_name = $2) "
        };
    }
    // The latest name a server was called under — a server's name is its
    // gateway path, but a renamed row's history still groups by its id. The
    // percentiles leave out what the gateway answered itself ($4: refusals,
    // missing connections), so they are the server's latency, not the
    // gateway's.
    let servers: Vec<(String, String, i64, i64, i64, i64, Option<f64>, Option<f64>)> =
        sqlx::query_as(concat!(
            "select server_id, (array_agg(server_name order by at desc))[1], count(*)::int8, \
                    (count(*) filter (where outcome = any($3::text[])))::int8, \
                    (count(*) filter (where outcome = 'denied'))::int8, \
                    (count(*) filter (where outcome = 'not_connected'))::int8, \
                    percentile_cont(0.5) within group (order by latency_ms) \
                      filter (where outcome <> all($4::text[])), \
                    percentile_cont(0.95) within group (order by latency_ms) \
                      filter (where outcome <> all($4::text[]))",
            window!(),
            "group by server_id"
        ))
        .bind(since_ms)
        .bind(server)
        .bind(&errs)
        .bind(Outcome::gateway_answered_strs())
        .fetch_all(pg)
        .await?;
    let tools: Vec<(String, String, i64, i64)> = sqlx::query_as(concat!(
        "select server_id, tool, count(*)::int8, \
                (count(*) filter (where outcome = any($3::text[])))::int8",
        window!(),
        "and tool is not null group by server_id, tool"
    ))
    .bind(since_ms)
    .bind(server)
    .bind(&errs)
    .fetch_all(pg)
    .await?;
    let agents: Vec<(String, i64, i64)> = sqlx::query_as(concat!(
        "select agent, count(*)::int8, \
                (count(*) filter (where outcome = any($3::text[])))::int8",
        window!(),
        "group by agent"
    ))
    .bind(since_ms)
    .bind(server)
    .bind(&errs)
    .fetch_all(pg)
    .await?;
    Ok(summary_wire(
        since_ms,
        servers
            .into_iter()
            .map(
                |(server_id, server_name, calls, errors, denied, not_connected, p50, p95)| {
                    ServerAgg {
                        server_id,
                        server_name,
                        calls,
                        errors,
                        denied,
                        not_connected,
                        p50_ms: p50,
                        p95_ms: p95,
                    }
                },
            )
            .collect(),
        tools
            .into_iter()
            .map(|(server_id, tool, calls, errors)| ToolAgg {
                server_id,
                tool,
                calls,
                errors,
            })
            .collect(),
        agents
            .into_iter()
            .map(|(agent, calls, errors)| AgentAgg {
                agent,
                calls,
                errors,
            })
            .collect(),
    ))
}

// ── Retention ───────────────────────────────────────────────────────────────

/// The retention the setting asks for, in days: a positive whole number
/// (a JSON number or a numeric string), clamped to ten years; anything else
/// is the default — a typo in a setting must never prune everything.
pub fn retention_days(setting: &Value) -> i64 {
    let asked = match setting {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    match asked {
        Some(d) if d >= 1 => d.min(MAX_RETENTION_DAYS),
        _ => DEFAULT_RETENTION_DAYS,
    }
}

/// A retention an admin ASKED for — stricter than `retention_days`, which
/// forgives a stored typo: a whole number of days in 1..=3650, or the 400
/// sentence.
pub fn parse_retention(asked: &Value) -> Result<i64, String> {
    asked
        .as_i64()
        .filter(|d| (1..=MAX_RETENTION_DAYS).contains(d))
        .ok_or_else(|| format!("days must be a whole number from 1 to {MAX_RETENTION_DAYS}"))
}

/// The retention in force, in days.
pub async fn retention(pg: &PgPool) -> i64 {
    retention_days(
        &talaria_settings::get_setting(pg, RETENTION_KEY, json!(DEFAULT_RETENTION_DAYS)).await,
    )
}

/// Store a retention (already validated by `parse_retention`). The next
/// prune applies it.
pub async fn set_retention(pg: &PgPool, days: i64) -> Result<(), sqlx::Error> {
    talaria_settings::set_setting(pg, RETENTION_KEY, &json!(days)).await
}

/// Rows deleted per statement: a backlog after a long outage is pruned in
/// bites, so no single delete holds the table for long.
const PRUNE_BATCH: i64 = 5_000;

/// Delete every row older than the retention window; answers how many.
pub async fn prune(pg: &PgPool) -> Result<u64, sqlx::Error> {
    let days = retention(pg).await;
    let mut total = 0u64;
    loop {
        let n = sqlx::query(
            "delete from mcp_tool_calls where id in ( \
               select id from mcp_tool_calls \
               where at < now() - make_interval(days => $1::int4) \
               order by at limit $2::int8)",
        )
        .bind(i32::try_from(days).unwrap_or(i32::MAX))
        .bind(PRUNE_BATCH)
        .execute(pg)
        .await?
        .rows_affected();
        total += n;
        if n < PRUNE_BATCH as u64 {
            return Ok(total);
        }
    }
}

/// The scheduled prune: every six hours, leased (one instance per interval —
/// the rows are shared).
pub fn prune_job_spec(pg: PgPool) -> talaria_scheduler::JobSpec {
    talaria_scheduler::JobSpec {
        name: talaria_scheduler::JobName::McpCallsPrune,
        every_ms: 6 * 60 * 60_000,
        first_run_delay_ms: Some(10 * 60_000),
        max_run_ms: Some(5 * 60_000),
        per_instance: false,
        run: Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move {
                match prune(&pg).await {
                    Ok(0) => Ok(None),
                    Ok(n) => Ok(Some(format!("pruned {n} MCP call records past retention"))),
                    Err(e) => Err(format!("prune failed: {e}")),
                }
            })
        }),
    }
}

/// Boot registration (talaria-jobs' table).
pub fn register_mcp_calls_prune_job(pg: PgPool) {
    talaria_scheduler::register_job(prune_job_spec(pg));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outcome_round_trips_its_wire_string() {
        let wire: Vec<&str> = Outcome::ALL.iter().map(|o| o.as_str()).collect();
        assert_eq!(
            wire,
            [
                "ok",
                "error",
                "denied",
                "not_connected",
                "upstream_error",
                "spend_error",
                "cancelled"
            ]
        );
        for o in Outcome::ALL {
            assert_eq!(Outcome::parse(o.as_str()), Some(o));
        }
        assert_eq!(Outcome::parse("OK"), None);
        assert_eq!(Outcome::parse(""), None);
    }

    #[test]
    fn errors_are_failures_not_policy() {
        assert_eq!(
            Outcome::error_strs(),
            ["error", "upstream_error", "spend_error"]
        );
        assert!(!Outcome::Ok.is_error());
        assert!(!Outcome::Denied.is_error());
        assert!(!Outcome::NotConnected.is_error());
        assert!(
            !Outcome::Cancelled.is_error(),
            "a caller hanging up is not the server's error"
        );
        assert_eq!(
            Outcome::gateway_answered_strs(),
            ["denied", "not_connected"],
            "the outcomes the gateway answers itself stay out of the percentiles"
        );
        assert!(Outcome::Cancelled.reached_server());
    }

    #[test]
    fn an_error_sentence_is_capped_at_300_chars_and_says_so() {
        assert_eq!(truncate_error("  short  "), "short");
        let exact = "x".repeat(300);
        assert_eq!(truncate_error(&exact), exact, "300 chars fit whole");
        let long = "y".repeat(301);
        let cut = truncate_error(&long);
        assert_eq!(cut.chars().count(), 300);
        assert!(cut.ends_with('…'));
        assert!(cut.starts_with(&"y".repeat(299)));
        // Multibyte text is cut by character, never mid-character.
        let wide = "é".repeat(400);
        let cut = truncate_error(&wide);
        assert_eq!(cut.chars().count(), 300);
        assert!(cut.is_char_boundary(cut.len()));
    }

    #[test]
    fn only_a_single_tools_call_is_recorded() {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "search" } });
        assert!(is_recorded(Some(&call)));
        assert_eq!(tool_of(Some(&call)).as_deref(), Some("search"));
        for method in [
            "initialize",
            "tools/list",
            "ping",
            "notifications/initialized",
            "notifications/cancelled",
            "resources/list",
        ] {
            let rpc = json!({ "jsonrpc": "2.0", "id": 1, "method": method });
            assert!(!is_recorded(Some(&rpc)), "{method} is chatter");
        }
        assert!(
            !is_recorded(Some(&json!([call]))),
            "a batch is not recorded"
        );
        assert!(!is_recorded(None), "an unparsed body is not recorded");
        // A nameless (or non-string-named) call is still a call; it has no tool.
        let nameless = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {} });
        assert!(is_recorded(Some(&nameless)));
        assert_eq!(tool_of(Some(&nameless)), None);
        let numeric = json!({ "method": "tools/call", "params": { "name": 7 } });
        assert_eq!(tool_of(Some(&numeric)), None);
    }

    #[test]
    fn a_reply_classifies_by_error_then_is_error_then_ok() {
        let ok = json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [{ "type": "text", "text": "secret result" }] } });
        assert_eq!(classify_reply(&ok, false), (Outcome::Ok, None));

        let rpc_err = json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32602, "message": "tool \"rm\" is not available here" } });
        assert_eq!(
            classify_reply(&rpc_err, false),
            (
                Outcome::Error,
                Some("tool \"rm\" is not available here".into())
            )
        );
        let coded = json!({ "error": { "code": -32000 } });
        assert_eq!(
            classify_reply(&coded, false),
            (Outcome::Error, Some("JSON-RPC error -32000".into()))
        );
        let house = json!({ "error": "upstream unreachable" });
        assert_eq!(
            classify_reply(&house, false),
            (Outcome::Error, Some("upstream unreachable".into()))
        );

        // A tool-reported failure keeps a fixed sentence, never its text.
        let failed = json!({ "result": { "isError": true, "content": [{ "type": "text", "text": "the row said password=hunter2" }] } });
        let (o, e) = classify_reply(&failed, false);
        assert_eq!(o, Outcome::Error);
        assert_eq!(e.as_deref(), Some(TOOL_FAILED));
        assert!(!e.unwrap().contains("hunter2"));

        let null_error = json!({ "result": {}, "error": null });
        assert_eq!(classify_reply(&null_error, false), (Outcome::Ok, None));

        let long = json!({ "error": { "message": "z".repeat(1000) } });
        assert_eq!(classify_reply(&long, false).1.unwrap().chars().count(), 300);
    }

    #[test]
    fn an_in_process_answer_classifies_by_body_then_status() {
        let ok = json!({ "jsonrpc": "2.0", "id": 1, "result": {} });
        assert_eq!(classify_answer(200, Some(&ok), false), (Outcome::Ok, None));
        let err = json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32601, "message": "no such tool" } });
        assert_eq!(
            classify_answer(200, Some(&err), false),
            (Outcome::Error, Some("no such tool".into()))
        );
        assert_eq!(classify_answer(202, None, false), (Outcome::Ok, None));
        assert_eq!(
            classify_answer(500, None, false),
            (
                Outcome::Error,
                Some("the dispatcher answered HTTP 500".into())
            )
        );
        assert_eq!(
            classify_answer(502, Some(&json!({ "error": "app down" })), false),
            (Outcome::Error, Some("app down".into()))
        );
    }

    #[test]
    fn a_json_answer_is_classified_whole() {
        let id = json!(4);
        let body = br#"{"jsonrpc":"2.0","id":4,"result":{"content":[]}}"#;
        assert_eq!(
            classify_stream(
                body,
                false,
                "application/json",
                Some(&id),
                Ending::Done,
                false
            ),
            (Outcome::Ok, None)
        );
        let body = br#"{"jsonrpc":"2.0","id":4,"error":{"code":-32603,"message":"boom"}}"#;
        assert_eq!(
            classify_stream(
                body,
                false,
                "application/json",
                Some(&id),
                Ending::Done,
                false
            ),
            (Outcome::Error, Some("boom".into()))
        );
        // A large (truncated) JSON answer is a result.
        assert_eq!(
            classify_stream(
                b"aaaa",
                true,
                "application/json",
                Some(&id),
                Ending::Done,
                false
            ),
            (Outcome::Ok, None)
        );
        // An opaque 2xx is not second-guessed.
        assert_eq!(
            classify_stream(
                b"",
                false,
                "application/json",
                Some(&id),
                Ending::Done,
                false
            ),
            (Outcome::Ok, None)
        );
    }

    #[test]
    fn an_sse_answer_is_classified_by_its_reply_to_this_call() {
        let id = json!(9);
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n\
                    event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"isError\":true}}\n\n";
        assert_eq!(
            classify_stream(
                body.as_bytes(),
                false,
                "text/event-stream",
                Some(&id),
                Ending::Done,
                false
            ),
            (Outcome::Error, Some(TOOL_FAILED.into()))
        );
        // A reply to another id is not this call's reply.
        let other = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
        assert_eq!(
            classify_stream(
                other.as_bytes(),
                false,
                "text/event-stream",
                Some(&id),
                Ending::Done,
                false
            ),
            (Outcome::Error, Some(STREAM_NO_REPLY.into()))
        );
        // A truncated tail still finds the reply; its leading fragment is skipped.
        let tail = "ss\":{}}\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}\n";
        assert_eq!(
            classify_stream(
                tail.as_bytes(),
                true,
                "text/event-stream; charset=utf-8",
                Some(&id),
                Ending::Done,
                false
            ),
            (Outcome::Ok, None)
        );
    }

    #[test]
    fn a_stream_that_never_replied_says_how_it_ended() {
        let id = json!(1);
        for (ending, expected) in [
            (
                Ending::Broke,
                (Outcome::UpstreamError, Some(STREAM_BROKE.to_string())),
            ),
            (
                Ending::Abandoned,
                (Outcome::Cancelled, Some(CALLER_GONE.to_string())),
            ),
            (
                Ending::Done,
                (Outcome::Error, Some(STREAM_NO_REPLY.to_string())),
            ),
        ] {
            assert_eq!(
                classify_stream(b"", false, "text/event-stream", Some(&id), ending, false),
                expected
            );
        }
        // …but a reply already seen wins over how the stream ended.
        let body = br#"data: {"jsonrpc":"2.0","id":1,"result":{}}"#;
        assert_eq!(
            classify_stream(
                body,
                false,
                "text/event-stream",
                Some(&id),
                Ending::Abandoned,
                false
            ),
            (Outcome::Ok, None)
        );
    }

    #[tokio::test]
    async fn the_tap_keeps_a_bounded_tail() {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://calls-test@localhost:1/none")
            .expect("a lazy pool connects to nothing");
        let call =
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "t" } });
        let log = CallLog::begin(
            &pg,
            &AgentSubject::Model("a-b".into()),
            "srv",
            Some(&call),
            Instant::now(),
        )
        .expect("a tools/call begins a log");
        let mut tap = Tap {
            log,
            status: 200,
            content_type: "text/event-stream".into(),
            buf: Vec::new(),
            truncated: false,
            ending: Ending::Done,
        };
        for _ in 0..40 {
            tap.observe(&[b'x'; 8 * 1024]);
        }
        assert!(tap.truncated);
        assert!(tap.buf.len() <= 2 * TAP_KEEP);
        assert!(tap.buf.len() >= TAP_KEEP);
        // Dropping the tap records from a spawned task; the dead pool only
        // makes that task warn — the drop itself never blocks or panics.
        drop(tap);
    }

    #[tokio::test]
    async fn the_tapped_stream_passes_every_chunk_through_and_records_on_end() {
        use futures_util::StreamExt;
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://calls-test@localhost:1/none")
            .expect("a lazy pool connects to nothing");
        let call =
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "t" } });
        let log = CallLog::begin(
            &pg,
            &AgentSubject::Model("a-b".into()),
            "srv",
            Some(&call),
            Instant::now(),
        )
        .unwrap();
        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = vec![
            Ok(b"data: {\"jsonrpc\":\"2.0\",".to_vec()),
            Ok(b"\"id\":1,\"result\":{}}\n".to_vec()),
        ];
        let mut tapped = TappedStream::new(
            futures_util::stream::iter(chunks),
            log,
            200,
            "text/event-stream",
        );
        let mut out = Vec::new();
        while let Some(chunk) = tapped.next().await {
            out.extend(chunk.unwrap());
        }
        assert_eq!(
            out,
            b"data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n".to_vec(),
            "the relay is byte-for-byte"
        );
        assert!(
            tapped.tap.is_none(),
            "the record is written when the upstream finishes, not when the body drops"
        );
    }

    #[test]
    fn filters_parse_and_refuse_with_a_sentence() {
        let f = parse_filter(
            Some("notion"),
            Some(" a-b "),
            Some(""),
            Some("not_connected"),
            Some("1700000000000"),
            None,
        )
        .unwrap();
        assert_eq!(
            f,
            CallFilter {
                server: Some("notion".into()),
                agent: Some("a-b".into()),
                tool: None,
                outcome: Some(Outcome::NotConnected),
                since_ms: Some(1_700_000_000_000),
                limit: 100,
            }
        );
        let e = parse_filter(None, None, None, Some("bad"), None, None).unwrap_err();
        assert_eq!(
            e,
            "outcome must be one of ok, error, denied, not_connected, upstream_error, spend_error, cancelled"
        );
        assert!(parse_filter(None, None, None, None, Some("yesterday"), None).is_err());
        assert!(parse_filter(None, None, None, None, Some("-5"), None).is_err());
        assert!(parse_filter(None, None, None, None, None, Some("ten")).is_err());
        assert_eq!(clamp_limit(None), 100);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(250)), 250);
        assert_eq!(clamp_limit(Some(9_999)), 500);
        assert_eq!(
            parse_filter(None, None, None, None, None, Some("9999"))
                .unwrap()
                .limit,
            500
        );
    }

    #[test]
    fn the_summary_ranks_servers_tools_and_agents() {
        let s = |id: &str, name: &str, calls, errors| ServerAgg {
            server_id: id.into(),
            server_name: name.into(),
            calls,
            errors,
            denied: 1,
            not_connected: 2,
            p50_ms: Some(12.4),
            p95_ms: Some(99.6),
        };
        let t = |sid: &str, tool: &str, calls, errors| ToolAgg {
            server_id: sid.into(),
            tool: tool.into(),
            calls,
            errors,
        };
        let v = summary_wire(
            0,
            vec![
                s("1", "zeta", 5, 1),
                s("2", "alpha", 5, 0),
                s("3", "busy", 50, 3),
            ],
            vec![
                t("3", "b", 10, 0),
                t("3", "a", 10, 0),
                t("3", "c", 10, 2),
                t("3", "d", 9, 0),
                t("3", "e", 8, 0),
                t("3", "f", 7, 0),
                t("1", "only", 5, 1),
            ],
            vec![
                AgentAgg {
                    agent: "b-x".into(),
                    calls: 3,
                    errors: 0,
                },
                AgentAgg {
                    agent: "a-x".into(),
                    calls: 3,
                    errors: 1,
                },
                AgentAgg {
                    agent: "c-x".into(),
                    calls: 54,
                    errors: 3,
                },
            ],
        );
        let names: Vec<&str> = v["servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["serverName"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["busy", "alpha", "zeta"], "calls, then name");
        let busy = &v["servers"][0];
        let tools: Vec<&str> = busy["topTools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["tool"].as_str().unwrap())
            .collect();
        assert_eq!(
            tools,
            ["c", "a", "b", "d", "e"],
            "top five: calls, then errors, then name"
        );
        assert_eq!(busy["p50Ms"], json!(12));
        assert_eq!(busy["p95Ms"], json!(100));
        assert_eq!(busy["notConnected"], json!(2));
        assert_eq!(v["servers"][1]["topTools"], json!([]));
        assert_eq!(v["servers"][2]["topTools"][0]["tool"], json!("only"));
        assert_eq!(
            v["totals"],
            json!({ "calls": 60, "errors": 4, "denied": 3, "notConnected": 6 })
        );
        let agents: Vec<&str> = v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["agent"].as_str().unwrap())
            .collect();
        assert_eq!(agents, ["c-x", "a-x", "b-x"]);
        assert_eq!(v["since"], json!(epoch_ms_to_iso(0)));
    }

    #[test]
    fn an_empty_window_is_an_empty_summary_not_nulls() {
        let v = summary_wire(0, vec![], vec![], vec![]);
        assert_eq!(v["servers"], json!([]));
        assert_eq!(v["agents"], json!([]));
        assert_eq!(v["totals"]["calls"], json!(0));
        let lone = ServerAgg {
            server_id: "1".into(),
            server_name: "s".into(),
            calls: 1,
            errors: 0,
            denied: 1,
            not_connected: 0,
            p50_ms: None,
            p95_ms: None,
        };
        let v = summary_wire(0, vec![lone], vec![], vec![]);
        assert_eq!(v["servers"][0]["p50Ms"], Value::Null);
    }

    #[test]
    fn retention_reads_a_positive_whole_number_or_the_default() {
        assert_eq!(retention_days(&json!(7)), 7);
        assert_eq!(retention_days(&json!("14")), 14);
        assert_eq!(retention_days(&json!(99_999)), 3650);
        for bad in [
            json!(0),
            json!(-3),
            json!(2.5),
            json!("soon"),
            json!(null),
            json!({}),
        ] {
            assert_eq!(retention_days(&bad), DEFAULT_RETENTION_DAYS, "{bad}");
        }
    }

    #[tokio::test]
    async fn the_prune_job_is_leased_every_six_hours() {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://calls-test@localhost:1/none")
            .expect("a lazy pool connects to nothing");
        let spec = prune_job_spec(pg);
        assert_eq!(spec.name.as_str(), "mcp-calls-prune");
        assert_eq!(spec.every_ms, 6 * 60 * 60 * 1000);
        assert!(
            !spec.per_instance,
            "the rows are shared: one pruner per interval"
        );
    }

    #[test]
    fn a_spent_call_keeps_only_the_code_never_the_upstreams_sentence() {
        // The finding's shape: Zod quotes the value it received, and the
        // value is the real credential the spend boundary put there.
        let echo = json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32602,
            "message": "Invalid arguments for tool deploy: Invalid enum value. Expected 'a' | 'b', received 'ghp_realtokenvalue'" } });
        let (o, e) = classify_reply(&echo, true);
        assert_eq!(o, Outcome::Error);
        let e = e.unwrap();
        assert!(!e.contains("ghp_"), "{e}");
        assert_eq!(e, format!("JSON-RPC error -32602; {SPENT_REDACTED}"));
        // A house string error and an opaque error member are fixed too.
        for reply in [
            json!({ "error": "bad value ghp_realtokenvalue" }),
            json!({ "error": ["ghp_realtokenvalue"] }),
        ] {
            let (o, e) = classify_reply(&reply, true);
            assert_eq!(o, Outcome::Error);
            assert!(!e.as_deref().unwrap().contains("ghp_"), "{reply}");
        }
        // The same path through the dispatcher and the stream classifiers.
        let (_, e) = classify_answer(200, Some(&echo), true);
        assert!(!e.unwrap().contains("ghp_"));
        let sse = format!("data: {echo}\n\n");
        let (o, e) = classify_stream(
            sse.as_bytes(),
            false,
            "text/event-stream",
            Some(&json!(1)),
            Ending::Done,
            true,
        );
        assert_eq!(o, Outcome::Error);
        assert!(!e.unwrap().contains("ghp_"));
        // An ok spent call is still ok, and a tool-reported failure keeps
        // its fixed sentence (it never held the text anyway).
        assert_eq!(
            classify_reply(&json!({ "result": {} }), true),
            (Outcome::Ok, None)
        );
        assert_eq!(
            classify_reply(&json!({ "result": { "isError": true } }), true)
                .1
                .as_deref(),
            Some(TOOL_FAILED)
        );
        assert_eq!(
            upstream_text("pump said ghp_x", true),
            format!("the call failed; {SPENT_REDACTED}")
        );
        assert_eq!(upstream_text("  pump died  ", false), "pump died");
    }

    #[test]
    fn an_echoed_argument_is_scrubbed_from_the_stored_sentence() {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "lookup",
            "arguments": {
                "email": "alice@example.com",
                "nested": { "list": ["hunter2-password", "ab"] },
                "n": 42,
                "handle": "«secret:deploy.github_pat»"
            }
        } });
        let args = argument_strings(Some(&call));
        assert!(args.contains(&"alice@example.com".to_string()));
        assert!(args.contains(&"hunter2-password".to_string()));
        assert!(args.contains(&"«secret:deploy.github_pat»".to_string()));
        assert!(
            !args.iter().any(|a| a == "ab"),
            "short strings are left alone"
        );
        assert!(
            !args.iter().any(|a| a == "lookup"),
            "the tool name is not an argument"
        );
        let said = "no user alice@example.com; hint hunter2-password was wrong; «secret:deploy.github_pat» unknown";
        assert_eq!(
            scrub_arguments(said, &args),
            "no user «argument»; hint «argument» was wrong; «argument» unknown"
        );
        // Longest first: a value containing another is replaced whole.
        let both = json!({ "params": { "arguments": { "a": "abcdef", "b": "abcdefghij" } } });
        let args = argument_strings(Some(&both));
        assert_eq!(scrub_arguments("got abcdefghij", &args), "got «argument»");
        // No arguments, nothing scrubbed.
        assert_eq!(
            scrub_arguments("upstream unreachable", &[]),
            "upstream unreachable"
        );
        // The copy kept to scrub with is bounded.
        let huge =
            json!({ "params": { "arguments": { "a": "x".repeat(200_000), "b": "short-enough" } } });
        let args = argument_strings(Some(&huge));
        assert_eq!(args, vec!["short-enough".to_string()]);
    }

    #[test]
    fn the_tool_column_is_capped_like_the_error_column() {
        let long = json!({ "method": "tools/call", "params": { "name": "t".repeat(2_000_000) } });
        let tool = tool_of(Some(&long)).unwrap();
        assert_eq!(tool.chars().count(), TOOL_MAX_CHARS);
        assert!(tool.ends_with('…'));
        let exact = json!({ "method": "tools/call", "params": { "name": "u".repeat(200) } });
        assert_eq!(tool_of(Some(&exact)).unwrap(), "u".repeat(200));
    }

    #[test]
    fn every_tools_call_in_a_batch_is_a_member_and_the_batch_is_refused() {
        let call = |id: i64, name: &str| json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name } });
        let single = call(1, "echo");
        assert_eq!(recorded_members(Some(&single)), vec![&single]);
        assert!(
            !batch_needs_refusal(Some(&single)),
            "a single call goes through the gate"
        );

        let batch = json!([
            call(1, "echo"),
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            call(2, "delete_repo"),
        ]);
        let members = recorded_members(Some(&batch));
        assert_eq!(members.len(), 2);
        assert_eq!(tool_of(Some(members[1])).as_deref(), Some("delete_repo"));
        assert!(batch_needs_refusal(Some(&batch)));

        let listing = json!([{ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }]);
        assert!(
            batch_needs_refusal(Some(&listing)),
            "a batched tools/list skips the catalog filter"
        );
        assert!(recorded_members(Some(&listing)).is_empty());

        let chatter = json!([{ "jsonrpc": "2.0", "method": "notifications/initialized" }, { "jsonrpc": "2.0", "id": 3, "method": "ping" }]);
        assert!(
            !batch_needs_refusal(Some(&chatter)),
            "protocol chatter still relays"
        );
        assert!(recorded_members(Some(&json!({ "method": "tools/list" }))).is_empty());
        assert!(recorded_members(None).is_empty());
    }

    #[test]
    fn an_asked_retention_is_strict_where_the_stored_one_forgives() {
        assert_eq!(parse_retention(&json!(7)), Ok(7));
        assert_eq!(parse_retention(&json!(3650)), Ok(3650));
        for bad in [
            json!(0),
            json!(3651),
            json!(-1),
            json!(2.5),
            json!("30"),
            json!(null),
        ] {
            assert_eq!(
                parse_retention(&bad),
                Err("days must be a whole number from 1 to 3650".to_string()),
                "{bad}"
            );
        }
    }
}
