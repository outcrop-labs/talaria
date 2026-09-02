// POST /api/llm/v1/chat/completions. OpenAI-compatible chat over the org's
// model stack: streaming and non-streaming both relay, every call metered
// into the ledger under the calling key's identity, every completion through
// the confab guard (gateway/guard.rs).
//
// The pipeline: bearer → authenticate_key → the key's own rpm brake → body
// validation → resolve_route → check_budget (org + caller, key caps
// min-merged under the admin's ceiling) → build_upstream (defaults UNDER the
// client body, secret-vault seal, learned-param pre-strip) → fetch_upstream
// (400 strip-and-retry ≤4, dev hostname fallback) → relay. Non-streaming
// reads,
// meters, relays — annotate/strict rewriting the body before it goes out;
// streaming passes bytes through a metering Stream that scans SSE lines and
// settles the ledger exactly once from wherever the stream ends — clean
// flush, client hangup (Drop), or upstream error.
//
// THE GUARD'S TWO POSTURES. Observe (or an agent-loop key): the guard runs
// detached after the bytes are gone — findings record, response untouched.
// Annotate/strict on a human's key: non-streaming awaits the guard and
// rewrites the body (strict redacts, both append the caveat); streaming
// relays line-wise with [DONE] withheld, then injects the caveat as one
// final delta chunk. AN AGENT'S OWN TOOL LOOP IS NEVER ANNOTATED — a caveat
// mid-loop would contaminate the agent's context; the chat/channel layer
// annotates the human-facing copy instead.

use crate::auth::{authenticate_key, bearer_secret};
use crate::error::{
    BudgetFacts, js_num, log_upstream_error, openai_budget_error, openai_error,
    openai_error_null_param, sanitized_upstream_body, thrown_internal_error,
};
use crate::gateway::budget::{BudgetLimits, budget_message, check_budget};
use crate::gateway::guard::{
    Finding, Grounding, GuardMode, grounding_text_of, guard_completion, guard_config,
    needs_redaction, redact_secrets,
};
use crate::gateway::registry::resolve_route;
use crate::gateway::settings::get_setting_hot;
use crate::gateway::upstream::{Reply, build_upstream, fetch_upstream, js_truthy};
use crate::gateway::usage::{TokenCounts, estimate_tokens, normalize_usage, record_gateway_usage};
use crate::ratelimit::rate_limit;
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use futures_util::Stream;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// 25MB is a sanity ceiling for a chat body — a body past it lands in the
/// same 400 as unparseable JSON.
const BODY_LIMIT: usize = 25 * 1024 * 1024;

// The two Talaria-minted gateway credentials (fleet-brain owns both). Named
// here only as the DEFAULTS for the settings below; operators can extend the
// lists, and any other key — an operator's own — is metered and annotated.
const PERSONA_KEY: &str = "fleet-gateway";
const WORKBENCH_KEY: &str = "workbench-gateway";

pub async fn post(State(state): State<AppState>, req: Request<Body>) -> Response {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let identity = match bearer_secret(auth_header) {
        Some(secret) => authenticate_key(&state.pg, secret).await,
        None => Ok(None), // no header: not even a lookup
    };
    let id = match identity {
        Ok(Some(id)) => id,
        Ok(None) => return openai_error(StatusCode::UNAUTHORIZED, "invalid API key"),
        Err(e) => {
            tracing::error!("[llm/v1/chat] key lookup failed: {e}");
            return thrown_internal_error();
        }
    };

    // Detached last_used_at — same fire-and-forget write as the models route.
    {
        let pg = state.pg.clone();
        let key_id = id.key_id.clone();
        tokio::spawn(async move {
            if let Err(e) =
                sqlx::query("update llm_api_keys set last_used_at = now() where id::text = $1")
                    .bind(&key_id)
                    .execute(&pg)
                    .await
            {
                tracing::warn!("[llm/v1/chat] last_used_at update failed for {key_id}: {e}");
            }
        });
    }

    // The key's own request-rate ceiling (#265) — the cheapest refusal, first.
    // Unset (the default) or 0 skips it; a limiter outage fails open.
    if let Some(rpm) = id.caps.rpm.filter(|v| v.is_finite() && *v != 0.0)
        && let Ok(mut conn) = state.redis().await
    {
        let r = rate_limit(
            &mut conn,
            &format!("gw:key:{}", id.key_id),
            rpm.round() as i64,
            60,
        )
        .await;
        if !r.ok {
            let mut out = openai_error_null_param(
                StatusCode::TOO_MANY_REQUESTS,
                &format!("rate limit exceeded for this key: {rpm} requests per minute"),
                "rate_limit_exceeded",
                "rate_limit_exceeded",
            );
            out.headers_mut().insert(
                header::RETRY_AFTER,
                header::HeaderValue::from_str(&r.retry_after.max(1).to_string())
                    .unwrap_or(header::HeaderValue::from_static("60")),
            );
            return out;
        }
    }

    // Any unreadable/unparseable body is the same 400 — both collapse into
    // one path.
    let bytes = match axum::body::to_bytes(req.into_body(), BODY_LIMIT).await {
        Ok(b) => b,
        Err(_) => return openai_error(StatusCode::BAD_REQUEST, "model and messages are required"),
    };
    let client_body: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return openai_error(StatusCode::BAD_REQUEST, "model and messages are required"),
    };
    if !client_body["model"].is_string() || !client_body["messages"].is_array() {
        return openai_error(StatusCode::BAD_REQUEST, "model and messages are required");
    }
    let model = client_body["model"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let route = match resolve_route(&state.pg, &model).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return openai_error(
                StatusCode::NOT_FOUND,
                &format!("unknown model \"{model}\" — GET /v1/models"),
            );
        }
        Err(e) => {
            tracing::error!("[llm/v1/chat] route resolve failed: {e}");
            return thrown_internal_error();
        }
    };

    // The ceiling, BEFORE anything is spent upstream. Off unless an admin
    // configured a budget or the key set its own cap; the key's caps ride
    // min-merged — an owner can throttle their own key, never out-spend an
    // admin's ceiling.
    let caller = format!("api:{}", id.key_name);
    if let Some(denial) = check_budget(
        &state.pg,
        &caller,
        BudgetLimits {
            tokens: id.caps.tokens,
            usd: id.caps.usd,
        },
    )
    .await
    {
        let mut out = openai_budget_error(
            StatusCode::TOO_MANY_REQUESTS,
            &budget_message(&denial),
            "budget_exceeded",
            "budget_exceeded",
            BudgetFacts {
                scope: denial.scope.to_string(),
                subject: denial.subject.clone(),
                unit: denial.unit.to_string(),
                limit: js_num(denial.limit),
                used: js_num(denial.used),
                window_hours: denial.window_hours,
                via: denial.via.to_string(),
            },
        );
        out.headers_mut().insert(
            header::RETRY_AFTER,
            header::HeaderValue::from_str(&denial.retry_after_seconds.max(1).to_string())
                .unwrap_or(header::HeaderValue::from_static("300")),
        );
        return out;
    }

    let mut call = match build_upstream(&state, &route, &client_body).await {
        Ok(c) => c,
        Err(msg) => {
            // The build can fail on endpoint/credential internals — the caller
            // (an external key holder) gets a fixed sentence, the log gets why.
            log_upstream_error("build", "no-route", &msg);
            return openai_error(
                StatusCode::BAD_GATEWAY,
                "upstream request could not be built",
            );
        }
    };

    // METERING — does this call's spend already reach the ledger from another
    // writer? Only on the personas' key: a chat/channel turn writes one row
    // for the whole turn, so metering the N inner-loop calls behind it counts
    // that turn twice. Everything else is metered HERE because here is the
    // only place it lands.
    let unmetered = get_setting_hot(
        &state.pg,
        "gateway_unmetered_keys",
        json!(["fleet-gateway"]),
    )
    .await;
    let skip_meter = unmetered
        .as_array()
        .map(|a| a.iter().any(|v| v.as_str() == Some(id.key_name.as_str())))
        .unwrap_or(false);

    // ANNOTATION — is the caller an agent's own tool loop? Guard caveats are
    // for humans reading a reply; injecting one into an agent's loop would
    // contaminate its context, so annotate/strict never rewrite these keys'
    // responses (findings still record, and the chat/channel layer annotates
    // the human-facing copy). Both the personas' key and the workbench's are
    // loops.
    let agent_loop_keys = get_setting_hot(
        &state.pg,
        "gateway_agent_loop_keys",
        json!([PERSONA_KEY, WORKBENCH_KEY]),
    )
    .await;
    let is_agent_loop = agent_loop_keys
        .as_array()
        .map(|a| a.iter().any(|v| v.as_str() == Some(id.key_name.as_str())))
        .unwrap_or(false);
    let guard_cfg = guard_config(&state.pg).await;
    let may_annotate = !is_agent_loop && guard_cfg.mode.discloses();

    // Everything a guard call needs, held once for both relay paths.
    let guard_spec = GuardSpec {
        pg: state.pg.clone(),
        caller: caller.clone(),
        model: model.clone(),
        endpoint: route.endpoint.name.clone(),
        messages: client_body["messages"].clone(),
    };

    let prompt_chars = prompt_chars_of(&client_body["messages"]);

    let reply = match fetch_upstream(&state.pg, &mut call, Some(&route)).await {
        Ok(r) => r,
        Err(msg) => {
            // The fetch error can name the endpoint host — endpoint topology
            // is not a key holder's business. Fixed sentence out.
            log_upstream_error("fetch", "unreachable", &msg);
            return openai_error(StatusCode::BAD_GATEWAY, "upstream unreachable");
        }
    };

    let ledger = Ledger {
        pg: state.pg.clone(),
        caller: caller.clone(),
        endpoint: route.endpoint.name.clone(),
        endpoint_class: route.endpoint.class.clone(),
        upstream_model: route.upstream_model.clone(),
        prompt_chars,
        skip: skip_meter,
    };

    // ── Non-streaming: read, meter, relay ─────────────────────────────────
    if !js_truthy(&call.body["stream"]) {
        let status = reply.status();
        let content_type = reply.content_type();
        let mut text = reply.text().await;
        if !(200..300).contains(&status) {
            // The upstream's own error body never crosses to the key holder;
            // verbatim goes to the log. Metering is not gated on ok: a
            // rejected call still reports (and is still billed for) its
            // usage. One without usage books NOTHING — the estimate fallback
            // must not run here, or a rejection would invent spend.
            log_upstream_error("llm-v1", status, &text);
            if let Ok(j) = serde_json::from_str::<Value>(&text)
                && let Some(reported) = normalize_usage(j.get("usage"))
            {
                ledger.spawn(reported, false);
            }
            return fixed_json(status, &sanitized_upstream_body(status, &text));
        }
        if let Ok(mut j) = serde_json::from_str::<Value>(&text) {
            let content = j["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let counts = normalize_usage(j.get("usage"));
            ledger.maybe_spawn(counts, content.encode_utf16().count());
            if may_annotate && !content.is_empty() {
                // Nothing has been relayed yet, so annotate/strict can act on
                // the body itself: strict redacts leaked secrets, and any
                // findings append the human-facing caveat.
                let (findings, caveat, mode) = guard_completion(
                    &state.pg,
                    &content,
                    &client_body["messages"],
                    &caller,
                    &model,
                    Some(&route.endpoint.name),
                )
                .await;
                if !findings.is_empty() && j["choices"][0]["message"].is_object() {
                    // GROUNDED, like the findings above: redaction gets the
                    // same grounding material the findings got, or the two
                    // halves of one rule disagree about the same span.
                    let safe = if mode == GuardMode::Strict && needs_redaction(&findings) {
                        redact_secrets(
                            &content,
                            Some(&Grounding::new(&grounding_text_of(
                                client_body["messages"]
                                    .as_array()
                                    .map(Vec::as_slice)
                                    .unwrap_or_default(),
                            ))),
                        )
                        .0
                    } else {
                        content.clone()
                    };
                    j["choices"][0]["message"]["content"] = json!(format!("{safe}{caveat}"));
                    if let Ok(rewritten) = serde_json::to_string(&j) {
                        text = rewritten;
                    }
                }
            } else {
                // Observe posture (or no content): findings record, response
                // bytes are already relay-ready and stay untouched.
                guard_spec.observe(&content);
            }
        }
        return fixed_json_with_type(status, &text, &content_type);
    }

    // ── Streaming: pass bytes through, scan SSE lines for usage/content ──
    if !reply.is_ok() {
        // Same boundary as the non-streaming relay: a failed hop's body is
        // the upstream's words, not ours to forward. The client asked to
        // stream, but an error is one small JSON body — every OpenAI SDK
        // accepts that.
        let status = reply.status();
        let text = reply.text().await;
        log_upstream_error("llm-v1-stream", status, &text);
        return fixed_json(status, &sanitized_upstream_body(status, &text));
    }
    let Reply::Live(res) = reply else {
        return fixed_json(502, &sanitized_upstream_body(502, ""));
    };
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_string();
    let mode = if may_annotate {
        MeterMode::Annotate
    } else {
        MeterMode::Passthrough
    };
    let stream = MeteredStream::new(res.bytes_stream(), ledger, mode, guard_spec);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .expect("static headers build")
}

/// `Response` with a raw string body — used where the body is already the
/// exact bytes to relay (sanitized upstream errors, verbatim successes).
fn fixed_json(status: u16, body: &str) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("static response builds")
}

fn fixed_json_with_type(status: u16, body: &str, content_type: &str) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body.to_string()))
        .expect("static response builds")
}

/// The prompt-size figure: string content counts its UTF-16 length, anything
/// else counts its JSON serialization (null and absent both serialize as
/// `""`). Key order in that serialization is alphabetical, so the non-string
/// arm can wander a few chars on exotic bodies; it feeds only a chars/4
/// estimate.
fn prompt_chars_of(messages: &Value) -> usize {
    let Some(list) = messages.as_array() else {
        return 0;
    };
    list.iter()
        .map(|m| match &m["content"] {
            Value::String(s) => s.encode_utf16().count(),
            Value::Null => 2, // JSON.stringify('') === '""'
            v => serde_json::to_string(v)
                .map(|s| s.encode_utf16().count())
                .unwrap_or(2),
        })
        .sum()
}

/// The ledger write for one call — spawned, never awaited on the request
/// path.
#[derive(Clone)]
struct Ledger {
    pg: sqlx::PgPool,
    caller: String,
    endpoint: String,
    endpoint_class: String,
    upstream_model: String,
    prompt_chars: usize,
    skip: bool,
}

impl Ledger {
    /// Known usage books as reported; absent usage falls back to the chars/4
    /// estimate over prompt + completion.
    fn maybe_spawn(&self, counts: Option<TokenCounts>, content_chars: usize) {
        if self.skip {
            return;
        }
        let (counts, estimated) = match counts {
            Some(c) => (c, false),
            None => (
                TokenCounts {
                    prompt_tokens: estimate_tokens(self.prompt_chars),
                    completion_tokens: estimate_tokens(content_chars),
                    ..Default::default()
                },
                true,
            ),
        };
        self.spawn(counts, estimated);
    }

    fn spawn(&self, counts: TokenCounts, estimated: bool) {
        if self.skip {
            return;
        }
        let l = self.clone();
        tokio::spawn(async move {
            if let Err(e) = record_gateway_usage(
                &l.pg,
                &l.caller,
                &l.endpoint,
                &l.endpoint_class,
                &l.upstream_model,
                &counts,
                estimated,
            )
            .await
            {
                tracing::warn!("[llm/v1/chat] ledger write failed: {e}");
            }
        });
    }
}

/// Everything one guard call needs — held by the meter so the stream's end,
/// wherever it lands, can run the guard with the same context the handler
/// would.
#[derive(Clone)]
struct GuardSpec {
    pg: sqlx::PgPool,
    caller: String,
    model: String,
    endpoint: String,
    messages: Value,
}

impl GuardSpec {
    fn call(&self, answer: &str) -> GuardFut {
        let s = self.clone();
        let answer = answer.to_string();
        Box::pin(async move {
            guard_completion(
                &s.pg,
                &answer,
                &s.messages,
                &s.caller,
                &s.model,
                Some(&s.endpoint),
            )
            .await
        })
    }

    /// Observe posture: fire-and-forget, findings record, nobody is told.
    /// `try_current` because Drop can run outside the runtime at shutdown —
    /// losing the guard there is acceptable, panicking is not.
    fn observe(&self, answer: &str) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let fut = self.call(answer);
            handle.spawn(async move {
                let _ = fut.await;
            });
        }
    }
}

type GuardFut = Pin<Box<dyn Future<Output = (Vec<Finding>, String, GuardMode)> + Send>>;

/// Which relay the stream is. Passthrough relays bytes untouched (observe
/// posture or an agent-loop key); annotate relays line-wise with [DONE]
/// withheld so a caveat can land before it.
#[derive(PartialEq, Clone, Copy)]
enum MeterMode {
    Passthrough,
    Annotate,
}

/// The meter: SSE `data:` lines are scanned for usage/content while the bytes
/// relay, and the ledger settles once, from wherever the stream ends — a
/// clean flush, a client hangup (this struct's Drop), or an upstream error
/// frame. The provider bills a hung-up stream exactly the same, so the ledger
/// books it too — exactly once, whichever fires first. Only the CLEAN end
/// scans the pending tail; settle books, it does not scan.
struct MeteredStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    /// Partial line BYTES across chunk boundaries — buffering bytes (not
    /// lossy-decoded text) keeps a multibyte character split across chunks
    /// from degrading into U+FFFD.
    buf: Vec<u8>,
    content: String,
    usage: Option<Value>,
    settled: bool,
    ledger: Ledger,
    mode: MeterMode,
    /// [DONE] was seen and is being withheld until the guard has spoken.
    saw_done: bool,
    /// Re-emitted line bytes (annotate mode) and the caveat/[DONE] tail —
    /// drained before the upstream is polled again.
    out: VecDeque<Bytes>,
    /// The guard future, armed only at a clean annotate-mode end with content.
    guard: Option<GuardFut>,
    /// Upstream returned None — the flush path has run.
    ended: bool,
    /// Upstream errored mid-stream — neither flush nor cancel runs a guard.
    errored: bool,
    guard_spec: GuardSpec,
}

/// A data: line whose payload is exactly [DONE].
fn line_is_done(line: &[u8]) -> bool {
    line.strip_prefix(b"data:")
        .is_some_and(|rest| String::from_utf8_lossy(rest).trim() == "[DONE]")
}

/// The caveat as one final delta chunk — the byte shape is the contract
/// (preserve_order keeps the key order; pinned byte-for-byte below).
fn guard_caveat_chunk(caveat: &str, model: &str) -> Bytes {
    let chunk = json!({
        "id": "talaria-guard",
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [{ "index": 0, "delta": { "content": caveat }, "finish_reason": null }],
    });
    Bytes::from(format!("data: {chunk}\n\n"))
}

impl MeteredStream {
    fn new(
        inner: impl Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
        ledger: Ledger,
        mode: MeterMode,
        guard_spec: GuardSpec,
    ) -> Self {
        MeteredStream {
            inner: Box::pin(inner),
            buf: Vec::new(),
            content: String::new(),
            usage: None,
            settled: false,
            ledger,
            mode,
            saw_done: false,
            out: VecDeque::new(),
            guard: None,
            ended: false,
            errored: false,
            guard_spec,
        }
    }

    fn scan_line(&mut self, line: &str) {
        let Some(data) = line.strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data == "[DONE]" {
            return;
        }
        let Ok(j) = serde_json::from_str::<Value>(data) else {
            return;
        }; // partial line — ignore
        if j.get("usage").map(|u| !u.is_null()).unwrap_or(false) {
            self.usage = j.get("usage").cloned();
        }
        if let Some(delta) = j["choices"][0]["delta"]["content"].as_str() {
            self.content.push_str(delta);
        }
    }

    fn on_chunk(&mut self, chunk: &Bytes) {
        self.buf.extend_from_slice(chunk);
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            // The drained line carries its own \n (and any \r before it);
            // passthrough ignores it, annotate re-emits exactly those bytes.
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let body = &line[..line.len() - 1];
            self.scan_line(&String::from_utf8_lossy(body));
            if self.mode == MeterMode::Annotate {
                if line_is_done(body) {
                    self.saw_done = true;
                } else {
                    self.out.push_back(Bytes::from(line));
                }
            }
        }
    }

    /// The clean-end tail: scanned, and in annotate mode re-emitted verbatim
    /// — no newline added.
    fn flush_tail(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let tail = std::mem::take(&mut self.buf);
        self.scan_line(&String::from_utf8_lossy(&tail));
        if self.mode == MeterMode::Annotate {
            if line_is_done(&tail) {
                self.saw_done = true;
            } else {
                self.out.push_back(Bytes::from(tail));
            }
        }
    }

    /// Book the ledger — once, from whichever end the stream comes to. The
    /// pending line is NOT scanned here: settle books only, and a cancelled
    /// or errored stream's partial tail was never a complete frame.
    fn settle(&mut self) {
        if self.settled {
            return;
        }
        self.settled = true;
        self.ledger.maybe_spawn(
            normalize_usage(self.usage.as_ref()),
            self.content.encode_utf16().count(),
        );
    }
}

impl Stream for MeteredStream {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // SAFETY: no field of Self is structurally pinned — the only
        // address-sensitive data lives in `inner`, a Pin<Box> that pins its
        // own pointee, and no other field's address is ever handed out.
        let this = unsafe { self.get_unchecked_mut() };
        loop {
            if let Some(chunk) = this.out.pop_front() {
                return Poll::Ready(Some(Ok(chunk)));
            }
            if this.ended && this.guard.is_none() {
                return Poll::Ready(None);
            }
            // Annotate-mode clean end: await the guard, then the caveat (if
            // any) and the withheld [DONE] go out as the stream's last bytes.
            if let Some(g) = this.guard.as_mut() {
                if let Poll::Ready((_, caveat, _)) = g.as_mut().poll(cx) {
                    this.guard = None;
                    if !caveat.is_empty() {
                        this.out
                            .push_back(guard_caveat_chunk(&caveat, &this.guard_spec.model));
                    }
                    if this.saw_done {
                        this.out.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
                    }
                    continue;
                }
                return Poll::Pending;
            }
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.on_chunk(&chunk);
                    if this.mode == MeterMode::Passthrough {
                        // Passthrough: the client's bytes, untouched.
                        return Poll::Ready(Some(Ok(chunk)));
                    }
                    // Annotate: re-emitted lines come off the out queue.
                    continue;
                }
                Poll::Ready(Some(Err(e))) => {
                    // An error frame runs neither flush nor cancel — the
                    // spend settles, the guard does not run.
                    this.errored = true;
                    this.settle();
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(None) => {
                    // Clean end: scan the tail, settle, run the guard.
                    this.ended = true;
                    this.flush_tail();
                    this.settle();
                    match this.mode {
                        MeterMode::Passthrough => {
                            this.guard_spec.observe(&this.content);
                        }
                        MeterMode::Annotate if this.content.is_empty() => {
                            if this.saw_done {
                                this.out.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
                            }
                        }
                        MeterMode::Annotate => {
                            this.guard = Some(this.guard_spec.call(&this.content));
                        }
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl Drop for MeteredStream {
    fn drop(&mut self) {
        self.settle();
        // The cancel path: the client walked away mid-stream. The spend
        // settles above; in annotate mode the guard still runs observe-posture
        // on what assembled — the caveat can't reach a stream nobody is
        // reading, but the findings still record. A clean end already ran the
        // guard; an upstream error frame runs nothing.
        if self.mode == MeterMode::Annotate && !self.ended && !self.errored {
            self.guard_spec.observe(&self.content);
        }
    }
}

// The scanner is pure line-processing — testable without a socket.
#[cfg(test)]
mod tests {
    use super::*;

    fn scanner_mode(mode: MeterMode) -> MeteredStream {
        // skip:true — settle runs its scan logic, the ledger write doesn't
        // (no DB in unit tests; the write itself is exercised live).
        let ledger = Ledger {
            pg: sqlx::PgPool::connect_lazy("postgres://x").unwrap(),
            caller: "api:test".into(),
            endpoint: "e".into(),
            endpoint_class: "cloud".into(),
            upstream_model: "m".into(),
            prompt_chars: 0,
            skip: true,
        };
        let guard_spec = GuardSpec {
            pg: sqlx::PgPool::connect_lazy("postgres://x").unwrap(),
            caller: "api:test".into(),
            model: "test-model".into(),
            endpoint: "e".into(),
            messages: json!([]),
        };
        MeteredStream::new(futures_util::stream::empty(), ledger, mode, guard_spec)
    }

    fn scanner() -> MeteredStream {
        scanner_mode(MeterMode::Passthrough)
    }

    // #[tokio::test]: constructing the ledger's lazy PgPool arms sqlx's pool
    // reaper, which needs a runtime — the scan logic itself is pure.
    #[tokio::test]
    async fn sse_scan_assembles_content_and_usage_across_chunk_splits() {
        let mut s = scanner();
        // A usage frame, then a content delta whose `é` (0xC3 0xA9) is split
        // MID-CHARACTER across the two chunks, then [DONE]. Byte-buffering
        // (not lossy-decoding per chunk) is what reassembles it.
        s.on_chunk(&Bytes::from_static(
            b"data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"h\xc3",
        ));
        s.on_chunk(&Bytes::from_static(b"\xa9llo\"}}]}\n\ndata: [DONE]\n\n"));
        assert_eq!(s.content, "héllo");
        assert_eq!(
            s.usage.as_ref().unwrap()["prompt_tokens"],
            serde_json::json!(10)
        );
        // settle books only — it must be idempotent and scan nothing new.
        s.settle();
        s.settle();
        assert!(s.settled);
    }

    #[tokio::test]
    async fn partial_and_non_json_lines_are_ignored() {
        let mut s = scanner();
        s.on_chunk(&Bytes::from_static(
            b": keep-alive comment\ndata: not-json{",
        ));
        assert_eq!(s.content, "");
        assert!(s.usage.is_none());
        // The tail stays pending — settle books only; the CLEAN end scans it.
        assert!(!s.buf.is_empty());
        s.settle();
        assert!(!s.buf.is_empty()); // still pending — a cancel would drop it
        s.flush_tail();
        assert!(s.buf.is_empty()); // flushed (and still ignored — not JSON)
    }

    #[tokio::test]
    async fn annotate_relays_lines_with_done_withheld_and_tail_verbatim() {
        let mut s = scanner_mode(MeterMode::Annotate);
        s.on_chunk(&Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
        ));
        // The delta line re-emits byte-for-byte; the blank separator line too;
        // [DONE] is withheld; the blank AFTER [DONE] still goes out (it is a
        // line like any other — only [DONE] itself is withheld).
        assert_eq!(
            s.out.pop_front().unwrap(),
            Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n")
        );
        assert_eq!(s.out.pop_front().unwrap(), Bytes::from_static(b"\n"));
        assert_eq!(s.out.pop_front().unwrap(), Bytes::from_static(b"\n"));
        assert!(s.out.is_empty());
        assert!(s.saw_done);
        assert_eq!(s.content, "hi");
        // A partial tail at the clean end goes out verbatim, no newline added.
        s.on_chunk(&Bytes::from_static(b"data: {\"choices\""));
        s.flush_tail();
        assert_eq!(
            s.out.pop_front().unwrap(),
            Bytes::from_static(b"data: {\"choices\"")
        );
        // A [DONE] tail is withheld like a [DONE] line.
        s.buf = b"data: [DONE]".to_vec();
        s.flush_tail();
        assert!(s.out.is_empty());
        assert!(s.saw_done);
    }

    #[test]
    fn done_detection_and_the_caveat_chunk_shape() {
        assert!(line_is_done(b"data: [DONE]"));
        assert!(line_is_done(b"data:[DONE]"));
        assert!(line_is_done(b"data: [DONE]\r")); // \r belongs to the line
        assert!(!line_is_done(b"data: {\"x\":1}"));
        assert!(!line_is_done(b": [DONE]"));
        // The caveat chunk is byte-exact — the pinned bytes are the contract.
        let chunk = guard_caveat_chunk("\n\n--- caveat", "claude-3");
        assert_eq!(
            chunk,
            Bytes::from_static(
                b"data: {\"id\":\"talaria-guard\",\"object\":\"chat.completion.chunk\",\"model\":\"claude-3\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\\n\\n--- caveat\"},\"finish_reason\":null}]}\n\n"
            )
        );
    }

    #[test]
    fn prompt_chars_counts_utf16_and_json_serializations() {
        let msgs = serde_json::json!([
            {"role": "user", "content": "héllo"},          // 5 UTF-16 units
            {"role": "user", "content": {"text": "x"}},    // JSON: {"text":"x"} = 12
            {"role": "user"},                              // absent → "" → 2
            {"role": "user", "content": null},             // null → "" → 2
        ]);
        assert_eq!(prompt_chars_of(&msgs), 21);
    }
}
