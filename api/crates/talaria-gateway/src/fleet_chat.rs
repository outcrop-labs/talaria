// The fleet chat transport — proxy_chat, proxy_session_turn, and the agent SSE
// stream parser the pair of them share: the comms reply loop drives one of the
// two and reads the same events out either way. The difference is only who
// keeps the transcript — we do, as a `messages` array, or the agent does, as
// a session we send one message at a time.
//
// A session turn can also be reached while it runs: steer_run pushes words
// into it and stop_run interrupts it, both naming the run id the stream
// announced. They live here because they go through the same door with the
// same bearer as the turn itself — as does agent_request, the plain json call
// other crates make on that door (talaria-agent-crons drives Hermes's own
// `/api/jobs` with it).
//
// Every Hermes persona turn in the tree is sent from here to the agent's own
// container, which then talks to a provider we do not control the request
// assembly for — the door that matters most, because a persona holds
// workspace context all day and is the agent most likely to have a credential
// in front of it.
//
// WHY THIS IS NOT THE GATEWAY RELAY: the manifest entry IS the agent — a
// container with its own local gateway at `url`. The registry relay
// (routes/llm_chat.rs) serves workspace API keys against provider endpoints;
// this path serves TALKING TO THE AGENTS THEMSELVES, and only the comms/chat
// planes do it.
//
// WHERE A TURN'S SPEND LANDS: not here. The relay books every call the agent
// makes on its own gateway key; what this door adds is WHOSE turn those calls
// belong to. Each caller hands over its `TurnAttribution` (the flow's source,
// conversation or run, ticket), and the turn's conversation id — the session
// id we created, or the id Hermes answers in `X-Hermes-Session-Id` on the
// completions arm — is recorded under it. The agent's talaria-events plugin
// ties the affinity key Hermes stamps on the turn's calls to that same id, so
// the relay can stamp the flow on every row it books (usage.rs, "Attribution").

use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::fleet_lost_turns::{LostTurnRetry, heal_lost_turn};
use crate::provider::http;
use crate::usage::{TurnAttribution, affinity_key, backfill_attribution, record_attribution};
use crate::vault::{SecretVault, seal_content};
use talaria_fleet_layout::{ManifestEntry, describe_agent, read_manifest};

/// The verb of an `agent_request`, re-exported so a caller names one without
/// a reqwest dependency of its own.
pub use reqwest::Method;

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

pub type ByteStream = Pin<Box<dyn futures_util::Stream<Item = reqwest::Result<Bytes>> + Send>>;

/// How long the turn waits for the agent gateway to OPEN the stream (the
/// response's headers) before the retry/deadline loop takes over. Generous on
/// purpose: the agent's first frame rides its provider's time-to-first-token,
/// and a long context — or a mid-task compaction — can hold that for minutes.
/// Only silence past this reads as "not serving yet".
const HEADERS_WINDOW: Duration = Duration::from_secs(300);
/// The bound on a steer or a stop. A control call names a run that is live
/// at this instant and the container answers it in milliseconds or not at
/// all; the drain loop fires these from inside a stream it must keep
/// reading, and a route answers a person who just pressed the button.
/// Neither can wait the turn's five minutes on a container that accepted
/// TCP and went quiet.
pub(crate) const CONTROL_WINDOW: Duration = Duration::from_secs(10);

/// One OpenAI-style streamed reply, or a canned one. `status` is the upstream
/// (or canned = 200) status; the body is raw SSE bytes the caller parses with
/// `AgentStreamParser`. `content_type` is what the upstream answered,
/// defaulting to text/event-stream — the chat route's own headers read it
/// back.
pub struct ChatStream {
    pub status: u16,
    pub content_type: String,
    pub body: ByteStream,
    /// `"mock"` / `"hold"` when this is a CANNED stream. A canned stream is
    /// an outage wearing a 200: right for a human chat window,
    /// indistinguishable from a model reply to everything downstream of
    /// `ok()`. The persona transport reads this and fails the call so the
    /// sentence can never be persisted as an agent's work.
    pub canned: Option<&'static str>,
    /// The conversation id Hermes answered in `X-Hermes-Session-Id` — on the
    /// completions arm, Hermes' own id for the conversation (`api-<digest>`
    /// of its opening), which is the key every call of the turn is metered
    /// under. None on a canned or synthesized reply.
    pub hermes_session: Option<String>,
}

impl ChatStream {
    /// A 2xx answer is the stream the caller wants; every other status is
    /// the reply, body and all, but not one to persist.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The chat plane's payload — `{model, messages}` plus the effort when one
/// was honored, in that key order.
pub fn chat_payload(model: &str, messages: &Value, effort: Option<&str>) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("model".into(), json!(model));
    payload.insert("messages".into(), messages.clone());
    if let Some(options) = persona_model_options(effort) {
        payload.insert("model_options".into(), options);
    }
    Value::Object(payload)
}

/// THE ONE PLACE A PER-TURN EFFORT IS SPELLED FOR A PERSONA: Hermes' API
/// server reads a request's effort from `model_options.reasoning_effort` and
/// from nowhere else — a top-level `reasoning_effort` (the OpenAI spelling)
/// is ignored on `/v1/chat/completions` and on the session-chat route alike,
/// and the agent runs at its configured default instead
/// (`gateway/platforms/api_server_request_options.py`, Hermes v0.22). Both
/// arms — `chat_payload`/the harness's `persona_payload` and the session
/// body — spend it through here so the two cannot spell it apart.
pub fn persona_model_options(effort: Option<&str>) -> Option<Value> {
    effort.map(|e| json!({ "reasoning_effort": e }))
}

/// Send one chat turn to the agent's container. Seals every
/// message's content through a per-call vault (an image turn's text part is as
/// credential-prone as any prose turn), retries while the agent's gateway is
/// coming up (502/503/504, backoff capped at 5s), and never waits past two
/// minutes — past the deadline the reply is the honest canned sentence, so
/// history shows what happened rather than a silent failure.
///
/// `payload` is the WHOLE request body the agent's gateway sees — the turn
/// payload plus `stream: true` and `stream_options` — so whatever a persona
/// turn carries beside model and messages (`model_options` for its effort)
/// is decided by the one mapping that builds it (`persona_payload`,
/// `chat_payload`). `messages` is the OpenAI history (system + turns, content
/// string or parts). `wait_ms` is the hold window; None is the two-minute
/// default.
///
/// `attribution` is the flow this turn belongs to: the conversation key
/// Hermes answers is recorded under it, so every call the agent makes for
/// the turn is metered as that flow's. None leaves the turn's calls
/// `unattended` at the relay — still booked, under the agent.
pub async fn proxy_chat(
    payload: &Value,
    wait_ms: Option<u64>,
    attribution: Option<TurnAttribution>,
) -> ChatStream {
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // CREDENTIALS DO NOT LEAVE THIS PROCESS, PERSONA EDITION: the vault is
    // per-call and discarded when this returns — nothing downstream of a
    // persona turn spends a handle.
    let mut vault = SecretVault::default();
    let mut payload = payload.clone();
    if let Some(list) = payload.get_mut("messages").and_then(Value::as_array_mut) {
        for m in list.iter_mut() {
            if let Some(content) = m.get_mut("content") {
                *content = seal_content(&content.clone(), &mut vault);
            }
        }
    }
    report_sealed(&vault, &model);

    match hold_for_agent(
        &model,
        AgentRequest::ChatCompletions { payload },
        wait_ms,
        attribution,
    )
    .await
    {
        Ok(stream) => stream,
        // The completions arm names no session, so it has none to lose; the
        // canned sentence is the honest answer if the type ever says
        // otherwise.
        Err(SessionLost) => canned_chat_stream(&unavailable_reply(&model), "hold"),
    }
}

/// Send one turn on a Hermes SESSION — the arm for an agent that keeps its own
/// conversation history. Talaria sends only the NEW message plus the turn's
/// ephemeral system prompt; the transcript itself lives in the container,
/// keyed by `conversation_id`. That is why this is not `proxy_chat` with
/// another path: there is no `messages` array to assemble here, so there is no
/// way for our idea of what was said to drift from the agent's.
///
/// THE KEY IS THE CALLER'S, and it is also the id the agent's talaria-events
/// plugin reports every tool frame against: a pinned chat keys the session by
/// its conversation id, a work session by its RUN id (the harness transport's
/// `PersonaSession`), so a frame at the tool-events door names the thing it
/// belongs to.
///
/// Everything else is `proxy_chat`'s contract, deliberately: the session is
/// created on the way in (idempotently — an existing one answers 409, which is
/// the steady state after the first turn), a container that isn't serving yet
/// is held for, the deadline answers with the honest canned sentence, and no
/// manifest row at all is mock mode. `message` is a string or OpenAI content
/// parts; `system_message` is the per-turn prompt Hermes calls ephemeral. Both
/// are sealed through a per-call vault before they leave the process.
/// `effort` rides as `model_options`, the one spelling Hermes reads
/// ([`persona_model_options`]).
///
/// `history_expected` says the conversation has been driven through this
/// session before, so the session MUST already exist in the container. Such a
/// session is LOOKED UP, never created: one that is not there means the state
/// volume was lost (a re-hire on the same slug, a swapped volume, a host
/// restore) or the session was deleted, and Hermes would answer from an empty
/// room while the transcript on screen says otherwise. That turn is `None` —
/// the caller sends the full history instead — rather than a stream from a
/// session that knows nothing. Nothing is created on the way out either: an
/// empty room made here would be found by the NEXT turn, which would then send
/// only its new message into it. A create Hermes refuses as `session_deleted`
/// is `None` the same way, since v0.22 never lets a deleted id be reused.
///
/// A turn Hermes refuses because the session LOST an earlier turn in a
/// non-graceful restart is repaired here, once, before the caller sees it
/// (`heal_lost_turn`).
///
/// `attribution` is `proxy_chat`'s: here the key is `conversation_id`
/// itself, recorded before the first attempt so the agent's first call
/// already finds it.
#[allow(clippy::too_many_arguments)] // one turn's facts, each named at the call site
pub async fn proxy_session_turn(
    agent_model: &str,
    conversation_id: &str,
    system_message: Option<&str>,
    message: &Value,
    effort: Option<&str>,
    wait_ms: Option<u64>,
    history_expected: bool,
    attribution: Option<TurnAttribution>,
) -> Option<ChatStream> {
    let mut vault = SecretVault::default();
    let body = session_turn_body(message, system_message, effort, &mut vault);
    report_sealed(&vault, agent_model);

    hold_for_agent(
        agent_model,
        AgentRequest::SessionTurn {
            conversation_id: conversation_id.to_string(),
            body,
            history_expected,
        },
        wait_ms,
        attribution,
    )
    .await
    .ok()
}

/// The session turn's request body, sealed. The system prompt goes through the
/// same sealer as the message because it is assembled from workspace state — a
/// task's own text, a board's titles — and is exactly as likely to carry a
/// credential as anything a person typed.
fn session_turn_body(
    message: &Value,
    system_message: Option<&str>,
    effort: Option<&str>,
    vault: &mut SecretVault,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("message".into(), seal_content(message, vault));
    if let Some(system) = system_message {
        body.insert("system_message".into(), seal_content(&json!(system), vault));
    }
    if let Some(options) = persona_model_options(effort) {
        body.insert("model_options".into(), options);
    }
    Value::Object(body)
}

/// Say what the vault took out of an outbound turn — one warn line per
/// credential. A persona echoing a key back at us is the event this makes
/// legible, and both arms of the transport report it the same way.
fn report_sealed(vault: &SecretVault, model: &str) {
    for s in &vault.sealed {
        tracing::warn!("[secrets] sealed {} out of a turn to {model}", s.label);
    }
}

/// What a held turn SENDS once the agent's manifest row is in hand: the OpenAI
/// chat body, or one turn on a Hermes session. The waiting, the mock fallback
/// and the canned sentence are identical for both — only the path and the body
/// differ — so the two shapes are an enum over one loop rather than a second
/// copy of the loop that drifts from this one.
enum AgentRequest {
    ChatCompletions {
        payload: Value,
    },
    SessionTurn {
        conversation_id: String,
        body: Value,
        /// The conversation was driven through this session before, so the
        /// session must already be there: it is looked up, never created,
        /// and a lookup that finds nothing is a lost session, not a first
        /// turn.
        history_expected: bool,
    },
}

/// The pinned session this turn expected to find is gone: the lookup of a
/// session that carried turns answered `session_not_found` (a lost state
/// volume), or a create was refused as `session_deleted`. Not a hold and not a
/// canned reply — the caller has the full history in hand and sends it the
/// other way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLost;

/// One attempt's answer, for the hold loop.
enum Opened {
    Stream(ChatStream),
    /// Not serving yet — back off and try again until the deadline.
    NotYet,
    Lost,
}

impl AgentRequest {
    /// One attempt against this agent's own gateway. `NotYet` means "not
    /// serving yet" — the hold loop backs off and tries again until its
    /// deadline.
    async fn open(&self, entry: &ManifestEntry) -> Opened {
        match self {
            AgentRequest::ChatCompletions { payload } => {
                let mut request = payload.clone();
                // include_usage: the final chunk reports token counts for the
                // ledger (gateways that don't support it just ignore the option).
                request["stream"] = json!(true);
                request["stream_options"] = json!({ "include_usage": true });
                match open_stream(agent_post(entry, "/v1/chat/completions"), &request).await {
                    Some(stream) => Opened::Stream(stream),
                    None => Opened::NotYet,
                }
            }
            AgentRequest::SessionTurn {
                conversation_id,
                body,
                history_expected,
            } => {
                // The session must exist before a turn can land on it, and the
                // look-up (or the create) is the only place we can learn that
                // it doesn't. A refusal here is treated exactly like a gateway
                // that isn't serving yet — on a container that is still
                // booting that is precisely what it is.
                match ensure_session(entry, conversation_id, *history_expected).await {
                    SessionCreate::NotReady => return Opened::NotYet,
                    SessionCreate::Existing | SessionCreate::Fresh => {}
                    // The room is gone: the state-volume loss M5 names, or a
                    // deleted session. Said once per conversation, because the
                    // caller comes back this way every turn from now on.
                    SessionCreate::Lost(why) => {
                        if lost_sessions_first_sight(conversation_id) {
                            tracing::warn!(
                                "[fleet] {}: session {conversation_id} is gone — {why}; this \
                                 conversation is sent with its full history instead",
                                entry.url
                            );
                        }
                        return Opened::Lost;
                    }
                }
                let path = format!("/api/sessions/{conversation_id}/chat/stream");
                match open_stream(agent_post(entry, &path), body).await {
                    Some(stream) if stream.ok() => Opened::Stream(heal_lost_turn(
                        stream,
                        LostTurnRetry::over_http(entry, conversation_id, path, body.clone()),
                    )),
                    Some(stream) => Opened::Stream(stream),
                    None => Opened::NotYet,
                }
            }
        }
    }
}

/// Conversations whose pinned session was found lost, so the warning above
/// is said once per process rather than once per turn.
static LOST_SESSIONS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

fn lost_sessions_first_sight(conversation_id: &str) -> bool {
    LOST_SESSIONS
        .lock()
        .map(|mut set| set.insert(conversation_id.to_string()))
        .unwrap_or(true)
}

/// The hold: retry while the agent's gateway is coming up (502/503/504,
/// backoff capped at 5s) and never wait past the window — past the deadline
/// the reply is the honest canned sentence, so history shows what happened
/// rather than a silent failure. `wait_ms` None is the two-minute default.
/// `Err(SessionLost)` is the session arm's one way out that is not a stream:
/// the room it expected is gone, and the caller decides what to send instead.
///
/// THE TURN'S KEY IS RECORDED HERE — the one door every Talaria-driven
/// persona turn goes through (both arms, every caller: chat, channel,
/// harness). The session arm knows its key before it sends anything, so it
/// is recorded before the first attempt; the completions arm learns it from
/// the response headers, and records it then, with a backfill for the calls
/// that finished first. Either way the stream carries a second backfill to
/// its end — clean, hung up, errored — for a call that raced the record.
async fn hold_for_agent(
    model: &str,
    request: AgentRequest,
    wait_ms: Option<u64>,
    attribution: Option<TurnAttribution>,
) -> Result<ChatStream, SessionLost> {
    if let (
        Some(a),
        AgentRequest::SessionTurn {
            conversation_id, ..
        },
    ) = (&attribution, &request)
        && let Some(key) = affinity_key(Some(conversation_id))
    {
        record(a, &key).await;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms.unwrap_or(120_000));
    let mut attempt: u32 = 0;
    loop {
        let manifest = read_manifest().await;
        let entry = manifest.iter().find(|m| m.model == model);
        // Two strikes with no manifest row at all → mock mode: no agents are
        // rendered yet, and the reply says so instead of hanging.
        if entry.is_none() && attempt >= 2 {
            return Ok(canned_chat_stream(&mock_reply(model), "mock"));
        }

        if let Some(entry) = entry {
            match request.open(entry).await {
                Opened::Stream(stream) => {
                    // Hermes' approval requests on this turn are refused
                    // here, and recorded (fleet_approvals.rs).
                    let subject = turn_subject(&manifest, entry);
                    let stream = crate::fleet_approvals::refuse_approvals(stream, entry, &subject);
                    return Ok(attribute_stream(stream, &request, attribution).await);
                }
                Opened::Lost => return Err(SessionLost),
                Opened::NotYet => {}
            }
        }

        if tokio::time::Instant::now() >= deadline {
            return Ok(canned_chat_stream(&unavailable_reply(model), "hold"));
        }
        attempt += 1;
        tokio::time::sleep(Duration::from_millis(
            (1_500u64 * attempt as u64).min(5_000),
        ))
        .await;
    }
}

/// The key a turn's calls reach the relay under: the session id we created,
/// or on the completions arm the id Hermes answered for the conversation.
/// None when the completions arm answered no usable id (a Hermes without the
/// header, a synthesized error) — the turn's calls then stay `unattended`.
fn turn_key(request: &AgentRequest, stream: &ChatStream) -> Option<String> {
    match request {
        AgentRequest::SessionTurn {
            conversation_id, ..
        } => affinity_key(Some(conversation_id)),
        AgentRequest::ChatCompletions { .. } => affinity_key(stream.hermes_session.as_deref()),
    }
}

/// The name a turn is registered under: the agent's model id — the relay's
/// subject for that agent's key, and the ledger namespace its rows land in.
/// A tier is dialed as its own manifest row, `<agent model>-<tier>`, pointing
/// at the same container as the agent's row; the agent's row is the shortest
/// model sharing the container's url, so a tiered turn registers under the
/// agent and not under a name the relay never sees.
pub(crate) fn turn_subject(manifest: &[ManifestEntry], entry: &ManifestEntry) -> String {
    manifest
        .iter()
        .filter(|m| m.url == entry.url)
        .map(|m| m.model.as_str())
        .min_by_key(|m| m.len())
        .unwrap_or(entry.model.as_str())
        .to_string()
}

/// Record a turn's key, saying so when it cannot: the turn goes on, and its
/// calls are still booked — under the agent, `unattended`.
async fn record(a: &TurnAttribution, key: &str) {
    if let Err(e) = record_attribution(a, key).await {
        tracing::warn!(
            "[fleet] {}: could not record turn {key} as {} spend: {e}",
            a.agent_model,
            a.source
        );
    }
}

/// Claim the rows already booked under `key`, detached. `try_current`: a
/// stream dropped outside the runtime at shutdown loses its backfill, which
/// leaves those rows `unattended` — still counted, never panicking.
fn spawn_backfill(pg: &sqlx::PgPool, agent_model: &str, key: &str) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let (pg, agent_model, key) = (pg.clone(), agent_model.to_string(), key.to_string());
    handle.spawn(async move {
        if let Err(e) = backfill_attribution(&pg, &agent_model, &key).await {
            tracing::warn!("[fleet] {agent_model}: backfilling turn {key} failed: {e}");
        }
    });
}

/// Tie a turn's attribution to the stream it covers: the completions arm's
/// key is recorded now that the headers named it, and either arm's stream
/// backfills once more when the consumer lets go of it.
async fn attribute_stream(
    mut stream: ChatStream,
    request: &AgentRequest,
    attribution: Option<TurnAttribution>,
) -> ChatStream {
    let Some(a) = attribution else {
        return stream;
    };
    let Some(key) = turn_key(request, &stream) else {
        return stream;
    };
    if matches!(request, AgentRequest::ChatCompletions { .. }) {
        record(&a, &key).await;
        spawn_backfill(&a.pg, &a.agent_model, &key);
    }
    let inner = std::mem::replace(&mut stream.body, Box::pin(futures_util::stream::empty()));
    stream.body = Box::pin(Attributed {
        inner,
        pg: a.pg,
        agent_model: a.agent_model,
        key,
    });
    stream
}

/// The stream with its turn's key riding along, so the end of the read —
/// however it ends — claims any row that raced the record.
struct Attributed {
    inner: ByteStream,
    pg: sqlx::PgPool,
    agent_model: String,
    key: String,
}

impl futures_util::Stream for Attributed {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

impl Drop for Attributed {
    fn drop(&mut self) {
        spawn_backfill(&self.pg, &self.agent_model, &self.key);
    }
}

/// An authenticated POST to one agent's own gateway. The session arm and the
/// chat arm go through the SAME door with the SAME bearer — the manifest row's
/// key, which the renderer put in the container as its API_SERVER_KEY — so the
/// key handling can only be got right or wrong once.
pub(crate) fn agent_post(entry: &ManifestEntry, path: &str) -> reqwest::RequestBuilder {
    let req = http()
        .post(format!("{}{path}", entry.url))
        .header("content-type", "application/json");
    with_bearer(entry, req)
}

/// The same door, read-only — the session reads (`/api/sessions/{id}` and its
/// messages) ask Hermes about a transcript it is already holding. Same bearer
/// as the POST side for the same reason: one place to get the key right.
pub(crate) fn agent_get(entry: &ManifestEntry, path: &str) -> reqwest::RequestBuilder {
    with_bearer(entry, http().get(format!("{}{path}", entry.url)))
}

/// The manifest row's key as the request's bearer — the key the renderer put
/// in the container as its API_SERVER_KEY. An empty key is no key: a local
/// container with auth off takes the request bare.
fn with_bearer(entry: &ManifestEntry, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match entry.key.as_deref().filter(|k| !k.is_empty()) {
        Some(key) => req.bearer_auth(key),
        None => req,
    }
}

/// Open the streamed response, or report that this agent isn't serving yet.
///
/// NO TOTAL TIMEOUT ON THE TURN. A per-request .timeout() owns the body end to
/// end, and an agent turn is one long stream: a knowledgebase buildout works
/// for an hour, and the 600s ceiling this replaces killed it at ten minutes
/// while the container was still demonstrably working — the API hung up on a
/// live agent and called the turn "interrupted". Liveness is the DRAIN LOOP's
/// idle ceiling (chat_persist's STREAM_IDLE): as long as frames arrive, the
/// turn runs; only silence ends it. What still needs a bound here is the wait
/// for the response's HEADERS — a container that accepted the request and
/// never opened the stream — because connect_timeout covers TCP only.
///
/// `None` MEANS "NOTHING WAS DELIVERED": the connection was refused or reset
/// before any byte of a response, which is the one case a re-POST cannot
/// double — the container is restarting. Everything else is an answer. A
/// 502/503/504 or a response whose headers never arrive inside the window
/// comes AFTER the request was sent and possibly accepted: the agent may be
/// executing the turn, and sending the same body again would put a second
/// copy of it into the session, invisibly to the step's checkpoint. Those
/// come back as the error they are (the upstream status as-is, or a
/// synthesized 504 for the silent one) and the caller fails the turn.
pub(crate) async fn open_stream(req: reqwest::RequestBuilder, body: &Value) -> Option<ChatStream> {
    let upstream = match tokio::time::timeout(HEADERS_WINDOW, req.json(body).send()).await {
        Ok(Ok(upstream)) => upstream,
        Ok(Err(e)) if failed_before_any_byte(&e) => return None,
        Ok(Err(e)) => {
            return Some(error_stream(
                502,
                format!(
                    "the agent gateway failed after the request was sent: {}",
                    transport_error(e)
                ),
            ));
        }
        Err(_) => {
            return Some(error_stream(
                504,
                format!(
                    "the agent gateway accepted the request and sent no response headers within \
                     {}s",
                    HEADERS_WINDOW.as_secs()
                ),
            ));
        }
    };
    let status = upstream.status().as_u16();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_string();
    let hermes_session = upstream
        .headers()
        .get("x-hermes-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    Some(ChatStream {
        status,
        content_type,
        body: Box::pin(upstream.bytes_stream()),
        canned: None,
        hermes_session,
    })
}

/// What the session look-up or create answered — the two successes are kept
/// apart because they mean different things to a turn that expected a
/// transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionCreate {
    /// 2xx on a create: the container made the session just now.
    Fresh,
    /// The session is there: a look-up found it, or a create answered 409
    /// `session_exists` because an earlier turn made it — the steady state.
    Existing,
    /// The room is gone for good — the reason is for the log line. The caller
    /// sends the full history the other way, every turn from here on.
    Lost(&'static str),
    /// Anything else: not serving yet, or a container refusing the room.
    NotReady,
}

/// A transport error that proves no byte of the request reached a serving
/// process: connect refused, or the peer closing the connection before any
/// response (hyper's "connection closed before message completed" / "reset"
/// — the shape a container gives while it restarts). Anything else may have
/// been accepted.
fn failed_before_any_byte(e: &reqwest::Error) -> bool {
    if e.is_connect() {
        return true;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = source {
        let text = err.to_string().to_ascii_lowercase();
        if text.contains("connection reset")
            || text.contains("connection refused")
            || text.contains("connection closed before message completed")
        {
            return true;
        }
        source = err.source();
    }
    false
}

/// A failed request to an agent's API server, as a reader may see it: what
/// went wrong and why, never where.
///
/// reqwest's Display is "error sending request for url (http://agent-x:8642/
/// v1/…)" — the agent's dial URL (a container DNS name and port, or a host
/// loopback port: the fleet's topology) and no cause, so it was both leaky
/// and useless to the person reading it. This keeps reqwest's own words
/// without the URL and adds the innermost cause ("Connection refused (os
/// error 111)", "operation timed out"), which no layer of the chain prints
/// with an address. The whole error, URL and chain, goes to the server log.
/// Every agent-transport error that reaches a caller's text goes through
/// here.
pub(crate) fn transport_error(e: reqwest::Error) -> String {
    let mut chain = Vec::new();
    let mut source = std::error::Error::source(&e);
    while let Some(err) = source {
        chain.push(err.to_string());
        source = err.source();
    }
    tracing::warn!("[fleet] agent transport: {e}: {}", chain.join(": "));
    let url = e.url().map(|u| u.to_string());
    let host = e
        .url()
        .and_then(|u| Some(format!("{}:{}", u.host_str()?, u.port_or_known_default()?)));
    let what = e.without_url().to_string();
    let text = match chain.last() {
        Some(cause) if *cause != what => format!("{what}: {cause}"),
        _ => what,
    };
    // No layer below reqwest prints an address today; this keeps it so if
    // one ever does.
    [url, host]
        .into_iter()
        .flatten()
        .fold(text, |t, leak| t.replace(&leak, "the agent"))
}

/// A non-200 reply synthesized on this side of the wire, for the two
/// failures that happen after the request was sent. Not canned: a canned
/// stream is a 200 the harness unwraps, and these must read as the error
/// status they are.
fn error_stream(status: u16, text: String) -> ChatStream {
    ChatStream {
        status,
        content_type: "text/plain".into(),
        body: futures_util::stream::once(
            async move { Ok::<Bytes, reqwest::Error>(Bytes::from(text)) },
        )
        .boxed(),
        canned: None,
        hermes_session: None,
    }
}

/// Make sure the Hermes session this turn lands on is there.
///
/// A session that has carried turns before (`history_expected`) is LOOKED UP
/// (`GET /api/sessions/{id}`), never created: Hermes answering
/// `session_not_found` means the room is gone, and creating an empty one here
/// would be found by the next turn, which would then send only its new message
/// into a session that knows nothing. Every other session is CREATED,
/// idempotently: a success is a fresh session, and 409 `session_exists` is the
/// one an earlier turn made. A 409 `session_deleted` is a room that is gone for
/// good — v0.22 fences a deleted id so it is never reused — and treating it as
/// "not ready" held every turn for two minutes and then answered with the
/// "restarting" sentence while the agent was up. Anything else (including a
/// transport error) reads as "this container is not ready", which is the same
/// answer the stream itself gives while a container boots.
///
/// The session carries no `system_prompt` of its own on purpose: every turn
/// sends its own ephemeral one, so there is no stored prompt here to go stale
/// against the workspace it was rendered from. The headers window is the same
/// one the stream uses — a call that answers at all answers in milliseconds,
/// and the bound exists for the container that accepts TCP and then says
/// nothing.
async fn ensure_session(
    entry: &ManifestEntry,
    conversation_id: &str,
    history_expected: bool,
) -> SessionCreate {
    let (verb, request) = if history_expected {
        (
            "GET /api/sessions/{id}",
            agent_get(entry, &format!("/api/sessions/{conversation_id}")),
        )
    } else {
        (
            "POST /api/sessions",
            agent_post(entry, "/api/sessions").json(&json!({ "id": conversation_id })),
        )
    };
    let Ok(Ok(res)) = tokio::time::timeout(HEADERS_WINDOW, request.send()).await else {
        return SessionCreate::NotReady;
    };
    let status = res.status().as_u16();
    let code = if (200..300).contains(&status) {
        None
    } else {
        let body: Value = res.json().await.unwrap_or(Value::Null);
        body["error"]["code"].as_str().map(str::to_string)
    };
    let answer = if history_expected {
        session_lookup_answer(status, code.as_deref())
    } else {
        session_create_answer(status, code.as_deref())
    };
    // A Hermes without the session API at all: its 404 carries no session
    // code. The hold treats it as a container still booting, which is the one
    // thing it is not — so the log names it, because nothing else in a
    // two-minute hold will.
    if answer == SessionCreate::NotReady && status == 404 {
        eprintln!(
            "[fleet] {}: {verb} answered 404 — this Hermes has no session API; the agent cannot be pinned",
            entry.url
        );
    }
    answer
}

/// What a session LOOK-UP answered, as the turn reads it. Only a 2xx is the
/// room, and only Hermes' own `session_not_found` says it is gone — a bare 404
/// is a Hermes with no session API, which is "not ready" like everything else.
fn session_lookup_answer(status: u16, code: Option<&str>) -> SessionCreate {
    match (status, code) {
        (200..=299, _) => SessionCreate::Existing,
        (404, Some("session_not_found")) => SessionCreate::Lost(
            "a transcript was expected and the session is not in the container (its state volume \
             was lost, or the session was deleted)",
        ),
        _ => SessionCreate::NotReady,
    }
}

/// What a session CREATE answered. A 409 is "exists" only when it says so,
/// and "deleted" only when it says that: any other conflict is a container
/// refusing the session, and the stream POST would then be talking to a room
/// that was never made.
fn session_create_answer(status: u16, code: Option<&str>) -> SessionCreate {
    match (status, code) {
        (200..=299, _) => SessionCreate::Fresh,
        (409, Some("session_exists")) => SessionCreate::Existing,
        (409, Some("session_deleted")) => {
            SessionCreate::Lost("the session was deleted, and Hermes never reuses a deleted id")
        }
        _ => SessionCreate::NotReady,
    }
}

/// The agent stayed down past the hold window: say so honestly, streamed as a
/// normal reply so history shows what happened, not a silent failure.
fn unavailable_reply(model: &str) -> String {
    format!(
        "{} is restarting (or down) and didn't come back within two minutes. Your message is saved; send it again in a moment.",
        describe_agent(model).label
    )
}

/// Offline fallback: mock mode, no agents rendered yet.
fn mock_reply(model: &str) -> String {
    format!(
        "Hi, this is {} (mock mode: no agents are rendered yet). Create and start an agent to chat for real.",
        describe_agent(model).label
    )
}

/// A canned SSE reply in OpenAI chunk format, one word per 35ms tick — the
/// cadence the canned stream types at. `canned` names which outage this is.
fn canned_chat_stream(text: &str, canned: &'static str) -> ChatStream {
    let words: Vec<String> = text.split(' ').map(|w| format!("{w} ")).collect();
    let body = futures_util::stream::iter(words)
        .then(|w| async move {
            tokio::time::sleep(Duration::from_millis(35)).await;
            Ok::<Bytes, reqwest::Error>(Bytes::from(format!(
                "data: {}\n\n",
                json!({ "choices": [{ "delta": { "content": w } }] })
            )))
        })
        .chain(futures_util::stream::once(async {
            Ok(Bytes::from_static(b"data: [DONE]\n\n"))
        }))
        .boxed();
    ChatStream {
        status: 200,
        content_type: "text/event-stream".into(),
        body,
        canned: Some(canned),
        hermes_session: None,
    }
}

// ── Run control ──────────────────────────────────────────────────────────────

/// What a steer did. A steer is a message pushed INTO a run that is already
/// going, so the three answers are the three things a caller has to tell a
/// person apart: the words landed, the run was past taking them, or the
/// attempt never got there.
#[derive(Debug, Clone, PartialEq)]
pub enum SteerOutcome {
    Accepted,
    NotAccepting,
    Failed(String),
}

/// Steer a live run: Hermes hands the text to the agent mid-turn, so what it
/// does next accounts for it.
///
/// NO HOLD LOOP, unlike a turn. A steer is now or never — it names a run that
/// is running at this instant, and a container we had to wait two minutes for
/// is a container whose run ended while we waited. That is also why a 409 is
/// an ordinary answer rather than a failure: `run_not_accepting_steer` when
/// the run already finished, `steer_not_accepted` when the agent declined it,
/// and either way the run is simply past steering. The headers window is the
/// same bound the session create uses — an answer at all arrives in
/// milliseconds, and the ceiling is there for a container that accepts TCP
/// and then says nothing.
///
/// The text is sealed through a per-call vault, the same one a turn's message
/// goes through: a steer is a person typing into a running agent, which is
/// exactly the pasted-credential moment.
pub async fn steer_run(agent_model: &str, run_id: &str, text: &str) -> SteerOutcome {
    let Some(entry) = agent_entry(agent_model).await else {
        return SteerOutcome::Failed("no such agent".into());
    };
    let mut vault = SecretVault::default();
    let body = steer_body(text, &mut vault);
    report_sealed(&vault, agent_model);

    let sent = tokio::time::timeout(
        CONTROL_WINDOW,
        agent_post(&entry, &format!("/v1/runs/{run_id}/steer"))
            .json(&body)
            .send(),
    )
    .await;
    match sent {
        Ok(Ok(res)) if res.status().is_success() => SteerOutcome::Accepted,
        Ok(Ok(res)) if res.status() == reqwest::StatusCode::CONFLICT => SteerOutcome::NotAccepting,
        Ok(Ok(res)) => SteerOutcome::Failed(control_failure(res).await),
        Ok(Err(e)) => SteerOutcome::Failed(transport_error(e)),
        Err(_) => SteerOutcome::Failed("the agent did not answer the steer".into()),
    }
}

/// The steer's request body, sealed. `input` is the key Hermes reads (it takes
/// `message` and `text` too; one key is one thing to get wrong).
fn steer_body(text: &str, vault: &mut SecretVault) -> Value {
    json!({ "input": seal_content(&json!(text), vault) })
}

/// Stop a live run. Hermes answers `stopping` and then hard-interrupts the
/// agent, so this `Ok` means THE INTERRUPT WAS ACCEPTED, not that the turn is
/// over: the end of the turn arrives on the session stream — an
/// `assistant.completed` carrying whatever the run managed to say, then
/// `run.cancelled` — and the drain loop still owns that ending. Same door,
/// same bearer and same window as the steer above.
pub async fn stop_run(agent_model: &str, run_id: &str) -> Result<(), String> {
    let Some(entry) = agent_entry(agent_model).await else {
        return Err("no such agent".into());
    };
    let sent = tokio::time::timeout(
        CONTROL_WINDOW,
        agent_post(&entry, &format!("/v1/runs/{run_id}/stop")).send(),
    )
    .await;
    match sent {
        Ok(Ok(res)) if res.status().is_success() => Ok(()),
        Ok(Ok(res)) => Err(control_failure(res).await),
        Ok(Err(e)) => Err(transport_error(e)),
        Err(_) => Err("the agent did not answer the stop".into()),
    }
}

/// This agent's manifest row, looked up once. The control calls do not hold
/// for a container coming up the way a turn does, so a missing row is the
/// answer here rather than something to wait out.
async fn agent_entry(model: &str) -> Option<ManifestEntry> {
    read_manifest().await.into_iter().find(|m| m.model == model)
}

/// What an agent's API server answered: the status as-is, and the body as
/// json (`Null` when it was empty or not json).
#[derive(Debug, Clone)]
pub struct AgentAnswer {
    pub status: u16,
    pub body: Value,
}

/// One json request to an agent's own API server, through the same door and
/// with the same bearer as a turn — for the control-plane surfaces Hermes
/// serves there (its `/api/jobs` cron REST is the first). `window` bounds the
/// whole exchange, body included: the caller knows how long its call may
/// legitimately take.
///
/// `Err` means NO ANSWER — no manifest row, a refused or reset connection, or
/// silence past the window. Any status is an answer and comes back `Ok`; what
/// a refusal means is the caller's to read, so this never interprets it.
pub async fn agent_request(
    agent_model: &str,
    method: Method,
    path: &str,
    body: Option<&Value>,
    window: Duration,
) -> Result<AgentAnswer, String> {
    let Some(entry) = agent_entry(agent_model).await else {
        return Err("not in the fleet manifest".into());
    };
    let mut req = with_bearer(
        &entry,
        http().request(method, format!("{}{path}", entry.url)),
    );
    if let Some(body) = body {
        req = req.json(body);
    }
    let exchange = async {
        let res = req.send().await?;
        let status = res.status().as_u16();
        let bytes = res.bytes().await?;
        Ok::<_, reqwest::Error>((status, bytes))
    };
    match tokio::time::timeout(window, exchange).await {
        Ok(Ok((status, bytes))) => Ok(AgentAnswer {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }),
        Ok(Err(e)) => Err(transport_error(e)),
        Err(_) => Err(format!("no answer within {}s", window.as_secs())),
    }
}

/// What Hermes is holding for one pinned conversation: how much of the
/// transcript it has compacted away, and how far back what is left reaches.
///
/// `compacted` counts the rows Hermes projects as `display_kind: "hidden"` —
/// a compaction carrier (and the odd diagnostic notice) keeps its id, role
/// and timestamp but answers with empty content, because THE SUMMARY ITSELF
/// NEVER CROSSES THE WIRE. So this is a count and a boundary, never a
/// rendering of what the agent remembers.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionOverview {
    /// The Hermes session id — for a pinned agent, the conversation's own id.
    pub session_id: String,
    /// Hidden rows in the session: compaction carriers plus notices.
    pub compacted: u32,
    /// The first surviving (non-hidden) row's timestamp, verbatim as Hermes
    /// answered it. None when nothing survives, or when the row carries a
    /// timestamp that is not a string — this is a label, not arithmetic.
    pub since: Option<String>,
    /// Hermes' own `message_count` for the session, when it answered the
    /// session read. The rows are the overview; the count is a nicety, and a
    /// session read that fails leaves it absent rather than failing the call.
    /// The 500-row page was not the whole session: `compacted` is a floor.
    pub partial: bool,
    pub message_count: Option<i64>,
}

/// One pinned conversation's compaction state, asked of the agent's own
/// container. Two reads on the CONTROL window, never the turn's: a drawer is
/// open and a person is waiting, so a container that accepted TCP and went
/// quiet has to lose rather than hold the surface for five minutes.
///
/// `None` is every failure — no manifest row, a 404 (Hermes has no session
/// under this id), a timeout, a body that is not the documented shape. The
/// caller's answer for all of them is the same ("this does not apply here"),
/// and the reason lands in `tracing::debug` where a developer can read it.
pub async fn session_overview(agent_model: &str, conversation_id: &str) -> Option<SessionOverview> {
    let entry = agent_entry(agent_model).await?;
    // Both reads at once, so the surface waits one control window and not
    // two; the rows are load-bearing, the session read only fills the count.
    let rows_path = format!("/api/sessions/{conversation_id}/messages?limit=500&order=oldest");
    let session_path = format!("/api/sessions/{conversation_id}");
    let (body, session) = tokio::join!(
        control_get_json(&entry, &rows_path, agent_model),
        control_get_json(&entry, &session_path, agent_model),
    );
    let body = body?;
    // The id Hermes ANSWERED FOR, not the one asked about: a resume lineage
    // can resolve one id to another, and numbers from some other session
    // must not be hung on this thread.
    let answered = body
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or(conversation_id);
    if answered != conversation_id {
        tracing::debug!(
            "[fleet_chat] {agent_model}: session {conversation_id} resolved to {answered}; not this thread's"
        );
        return None;
    }
    let Some((compacted, since, partial)) = parse_session_rows(&body) else {
        tracing::debug!(
            "[fleet_chat] {agent_model}: session {conversation_id} messages read carried no data array"
        );
        return None;
    };
    let message_count = session.as_ref().and_then(parse_message_count);
    Some(SessionOverview {
        session_id: answered.to_string(),
        compacted,
        since,
        partial,
        message_count,
    })
}

/// A read through the agent's own door, parsed as json — bounded by the
/// control window. Only the STATUS of a refusal is logged: an agent's own
/// words can be in that body, and this is a session's transcript.
async fn control_get_json(entry: &ManifestEntry, path: &str, model: &str) -> Option<Value> {
    match tokio::time::timeout(CONTROL_WINDOW, agent_get(entry, path).send()).await {
        Ok(Ok(res)) if res.status().is_success() => match res.json::<Value>().await {
            Ok(body) => Some(body),
            Err(_) => {
                tracing::debug!("[fleet_chat] {model}: {path} answered a body that is not json");
                None
            }
        },
        Ok(Ok(res)) => {
            tracing::debug!("[fleet_chat] {model}: {path} answered {}", res.status());
            None
        }
        Ok(Err(e)) => {
            tracing::debug!("[fleet_chat] {model}: {path} did not answer: {e}");
            None
        }
        Err(_) => {
            tracing::debug!("[fleet_chat] {model}: {path} went quiet past the control window");
            None
        }
    }
}

/// The messages page, reduced to (hidden count, first surviving timestamp).
/// `None` when the body is not a page of rows at all — a 200 that does not
/// carry `data` is a different API, not an empty session.
/// (hidden rows, the first survivor's timestamp, whether the page was the
/// whole session). One page of 500 oldest-first: `since` is exact either way;
/// a count read off a partial page is a FLOOR, and the caller says so.
fn parse_session_rows(body: &Value) -> Option<(u32, Option<String>, bool)> {
    let rows = body.get("data")?.as_array()?;
    let hidden = |row: &Value| row.get("display_kind").and_then(Value::as_str) == Some("hidden");
    let compacted = rows.iter().filter(|row| hidden(row)).count() as u32;
    let since = rows
        .iter()
        .find(|row| !hidden(row))
        .and_then(|row| row.get("timestamp"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let partial = body
        .get("pagination")
        .and_then(|p| p.get("has_more"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some((compacted, since, partial))
}

/// Hermes' `message_count` off the session read, when it is a number.
fn parse_message_count(body: &Value) -> Option<i64> {
    body.get("message_count").and_then(Value::as_i64)
}

/// A refused control call as a reason a caller can show: the status, plus the
/// error code when the body names one (`run_not_found`, `invalid_steer_input`
/// — a closed vocabulary Hermes owns). THE BODY'S PROSE NEVER TRAVELS: an
/// agent's own words can be in there, and this string goes back into a chat.
pub(crate) async fn control_failure(res: reqwest::Response) -> String {
    let status = res.status().as_u16();
    match res
        .json::<Value>()
        .await
        .ok()
        .and_then(|body| body["error"]["code"].as_str().map(str::to_string))
        .filter(|code| !code.is_empty())
    {
        Some(code) => format!("{status} {code}"),
        None => status.to_string(),
    }
}

// ── The stream parser ────────────────────────────────────────────────────────

/// One parsed agent-stream event. The comms loop consumes content (the reply),
/// usage (the ledger), and tool names (the guard's backing-tool record);
/// reasoning feeds the chat plane that renders it.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStreamEvent {
    Content {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Tool {
        id: Option<String>,
        name: String,
        label: String,
        status: Option<String>,
    },
    Usage {
        prompt_tokens: i64,
        completion_tokens: i64,
    },
    Error {
        message: String,
    },
    /// The session arm's run handle, from `run.started` — what a steer or a
    /// stop has to name. The only event here that is not a piece of the reply,
    /// and internal: nothing persists it.
    RunStarted {
        run_id: String,
    },
    /// The stream saying this run was INTERRUPTED — a person pressed stop, or
    /// Talaria did on their behalf. Internal, and not an error: nothing
    /// failed, and the content that landed before it is the turn's real
    /// reply, so a consumer ends the turn on this rather than replacing what
    /// was said with a reason.
    Stopped,
}

/// The shape persisted into `messages.tools` and rendered beside the reply.
/// `id` is absent (not null) when the frame carried none —
/// `skip_serializing_if` drops the None.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub label: String,
    pub status: String, // 'running' | 'completed' | 'error'
}

/// Fold a tool event into a running list (dedupe by id when the frame names
/// one, else by name among the still-running). A later frame never overwrites
/// a good label with an empty one.
///
/// A FINISHED ROW STAYS FINISHED — `completed` and `error` alike, and this is
/// ONE RULE rather than two words with two fates. Hermes addresses tool
/// chatter to the open call and the open call's ordinal does not advance when
/// it ends, so a `tool.progress` frame arriving after the end would walk the
/// row back to "running" and leave it spinning in the chat and in history
/// forever. The label may still land — the reason can arrive after the
/// verdict — but the status does not move.
///
/// IT IS THE CLIENT'S RULE, VERBATIM: `mergeTool` in `ui/src/lib/sse-parse.ts`
/// folds the same frames for the row a reader is watching, so a difference
/// here is a row that says one thing live and another on reload. (The no-id
/// branch needs no help: it only ever matches a row still `running`, so a
/// later same-named call is its own row.)
pub fn merge_tool(
    tools: &[ToolCall],
    id: Option<&str>,
    name: &str,
    label: &str,
    status: Option<&str>,
) -> Vec<ToolCall> {
    let mut copy = tools.to_vec();
    let idx = match id {
        Some(id) => copy.iter().position(|t| t.id.as_deref() == Some(id)),
        None => copy
            .iter()
            .position(|t| t.name == name && t.status == "running"),
    };
    match idx {
        Some(i) => {
            let existing = &copy[i];
            copy[i] = ToolCall {
                id: existing.id.clone(),
                name: existing.name.clone(),
                label: if label.is_empty() {
                    existing.label.clone()
                } else {
                    label.to_string()
                },
                status: match (status, existing.status.as_str()) {
                    // Terminal: the row has ended, whichever way it ended.
                    (_, prior @ ("error" | "completed")) => prior.to_string(),
                    (Some(s), _) => s.to_string(),
                    (None, prior) => prior.to_string(),
                },
            };
        }
        None => copy.push(ToolCall {
            id: id.map(str::to_string),
            name: name.to_string(),
            label: label.to_string(),
            status: status.unwrap_or("running").to_string(),
        }),
    }
    copy
}

/// Push parser over the raw SSE body: feed it bytes as they arrive, take the
/// events each chunk completes. Frames split on a blank line; `event:` names
/// the frame, joined `data:` lines are its payload — the reader discipline
/// (`finish`) flushes a trailing frame with no blank line after it, which real
/// gateways emit.
#[derive(Default)]
pub struct AgentStreamParser {
    buffer: String,
    /// How many times each (message, tool) has STARTED on this stream. Hermes
    /// names no call id on a session's tool frames, so the id is synthesised
    /// — and a tool the agent runs three times in one reply is three calls,
    /// three rows, not one row rewritten three times: `tool.started` bumps
    /// the count and every progress or completion frame for that tool reads
    /// the current one, which is the call still in flight.
    calls: HashMap<String, u32>,
    /// Whether any `assistant.delta` carried text on THIS stream. The session
    /// arm's final frame repeats the whole reply, so one bit of history is
    /// what tells a turn that already streamed its words (do not land them
    /// twice) from one that never streamed at all (land them once).
    streamed: bool,
}

impl AgentStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<AgentStreamEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(sep) = self.buffer.find("\n\n") {
            let frame: String = self.buffer.drain(..sep + 2).collect();
            events.extend(parse_frame(
                &frame[..frame.len() - 2],
                &mut self.streamed,
                &mut self.calls,
            ));
        }
        events
    }

    /// End of body: parse whatever the final frame left behind.
    pub fn finish(&mut self) -> Vec<AgentStreamEvent> {
        let rest = std::mem::take(&mut self.buffer);
        parse_frame(&rest, &mut self.streamed, &mut self.calls)
    }
}

fn parse_frame(
    frame: &str,
    streamed: &mut bool,
    calls: &mut HashMap<String, u32>,
) -> Vec<AgentStreamEvent> {
    let mut event_name = String::new();
    let mut data_lines: Vec<&str> = Vec::new();
    for line in frame.split('\n') {
        let t = line.trim();
        if let Some(name) = t.strip_prefix("event:") {
            event_name = name.trim().to_string();
        } else if let Some(data) = t.strip_prefix("data:") {
            data_lines.push(data.trim());
        }
    }
    let data = data_lines.join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Vec::new();
    }
    if event_name == "hermes.tool.progress" || event_name == "claude.tool.progress" {
        return parse_tool_progress(&data).into_iter().collect();
    }
    let Ok(json) = serde_json::from_str::<Value>(&data) else {
        return Vec::new(); // keep-alive / partial frame
    };
    // THE SESSION STREAM NAMES ITS FRAMES; /v1/chat/completions does not. A
    // name that arm owns is answered there and returns, so the chunk shape
    // below still reads exactly the frames it always did.
    //
    // The one name both could use is `error`: the session arm takes it only
    // when the frame carries a session's own stamp, so a completions stream
    // that names its failure frame still reads through `error_message_of`.
    let session_stamped = json.get("run_id").is_some() || json.get("session_id").is_some();
    if (event_name != "error" || session_stamped)
        && let Some(events) = parse_session_frame(&event_name, &json, streamed, calls)
    {
        return events;
    }
    let mut events = Vec::new();
    // The failure frame first: it rides a 200 with no content delta, and the
    // turn is over the moment it lands — later arms may still pull usage, but
    // nothing downstream should read this frame as a healthy empty one.
    if let Some(message) = error_message_of(&json) {
        events.push(AgentStreamEvent::Error { message });
    }
    let delta = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"));
    if let Some(delta) = delta {
        let content = delta.get("content").and_then(Value::as_str);
        let reasoning = delta
            .get("reasoning")
            .or_else(|| delta.get("reasoning_content"))
            .and_then(Value::as_str);
        if let Some(text) = content.filter(|t| !t.is_empty()) {
            events.push(AgentStreamEvent::Content { text: text.into() });
        } else if let Some(text) = reasoning.filter(|t| !t.is_empty()) {
            events.push(AgentStreamEvent::Reasoning { text: text.into() });
        }
    }
    // Final chunk carries usage when stream_options.include_usage is honoured.
    let usage = json.get("usage");
    let pt = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let ct = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if pt > 0 || ct > 0 {
        events.push(AgentStreamEvent::Usage {
            prompt_tokens: pt,
            completion_tokens: ct,
        });
    }
    events
}

/// The reason a failure frame carries, if it is one: `finish_reason: "error"`
/// on the choice, a top-level `error: {message}` object, or Hermes's own
/// `hermes: {failed: true, error}` telemetry — any one of them names the
/// failure, preference order error.message → hermes.error → a plain sentence.
fn error_message_of(json: &Value) -> Option<String> {
    let finish = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str);
    let failed = finish == Some("error")
        || json.get("error").is_some()
        || json
            .get("hermes")
            .and_then(|h| h.get("failed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if !failed {
        return None;
    }
    let message = json
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            json.get("hermes")
                .and_then(|h| h.get("error"))
                .and_then(Value::as_str)
        })
        .unwrap_or("the agent reported an error")
        .to_string();
    Some(message)
}

/// hermes.tool.progress frames — the agent's own tool telemetry riding the
/// same SSE body. `label` may be empty (a later "completed" frame carries
/// none); display falls back to `name`.
fn parse_tool_progress(payload: &str) -> Option<AgentStreamEvent> {
    let r: Value = serde_json::from_str(payload).ok()?;
    let str_of = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let name = {
        let n = str_of("tool");
        if n.is_empty() { str_of("name") } else { n }
    };
    let name = if name.is_empty() { "tool".into() } else { name };
    let label = [str_of("emoji"), str_of("label")]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    let id = {
        let i = str_of("toolCallId");
        if i.is_empty() {
            str_of("tool_call_id")
        } else {
            i
        }
    };
    let id = (!id.is_empty()).then_some(id);
    // The status word, lowercased: anything unrecognized stays None (merge
    // keeps the prior value; a push says 'running'). A FAILURE READS AS
    // "error" HERE TOO — the completions arm's telemetry names it either way,
    // and mapping it to None left a failed call rendering as still running.
    let s = str_of("status").to_lowercase();
    let status = match s.as_str() {
        "running" => Some("running".to_string()),
        "completed" | "complete" => Some("completed".to_string()),
        "error" | "failed" => Some("error".to_string()),
        _ => None,
    };
    if label.is_empty() && id.is_none() {
        return None;
    }
    Some(AgentStreamEvent::Tool {
        id,
        name,
        label,
        status,
    })
}

/// The HERMES SESSION stream (`POST /api/sessions/{id}/chat/stream`), whose
/// frames are NAMED and whose payloads are flat, mapped onto the same events
/// the chat-completions arm produces — everything downstream (the drain loops,
/// the tool merge, the ledger) already knows those, and a session turn is not
/// a different kind of turn to them. `None` means "not a session frame": the
/// caller falls through to the OpenAI chunk shape.
fn parse_session_frame(
    name: &str,
    json: &Value,
    streamed: &mut bool,
    calls: &mut HashMap<String, u32>,
) -> Option<Vec<AgentStreamEvent>> {
    let str_of = |k: &str| json.get(k).and_then(Value::as_str).unwrap_or("");
    match name {
        "run.started" => {
            let run_id = str_of("run_id");
            if run_id.is_empty() {
                return Some(Vec::new());
            }
            Some(vec![AgentStreamEvent::RunStarted {
                run_id: run_id.to_string(),
            }])
        }
        // The assistant message's id lands here, and every event that needs it
        // carries it itself — so this frame is an acknowledgement, not news.
        "message.started" => Some(Vec::new()),
        "assistant.delta" => {
            let text = str_of("delta");
            if text.is_empty() {
                return Some(Vec::new());
            }
            // Set only when a delta actually carried words: a stream of empty
            // deltas has not landed the reply, and the final frame still has
            // to.
            *streamed = true;
            Some(vec![AgentStreamEvent::Content {
                text: text.to_string(),
            }])
        }
        "tool.progress" => {
            // "_thinking" is Hermes' reasoning channel wearing a tool's
            // clothes: the one progress frame whose delta is PROSE, not a
            // tool's own chatter.
            if str_of("tool_name") == "_thinking" {
                let text = str_of("delta");
                if text.is_empty() {
                    return Some(Vec::new());
                }
                return Some(vec![AgentStreamEvent::Reasoning {
                    text: text.to_string(),
                }]);
            }
            // Any other tool's progress is its own chatter, and the frame
            // carries it as `delta`, not `preview` — the label is the chatter,
            // clamped, so a long-running call shows what it is doing.
            let mut ev = session_tool(json, "running", calls, false);
            if let AgentStreamEvent::Tool { label, .. } = &mut ev {
                let delta = str_of("delta").trim();
                if !delta.is_empty() {
                    *label = delta.chars().take(200).collect();
                }
            }
            Some(vec![ev])
        }
        "tool.started" => Some(vec![session_tool(json, "running", calls, true)]),
        "tool.completed" => Some(vec![session_tool(json, "completed", calls, false)]),
        // A failure is its OWN terminal word. It used to land as "completed"
        // with the reason hidden in the preview, which read as a call that
        // worked; the row now says `error` everywhere it flows (the persisted
        // `messages.tools`, the client's face) and the preview still carries
        // Hermes' reason, because that sentence is the only account of WHY.
        "tool.failed" => Some(vec![session_tool(json, "error", calls, false)]),
        "assistant.commentary" => {
            let text = str_of("text");
            let already = json
                .get("already_streamed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if already || text.is_empty() {
                return Some(Vec::new());
            }
            Some(vec![AgentStreamEvent::Content {
                text: text.to_string(),
            }])
        }
        "assistant.completed" => {
            // The reply rides at the TOP of this frame as `content` (Hermes'
            // `final_response`); `message.content` is the older shape and
            // still read. A REPLY THAT NEVER STREAMED MUST STILL LAND: a
            // turn that answered in one go has its whole reply only here,
            // and a turn that streamed has it twice — the second copy is the
            // one to drop.
            let final_text = match json.get("content") {
                Some(Value::String(text)) if !text.is_empty() => text.clone(),
                _ => content_text(json.get("message").and_then(|m| m.get("content"))),
            };
            let mut events = Vec::new();
            if !final_text.is_empty() && !*streamed {
                events.push(AgentStreamEvent::Content { text: final_text });
            }
            events.extend(session_usage(json));
            Some(events)
        }
        // The run's own verdict, after the reply: the counts ride here, and a
        // FAILED run ends the turn as an ERROR so the chain does not re-fire
        // an unanswered message forever. (A failed run's
        // `assistant.completed` carries Hermes' apology sentence as its
        // content; the error row's reason wins over it.)
        "run.completed" => Some(session_usage(json)),
        "run.failed" => Some(vec![AgentStreamEvent::Error {
            message: match session_error_message(json) {
                m if m == "the agent reported an error" => "the run failed".to_string(),
                m => m,
            },
        }]),
        // A CANCELLED run is the other thing entirely: somebody meant to stop
        // it. Reading that as an error cost the turn the words it had already
        // said — the reason overwrote them — so the interrupt says only that
        // it happened, and the partial reply stands as the reply.
        "run.cancelled" => Some(vec![AgentStreamEvent::Stopped]),
        // Hermes acknowledging a steer it took. The steered words themselves
        // come back as the run's own frames from here on, so the
        // acknowledgement is nothing to land.
        "run.steered" => Some(Vec::new()),
        "run.queued" => Some(Vec::new()),
        // Hermes asking whoever reads the stream to approve a flagged call.
        // The door answered it as the bytes went past (fleet_approvals.rs);
        // the consumer has nothing to land.
        "approval.request" => Some(Vec::new()),
        "error" => Some(vec![AgentStreamEvent::Error {
            message: session_error_message(json),
        }]),
        // End of stream: the drain loop ends on the body, not on a frame.
        "done" => Some(Vec::new()),
        _ => None,
    }
}

/// Counts only when the frame carries them. Every consumer decides "real
/// counts or a character estimate" on whether a usage event arrived at all,
/// so a frame without counts must stay silent here — a reported 0/0 would
/// be billed as the truth.
fn session_usage(json: &Value) -> Vec<AgentStreamEvent> {
    let usage = json.get("usage");
    let count = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_i64);
    match (count("prompt_tokens"), count("completion_tokens")) {
        (Some(prompt_tokens), Some(completion_tokens)) => vec![AgentStreamEvent::Usage {
            prompt_tokens,
            completion_tokens,
        }],
        _ => Vec::new(),
    }
}

/// One session tool frame as the shape `merge_tool` dedupes on. Hermes names
/// no tool-call id on these frames, so the id is synthesised from the message
/// and the tool — stable across a started/completed pair, which is exactly
/// what folds them into one row instead of two.
fn session_tool(
    json: &Value,
    status: &str,
    calls: &mut HashMap<String, u32>,
    started: bool,
) -> AgentStreamEvent {
    let str_of = |k: &str| json.get(k).and_then(Value::as_str).unwrap_or("");
    let name = match str_of("tool_name") {
        "" => "tool",
        n => n,
    };
    let id = [str_of("tool_call_id"), str_of("id")]
        .into_iter()
        .find(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let message = str_of("message_id");
            if message.is_empty() {
                return None;
            }
            // `#n` is the call's ordinal within its message: a started frame
            // opens the next one, everything else addresses the one open.
            let key = format!("{message}:{name}");
            let n = calls.entry(key.clone()).or_insert(0);
            if started || *n == 0 {
                *n += 1;
            }
            Some(format!("{key}#{n}"))
        });
    AgentStreamEvent::Tool {
        id,
        name: name.to_string(),
        // `preview` is Hermes' display-redacted line for the call; a progress
        // or completion frame may carry none, and the merge keeps the good one
        // the started frame gave.
        label: str_of("preview").to_string(),
        status: Some(status.to_string()),
    }
}

/// The final assistant content, whatever shape it arrived in: the plain
/// string, or the OpenAI parts array a multimodal reply carries — the text
/// parts joined, because an image part has no words to land.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The sentence an `error` frame carries. The shape has moved between Hermes
/// versions — a flat `message`, an `error` object, a bare `error` string — so
/// all of them are read, and a frame that names none still reports a failure
/// rather than passing for a healthy end of turn.
fn session_error_message(json: &Value) -> String {
    json.get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .or_else(|| json.get("message").and_then(Value::as_str))
        .or_else(|| json.get("error").and_then(Value::as_str))
        .unwrap_or("the agent reported an error")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_metered_under_the_session_we_made_or_the_id_hermes_answered() {
        let stream = |hermes_session: Option<&str>| ChatStream {
            status: 200,
            content_type: "text/event-stream".into(),
            body: Box::pin(futures_util::stream::empty()),
            canned: None,
            hermes_session: hermes_session.map(str::to_string),
        };
        let session = AgentRequest::SessionTurn {
            conversation_id: "4f7c2a1e-9d3b-4c8a-b1f2-0e6d5a4c3b2a".into(),
            body: json!({}),
            history_expected: false,
        };
        // The session arm's key is ours, whatever the container answered.
        assert_eq!(
            turn_key(&session, &stream(Some("api-0123456789abcdef"))).as_deref(),
            Some("4f7c2a1e-9d3b-4c8a-b1f2-0e6d5a4c3b2a")
        );
        assert_eq!(
            turn_key(&session, &stream(None)).as_deref(),
            Some("4f7c2a1e-9d3b-4c8a-b1f2-0e6d5a4c3b2a")
        );
        // The completions arm's key is the one Hermes named for it.
        let completions = AgentRequest::ChatCompletions { payload: json!({}) };
        assert_eq!(
            turn_key(&completions, &stream(Some("api-0123456789abcdef"))).as_deref(),
            Some("api-0123456789abcdef")
        );
        // No header (an older Hermes, a synthesized error): no key, and the
        // turn's calls stay unattended rather than claiming someone else's.
        assert_eq!(turn_key(&completions, &stream(None)), None);
        assert_eq!(turn_key(&completions, &stream(Some("bad key"))), None);
    }

    #[test]
    fn parser_handles_frames_split_across_chunks() {
        let mut p = AgentStreamParser::new();
        assert!(
            p.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel")
                .is_empty()
        );
        let events = p.feed(b"lo\"}}]}\n\ndata: [DONE]\n\n");
        assert_eq!(
            events,
            vec![AgentStreamEvent::Content {
                text: "Hello".into()
            }]
        );
        assert!(p.finish().is_empty());
    }

    #[test]
    fn parser_yields_usage_and_reasoning() {
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            b"data: {\"choices\":[{\"delta\":{\"reasoning\":\"thinking\"}}]}\n\n\
              data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
        );
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::Reasoning {
                    text: "thinking".into()
                },
                AgentStreamEvent::Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5
                },
            ]
        );
    }

    #[test]
    fn failure_frame_is_an_error_event_not_an_empty_complete() {
        // The exact shape the agent gateway emits when the provider call dies
        // inside the container (observed live: openrouter 401ing on a stale
        // container key): a 200-SSE body whose final chunk carries
        // finish_reason "error", a top-level error object, and the hermes
        // telemetry block. This frame must surface as an Error event — reading
        // it as an empty complete turn is what made the chat chain re-fire one
        // of these every ~550ms for hours.
        let frame = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"error\"}],",
            "\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":0,\"total_tokens\":0},",
            "\"error\":{\"message\":\"HTTP 401: invalid API key\",\"type\":\"agent_error\"},",
            "\"hermes\":{\"completed\":false,\"partial\":false,\"failed\":true,",
            "\"error\":\"HTTP 401: invalid API key\",\"error_code\":\"agent_error\"}}\n\n"
        );
        let mut p = AgentStreamParser::new();
        let events = p.feed(frame.as_bytes());
        assert_eq!(
            events,
            vec![AgentStreamEvent::Error {
                message: "HTTP 401: invalid API key".into()
            }]
        );
        // [DONE] after it stays a non-event.
        assert!(p.feed(b"data: [DONE]\n\n").is_empty());
    }

    #[test]
    fn parser_reads_tool_progress_frames() {
        let mut p = AgentStreamParser::new();
        // One frame: the `event:` line rides WITH its `data:` line — a blank
        // line between them would be two frames in every SSE reader (the
        // event-only one carries no data, the data-only one no name).
        let events = p.feed(
            "event: hermes.tool.progress\ndata: {\"tool\":\"memory\",\"emoji\":\"🧠\"}\n\n"
                .as_bytes(),
        );
        assert_eq!(
            events,
            vec![AgentStreamEvent::Tool {
                id: None,
                name: "memory".into(),
                label: "🧠".into(),
                status: None,
            }]
        );
    }

    #[test]
    fn tool_progress_carries_the_status_word() {
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            "event: hermes.tool.progress\ndata: {\"tool\":\"search\",\"toolCallId\":\"c1\",\"status\":\"running\"}\n\n"
                .as_bytes(),
        );
        assert_eq!(
            events,
            vec![AgentStreamEvent::Tool {
                id: Some("c1".into()),
                name: "search".into(),
                label: String::new(),
                status: Some("running".into()),
            }]
        );
    }

    /// The completions arm's own telemetry names a failure, and it has to read
    /// as the error word — not as None, which left the row running forever.
    #[test]
    fn tool_progress_reads_a_failure_as_the_error_status() {
        for word in ["error", "failed", "ERROR"] {
            let mut p = AgentStreamParser::new();
            let events = p.feed(
                format!(
                    "event: hermes.tool.progress\ndata: {{\"tool\":\"search\",\"toolCallId\":\"c1\",\"label\":\"quota exhausted\",\"status\":\"{word}\"}}\n\n"
                )
                .as_bytes(),
            );
            assert_eq!(
                events,
                vec![AgentStreamEvent::Tool {
                    id: Some("c1".into()),
                    name: "search".into(),
                    label: "quota exhausted".into(),
                    status: Some("error".into()),
                }],
                "{word}"
            );
        }
    }

    /// A `tool.failed` session frame is an ERRORED row whose label is Hermes'
    /// reason — the only account of why the call did not work.
    #[test]
    fn a_failed_session_tool_frame_lands_as_an_error_with_its_reason() {
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            concat!(
                "event: tool.started\n",
                r#"data: {"message_id":"m1","tool_name":"memory","preview":"🧠 remember"}"#,
                "\n\n",
                "event: tool.failed\n",
                r#"data: {"message_id":"m1","tool_name":"memory","preview":"memory: write refused"}"#,
                "\n\n",
            )
            .as_bytes(),
        );
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::Tool {
                    id: Some("m1:memory#1".into()),
                    name: "memory".into(),
                    label: "🧠 remember".into(),
                    status: Some("running".into()),
                },
                AgentStreamEvent::Tool {
                    id: Some("m1:memory#1".into()),
                    name: "memory".into(),
                    label: "memory: write refused".into(),
                    status: Some("error".into()),
                },
            ]
        );
        // And the pair folds into ONE errored row carrying the reason.
        let mut tools: Vec<ToolCall> = Vec::new();
        for ev in events {
            if let AgentStreamEvent::Tool {
                id,
                name,
                label,
                status,
            } = ev
            {
                tools = merge_tool(&tools, id.as_deref(), &name, &label, status.as_deref());
            }
        }
        assert_eq!(
            tools,
            vec![ToolCall {
                id: Some("m1:memory#1".into()),
                name: "memory".into(),
                label: "memory: write refused".into(),
                status: "error".into(),
            }]
        );
    }

    /// Both endings are terminal: an ending overwrites a running row, and no
    /// later frame — a straggler progress for the same id, or a same-named
    /// call with no id — walks it back. The client's fold holds the same rule
    /// (`sse-parse.ts`), so the live row and the persisted one agree.
    #[test]
    fn merge_tool_keeps_error_terminal() {
        let tools = merge_tool(&[], Some("c1"), "memory", "🧠", Some("running"));
        let tools = merge_tool(&tools, Some("c1"), "memory", "write refused", Some("error"));
        assert_eq!(tools[0].status, "error");
        // A straggler addressed to the same call cannot reopen it.
        let tools = merge_tool(&tools, Some("c1"), "memory", "", Some("running"));
        assert_eq!(tools[0].status, "error");
        let tools = merge_tool(&tools, Some("c1"), "memory", "", Some("completed"));
        assert_eq!(tools[0].status, "error");
        // A good label still lands on an errored row (the reason may arrive
        // after the verdict); the row stays one row.
        let tools = merge_tool(&tools, Some("c1"), "memory", "quota exhausted", None);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].label, "quota exhausted");
        assert_eq!(tools[0].status, "error");
        // No id: an errored row is finished, so a later same-named call is its
        // OWN row — exactly how a completed row behaves.
        let anon = merge_tool(&[], None, "search", "🔎", Some("error"));
        let anon = merge_tool(&anon, None, "search", "🔎", Some("running"));
        assert_eq!(anon.len(), 2, "{anon:?}");
        assert_eq!(anon[0].status, "error");
        assert_eq!(anon[1].status, "running");
        // And it serialises as the one word the client reads.
        assert_eq!(
            serde_json::to_value(&tools).unwrap(),
            serde_json::json!([{
                "id": "c1",
                "name": "memory",
                "label": "quota exhausted",
                "status": "error"
            }])
        );

        // THE OTHER ENDING, same rule. A `tool.progress` frame addressed to
        // a call that has already completed — Hermes keeps the ordinal open
        // after the end — must not reopen the row: a reader watching the
        // turn would see a finished tool start spinning again, and the
        // persisted row would say "running" for ever.
        let done = merge_tool(&[], Some("c2"), "terminal", "🖥 ls", Some("running"));
        let done = merge_tool(&done, Some("c2"), "terminal", "", Some("completed"));
        assert_eq!(done[0].status, "completed");
        let done = merge_tool(
            &done,
            Some("c2"),
            "terminal",
            "still talking",
            Some("running"),
        );
        assert_eq!(done.len(), 1, "{done:?}");
        assert_eq!(done[0].status, "completed", "a finished row stays finished");
        assert_eq!(
            done[0].label, "still talking",
            "the label may still land — only the verdict is fixed"
        );
        // And a failure reported after a completion does not overwrite it
        // either: ONE rule, not a precedence table.
        let done = merge_tool(&done, Some("c2"), "terminal", "", Some("error"));
        assert_eq!(done[0].status, "completed");
    }

    #[test]
    fn merge_tool_dedupes_by_id_and_never_blanks_a_label() {
        let tools = merge_tool(&[], Some("c1"), "search", "🔎", Some("running"));
        // The completed frame carries no label — the good one survives.
        let tools = merge_tool(&tools, Some("c1"), "search", "", Some("completed"));
        assert_eq!(
            tools,
            vec![ToolCall {
                id: Some("c1".into()),
                name: "search".into(),
                label: "🔎".into(),
                status: "completed".into(),
            }]
        );
        // No id: a second running frame of the same name merges, a completed
        // one starts fresh only after the running entry flipped.
        let anon = merge_tool(&[], None, "memory", "", None);
        assert_eq!(anon[0].status, "running");
        let serde = serde_json::to_value(&anon).unwrap();
        assert_eq!(
            serde,
            serde_json::json!([{ "name": "memory", "label": "", "status": "running" }])
        );
    }

    #[test]
    fn finish_flushes_a_frame_without_a_trailing_blank_line() {
        let mut p = AgentStreamParser::new();
        p.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}");
        assert_eq!(
            p.finish(),
            vec![AgentStreamEvent::Content {
                text: "tail".into()
            }]
        );
    }

    // ── the session arm ─────────────────────────────────────────────────────
    //
    // One fixture per event name the Hermes session API emits (verified
    // against gateway/platforms/api_server.py in hermes-agent v0.21.4), fed in
    // the chunking a real body arrives in rather than one tidy frame per read.

    #[test]
    fn the_session_stream_maps_every_frame_name_onto_the_shared_events() {
        let mut p = AgentStreamParser::new();
        let mut events = Vec::new();

        // Two frames in one read: the run handle, then the message id.
        events.extend(
            p.feed(
                concat!(
                    "event: run.started\n",
                    r#"data: {"session_id":"c1","run_id":"run_7","seq":1,"ts":1,"user_message":{"role":"user"},"runtime":{}}"#,
                    "\n\n",
                    "event: message.started\n",
                    r#"data: {"session_id":"c1","run_id":"run_7","seq":2,"message":{"id":"msg_1","role":"assistant"}}"#,
                    "\n\n",
                )
                .as_bytes(),
            ),
        );

        // One frame split mid-JSON: nothing lands until the rest arrives.
        assert!(
            p.feed(b"event: assistant.delta\ndata: {\"message_id\":\"msg_1\",\"del")
                .is_empty()
        );
        events.extend(
            p.feed(
                concat!(
                    r#"ta":"Hello"}"#,
                    "\n\n",
                    "event: tool.progress\n",
                    r#"data: {"message_id":"msg_1","tool_name":"_thinking","delta":"weighing it"}"#,
                    "\n\n",
                )
                .as_bytes(),
            ),
        );

        // A tool's whole life: its own progress text, the call, the result,
        // and a second tool that failed.
        events.extend(
            p.feed(
                concat!(
                    "event: tool.started\n",
                    r#"data: {"message_id":"msg_1","tool_name":"terminal","preview":"🖥 ls -la","args":{}}"#,
                    "\n\n",
                    "event: tool.progress\n",
                    r#"data: {"message_id":"msg_1","tool_name":"terminal","preview":"ls -la","args":{}}"#,
                    "\n\n",
                    "event: tool.completed\n",
                    r#"data: {"message_id":"msg_1","tool_name":"terminal","preview":"","args":{}}"#,
                    "\n\n",
                    "event: tool.failed\n",
                    r#"data: {"message_id":"msg_1","tool_name":"memory","preview":"memory: write refused","args":{}}"#,
                    "\n\n",
                )
                .as_bytes(),
            ),
        );

        // Commentary that was already streamed is not content twice; the
        // final frame's content is the same reply the deltas carried, so it
        // is usage only.
        events.extend(
            p.feed(
                concat!(
                    "event: assistant.commentary\n",
                    r#"data: {"message_id":"msg_1","text":"Hello","already_streamed":true}"#,
                    "\n\n",
                    "event: assistant.commentary\n",
                    r#"data: {"message_id":"msg_1","text":"Checking the box first.","already_streamed":false}"#,
                    "\n\n",
                    "event: assistant.completed\n",
                    r#"data: {"message":{"role":"assistant","content":"Hello"},"usage":{"prompt_tokens":12,"completion_tokens":3},"runtime":{}}"#,
                    "\n\n",
                )
                .as_bytes(),
            ),
        );

        // The last frame arrives with no blank line after it.
        assert!(p.feed(b"event: done\ndata: {}").is_empty());
        events.extend(p.finish());

        assert_eq!(
            events,
            vec![
                AgentStreamEvent::RunStarted {
                    run_id: "run_7".into()
                },
                AgentStreamEvent::Content {
                    text: "Hello".into()
                },
                AgentStreamEvent::Reasoning {
                    text: "weighing it".into()
                },
                AgentStreamEvent::Tool {
                    id: Some("msg_1:terminal#1".into()),
                    name: "terminal".into(),
                    label: "🖥 ls -la".into(),
                    status: Some("running".into()),
                },
                AgentStreamEvent::Tool {
                    id: Some("msg_1:terminal#1".into()),
                    name: "terminal".into(),
                    label: "ls -la".into(),
                    status: Some("running".into()),
                },
                AgentStreamEvent::Tool {
                    id: Some("msg_1:terminal#1".into()),
                    name: "terminal".into(),
                    label: String::new(),
                    status: Some("completed".into()),
                },
                AgentStreamEvent::Tool {
                    id: Some("msg_1:memory#1".into()),
                    name: "memory".into(),
                    label: "memory: write refused".into(),
                    status: Some("error".into()),
                },
                AgentStreamEvent::Content {
                    text: "Checking the box first.".into()
                },
                AgentStreamEvent::Usage {
                    prompt_tokens: 12,
                    completion_tokens: 3
                },
            ]
        );
    }

    #[test]
    fn a_session_reply_that_never_streamed_still_lands_as_content() {
        // A turn that answers in one go emits no assistant.delta at all: the
        // final frame holds the whole reply, and dropping it would persist an
        // empty turn.
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            concat!(
                "event: assistant.completed\n",
                r#"data: {"message":{"role":"assistant","content":"Done — the box is checked."},"usage":{"prompt_tokens":7,"completion_tokens":2}}"#,
                "\n\n",
            )
            .as_bytes(),
        );
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::Content {
                    text: "Done — the box is checked.".into()
                },
                AgentStreamEvent::Usage {
                    prompt_tokens: 7,
                    completion_tokens: 2
                },
            ]
        );

        // The same reply as OpenAI content parts: the text parts are the
        // words it landed.
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            concat!(
                "event: assistant.completed\n",
                r#"data: {"message":{"role":"assistant","content":[{"type":"text","text":"Done."},{"type":"image_url","image_url":{"url":"x"}}]}}"#,
                "\n\n",
            )
            .as_bytes(),
        );
        // No counts on this frame, so no usage event: the ledger estimates.
        assert_eq!(
            events,
            vec![AgentStreamEvent::Content {
                text: "Done.".into()
            }]
        );
    }

    /// A completed frame without the two counts reports NO usage: the
    /// ledger reads "no usage event" as "estimate from characters", and a
    /// 0/0 would have been billed as the truth.
    #[test]
    fn session_usage_stays_silent_when_the_frame_omits_the_counts() {
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            concat!(
                "event: assistant.completed\n",
                r#"data: {"message":{"role":"assistant","content":""},"usage":{"total_tokens":4}}"#,
                "\n\n",
            )
            .as_bytes(),
        );
        assert_eq!(events, vec![]);
    }

    /// A completions stream that NAMES its failure frame still reads through
    /// the chunk arm's error rule: only a session-stamped `error` is the
    /// session's. A tool's own progress frame carries its chatter as `delta`.
    #[test]
    fn an_unstamped_error_frame_and_a_progress_delta_read_their_own_way() {
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            concat!(
                "event: error\n",
                r#"data: {"choices":[{"delta":{"content":"still fine"}}]}"#,
                "\n\n",
                "event: tool.progress\n",
                r#"data: {"run_id":"r1","message_id":"m1","tool_name":"terminal","delta":"  ls -la\n"}"#,
                "\n\n",
            )
            .as_bytes(),
        );
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::Content {
                    text: "still fine".into()
                },
                AgentStreamEvent::Tool {
                    id: Some("m1:terminal#1".into()),
                    name: "terminal".into(),
                    label: "ls -la".into(),
                    status: Some("running".into()),
                },
            ]
        );
    }

    /// The shapes Hermes v0.21.4 actually sends at the end of a run: the
    /// reply at the top of `assistant.completed` as `content`, the counts
    /// on `run.completed`, and a failed run's verdict as an error — which
    /// is what keeps a refused turn from landing as an empty reply the
    /// chain re-fires forever.
    #[test]
    fn the_run_verdict_frames_read_as_hermes_writes_them() {
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            concat!(
                "event: assistant.completed\n",
                r#"data: {"session_id":"c1","message_id":"m1","content":"OK","completed":true,"runtime":{}}"#,
                "\n\n",
                "event: run.completed\n",
                r#"data: {"session_id":"c1","message_id":"m1","messages":[],"usage":{"prompt_tokens":11,"completion_tokens":3},"runtime":{}}"#,
                "\n\n",
                "event: done\n",
                "data: {}\n\n",
            )
            .as_bytes(),
        );
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::Content { text: "OK".into() },
                AgentStreamEvent::Usage {
                    prompt_tokens: 11,
                    completion_tokens: 3
                },
            ]
        );

        let mut p = AgentStreamParser::new();
        let events = p.feed(
            concat!(
                "event: assistant.completed\n",
                r#"data: {"session_id":"c1","message_id":"m1","content":"Custom endpoint rejected this request.","completed":false}"#,
                "\n\n",
                "event: run.failed\n",
                r#"data: {"session_id":"c1","message_id":"m1","completed":false,"messages":[],"usage":{}}"#,
                "\n\n",
            )
            .as_bytes(),
        );
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::Content {
                    text: "Custom endpoint rejected this request.".into()
                },
                AgentStreamEvent::Error {
                    message: "the run failed".into()
                },
            ]
        );

        let mut p = AgentStreamParser::new();
        let events = p.feed("event: run.cancelled\ndata: {\"run_id\":\"r1\"}\n\n".as_bytes());
        assert_eq!(events, vec![AgentStreamEvent::Stopped]);
    }

    /// An approval request is the door's to answer (fleet_approvals.rs), on
    /// either stream: the consumer lands nothing for it, not a tool row and
    /// not an error, and the frames around it read as they always did.
    #[test]
    fn an_approval_request_lands_nothing_on_either_stream() {
        for run in ["run_1", "chatcmpl-1"] {
            let mut p = AgentStreamParser::new();
            let events = p.feed(
                format!(
                    "event: approval.request\ndata: {{\"execution_generation\": 1, \"command\": \"rm -rf /tmp/x\", \"description\": \"delete in root path\", \"request_id\": \"q1\", \"event\": \"approval.request\", \"run_id\": \"{run}\", \"choices\": [\"once\", \"deny\"]}}\n\nevent: assistant.delta\ndata: {{\"message_id\": \"m1\", \"delta\": \"ok\", \"run_id\": \"{run}\"}}\n\n"
                )
                .as_bytes(),
            );
            assert_eq!(
                events,
                vec![AgentStreamEvent::Content { text: "ok".into() }],
                "{run}"
            );
        }
    }

    /// Three runs of one tool inside one reply are three calls — three ids,
    /// three rows — and each call's completion folds into ITS row.
    #[test]
    fn repeated_calls_of_one_tool_get_their_own_ids() {
        let mut p = AgentStreamParser::new();
        let mut tools: Vec<ToolCall> = Vec::new();
        let frames = concat!(
            "event: tool.started\n",
            r#"data: {"message_id":"m1","tool_name":"terminal","preview":"date"}"#,
            "\n\n",
            "event: tool.completed\n",
            r#"data: {"message_id":"m1","tool_name":"terminal","preview":"date"}"#,
            "\n\n",
            "event: tool.started\n",
            r#"data: {"message_id":"m1","tool_name":"terminal","preview":"date"}"#,
            "\n\n",
            "event: tool.progress\n",
            r#"data: {"message_id":"m1","tool_name":"terminal","delta":"1792"}"#,
            "\n\n",
            "event: tool.completed\n",
            r#"data: {"message_id":"m1","tool_name":"terminal","preview":"date"}"#,
            "\n\n",
            "event: tool.started\n",
            r#"data: {"message_id":"m1","tool_name":"terminal","preview":"date"}"#,
            "\n\n",
        );
        let mut ids = Vec::new();
        for ev in p.feed(frames.as_bytes()) {
            if let AgentStreamEvent::Tool {
                id,
                name,
                label,
                status,
            } = ev
            {
                ids.push(id.clone().unwrap());
                tools = merge_tool(&tools, id.as_deref(), &name, &label, status.as_deref());
            }
        }
        assert_eq!(
            ids,
            vec![
                "m1:terminal#1",
                "m1:terminal#1",
                "m1:terminal#2",
                "m1:terminal#2",
                "m1:terminal#2",
                "m1:terminal#3"
            ]
        );
        assert_eq!(tools.len(), 3, "{tools:?}");
        assert_eq!(tools[0].status, "completed");
        assert_eq!(tools[1].status, "completed");
        assert_eq!(tools[2].status, "running");
    }

    #[test]
    fn a_session_tool_pair_merges_into_one_row() {
        // The synthesised id is what makes this one row: Hermes names no
        // tool-call id on either frame.
        let mut p = AgentStreamParser::new();
        let mut tools: Vec<ToolCall> = Vec::new();
        let frames = concat!(
            "event: tool.started\n",
            r#"data: {"message_id":"msg_1","tool_name":"terminal","preview":"🖥 ls -la"}"#,
            "\n\n",
            "event: tool.completed\n",
            r#"data: {"message_id":"msg_1","tool_name":"terminal","preview":""}"#,
            "\n\n",
        );
        for ev in p.feed(frames.as_bytes()) {
            if let AgentStreamEvent::Tool {
                id,
                name,
                label,
                status,
            } = ev
            {
                tools = merge_tool(&tools, id.as_deref(), &name, &label, status.as_deref());
            }
        }
        assert_eq!(
            tools,
            vec![ToolCall {
                id: Some("msg_1:terminal#1".into()),
                name: "terminal".into(),
                label: "🖥 ls -la".into(),
                status: "completed".into(),
            }]
        );
    }

    #[test]
    fn a_session_error_frame_names_the_failure_whatever_shape_it_came_in() {
        let shapes = [
            (
                r#"data: {"run_id":"r1","message":"the model refused"}"#,
                "the model refused",
            ),
            (
                r#"data: {"run_id":"r1","error":{"code":"run_failed","message":"container died"}}"#,
                "container died",
            ),
            (
                r#"data: {"session_id":"c1","error":"tool loop"}"#,
                "tool loop",
            ),
            (
                r#"data: {"run_id":"r1","seq":9}"#,
                "the agent reported an error",
            ),
        ];
        for (data, want) in shapes {
            let mut p = AgentStreamParser::new();
            let events = p.feed(format!("event: error\n{data}\n\n").as_bytes());
            assert_eq!(
                events,
                vec![AgentStreamEvent::Error {
                    message: want.into()
                }],
                "{data}"
            );
        }
    }

    #[test]
    fn a_session_turn_seals_the_message_and_the_ephemeral_prompt() {
        let mut vault = SecretVault::default();
        let body = session_turn_body(
            &json!("deploy with sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Some("the fallback key is sk-ant-bbbbbbbbbbbbbbbbbbbbbbbbbb"),
            None,
            &mut vault,
        );
        let message = body["message"].as_str().expect("message is a string");
        let system = body["system_message"].as_str().expect("system is a string");
        assert!(!message.contains("sk-ant-"), "{message}");
        assert!(!system.contains("sk-ant-"), "{system}");
        // Two distinct credentials, two handles — and the prose around them is
        // untouched.
        assert_eq!(vault.sealed.len(), 2);
        assert!(message.starts_with("deploy with «secret:"));

        // No ephemeral prompt this turn: the key is absent, not null — Hermes
        // reads a null as a prompt.
        let bare = session_turn_body(&json!("hello"), None, None, &mut vault);
        assert_eq!(bare, json!({ "message": "hello" }));
    }

    /// THE EFFORT RIDES WHERE HERMES READS IT. Its API server decodes a
    /// request's effort from `model_options.reasoning_effort` only; the
    /// OpenAI top-level `reasoning_effort` is ignored and the agent runs at
    /// its configured default. Both arms spell it the same way, and a turn
    /// with no pick carries no `model_options` at all.
    #[test]
    fn a_persona_turns_effort_rides_in_model_options_on_both_arms() {
        let mut vault = SecretVault::default();
        let session = session_turn_body(&json!("hi"), None, Some("high"), &mut vault);
        assert_eq!(
            session,
            json!({ "message": "hi", "model_options": { "reasoning_effort": "high" } })
        );
        let completion = chat_payload("dex-ops", &json!([]), Some("low"));
        assert_eq!(
            completion,
            json!({ "model": "dex-ops", "messages": [], "model_options": { "reasoning_effort": "low" } })
        );
        assert!(completion.get("reasoning_effort").is_none());
        let none = chat_payload("dex-ops", &json!([]), None);
        assert_eq!(none, json!({ "model": "dex-ops", "messages": [] }));
    }

    /// A stop is a verdict, not a failure. Hermes ends the stream with the
    /// words the run managed to say and then `run.cancelled`, so both have to
    /// survive this parser: the partial reply is what the person who pressed
    /// stop asked to keep.
    #[test]
    fn a_stopped_run_keeps_the_words_it_managed_to_say() {
        let mut p = AgentStreamParser::new();
        let events = p.feed(
            concat!(
                "event: assistant.completed\n",
                r#"data: {"session_id":"c1","message_id":"m1","content":"Half an answ","completed":false}"#,
                "\n\n",
                "event: run.cancelled\n",
                r#"data: {"session_id":"c1","run_id":"r1","interrupted":true,"completed":false,"partial":true}"#,
                "\n\n",
                "event: done\n",
                "data: {}\n\n",
            )
            .as_bytes(),
        );
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::Content {
                    text: "Half an answ".into()
                },
                AgentStreamEvent::Stopped,
            ]
        );
    }

    /// `run.steered` is informational — the steered words arrive as the run's
    /// own frames afterwards, so the acknowledgement itself lands nothing.
    #[test]
    fn a_steer_acknowledgement_lands_nothing() {
        let mut p = AgentStreamParser::new();
        assert!(
            p.feed(
                concat!(
                    "event: run.steered\n",
                    r#"data: {"session_id":"c1","run_id":"r1","accepted":true}"#,
                    "\n\n",
                )
                .as_bytes()
            )
            .is_empty()
        );
    }

    /// NO HTTP MOCK EXISTS IN THIS CRATE: what a control call SENDS is tested
    /// where it is assembled — the body here, the bearer and the url in the
    /// `agent_post` door both arms of the turn transport already use — and the
    /// statuses are read, not mocked.
    ///
    /// The body is the one key Hermes reads, and the text goes through the
    /// same sealer a turn's message does: a steer is a person typing into a
    /// running agent, so a pasted credential is as likely here as in a turn.
    #[test]
    fn a_steer_seals_the_text_it_carries() {
        let mut vault = SecretVault::default();
        let body = steer_body("use sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaa now", &mut vault);
        let input = body["input"].as_str().expect("input is a string");
        assert_eq!(body.as_object().map(serde_json::Map::len), Some(1));
        assert!(!input.contains("sk-ant-"), "{input}");
        assert_eq!(vault.sealed.len(), 1);
        // The handle replaces the credential and nothing else.
        assert!(input.starts_with("use «secret:"), "{input}");
        assert!(input.ends_with(" now"), "{input}");
    }

    /// The session overview is a COUNT AND A BOUNDARY: the hidden rows are
    /// how much Hermes compacted away, and the first row it still renders is
    /// how far back what is left reaches. The carriers' own content is empty
    /// on the wire — there is no summary here to leak, and nothing in this
    /// parser reaches for one.
    #[test]
    fn the_overview_counts_hidden_rows_and_dates_the_first_survivor() {
        let body = serde_json::json!({
            "object": "list",
            "session_id": "c1",
            "data": [
                { "id": "m1", "role": "user", "display_kind": "hidden", "content": "",
                  "timestamp": "2026-10-01T09:00:00.000Z" },
                { "id": "m2", "role": "assistant", "display_kind": "hidden", "content": "",
                  "timestamp": "2026-10-02T09:00:00.000Z" },
                { "id": "m3", "role": "user", "display_kind": "message", "content": "still here",
                  "timestamp": "2026-10-03T09:00:00.000Z" },
                { "id": "m4", "role": "assistant", "display_kind": "message", "content": "yes",
                  "timestamp": "2026-10-04T09:00:00.000Z" },
            ],
            "pagination": { "has_more": false },
        });
        assert_eq!(
            parse_session_rows(&body),
            Some((2, Some("2026-10-03T09:00:00.000Z".into()), false))
        );
    }

    /// A page of nothing but carriers has no surviving row to date from, and
    /// a row whose `display_kind` is absent is a visible row — the hidden
    /// projection is the only thing that hides one.
    #[test]
    fn an_all_hidden_page_has_no_since_and_a_bare_row_counts_as_visible() {
        let hidden_only = serde_json::json!({
            "data": [{ "display_kind": "hidden", "timestamp": "2026-10-01T09:00:00.000Z" }],
        });
        assert_eq!(parse_session_rows(&hidden_only), Some((1, None, false)));

        let bare = serde_json::json!({
            "data": [{ "role": "user", "timestamp": "2026-10-05T09:00:00.000Z" }],
        });
        assert_eq!(
            parse_session_rows(&bare),
            Some((0, Some("2026-10-05T09:00:00.000Z".into()), false))
        );
    }

    /// A 200 that carries no rows at all is not an empty session, it is a
    /// different API — and the route's answer for it is the same "this does
    /// not apply here" every other failure gets.
    #[test]
    fn a_partial_page_marks_the_count_as_a_floor() {
        let body = serde_json::json!({
            "session_id": "c1",
            "data": [{ "id": "m1", "display_kind": "hidden", "timestamp": "2026-10-01T00:00:00.000Z" }],
            "pagination": { "has_more": true }
        });
        assert_eq!(parse_session_rows(&body), Some((1, None, true)));
    }

    #[test]
    fn a_body_without_rows_is_no_overview() {
        assert_eq!(parse_session_rows(&serde_json::json!({})), None);
        assert_eq!(
            parse_session_rows(&serde_json::json!({ "data": "not a list" })),
            None
        );
        // An empty session IS an overview: nothing compacted, nothing to date.
        assert_eq!(
            parse_session_rows(&serde_json::json!({ "data": [] })),
            Some((0, None, false))
        );
    }

    #[test]
    fn the_message_count_is_read_only_when_it_is_a_number() {
        assert_eq!(
            parse_message_count(&serde_json::json!({ "message_count": 42 })),
            Some(42)
        );
        assert_eq!(
            parse_message_count(&serde_json::json!({ "message_count": null })),
            None
        );
        assert_eq!(parse_message_count(&serde_json::json!({})), None);
    }

    /// A port nothing listens on: bound once to learn a free one, then let go.
    fn closed_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    /// A refused agent reads as refused, and nowhere in it is the agent's
    /// address — reqwest's own Display would have been "error sending
    /// request for url (http://127.0.0.1:<port>/v1/runs/r/stop)".
    #[tokio::test]
    async fn a_refused_agent_names_the_cause_and_not_the_dial_url() {
        let entry = ManifestEntry {
            model: "x-eng".into(),
            url: format!("http://127.0.0.1:{}", closed_port()),
            key: Some("k".into()),
        };
        let e = agent_post(&entry, "/v1/runs/r/stop")
            .send()
            .await
            .expect_err("nothing listens there");
        assert!(e.to_string().contains("for url"), "the leak this replaces");
        let text = transport_error(e);
        assert!(!text.contains("127.0.0.1"), "{text}");
        assert!(!text.contains("for url"), "{text}");
        assert!(!text.contains("/v1/runs"), "{text}");
        assert!(text.to_ascii_lowercase().contains("refused"), "{text}");
    }

    /// The one transport failure a turn surfaces as text — a reply that
    /// broke after the request went out — says what broke, not where.
    #[tokio::test]
    async fn a_turn_that_breaks_after_the_send_says_so_without_the_url() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = conn.read(&mut buf);
            // Not HTTP: the request was taken, and the answer is garbage.
            let _ = conn.write_all(b"HTTP/9.9 what\r\n\r\n");
        });
        let entry = ManifestEntry {
            model: "x-eng".into(),
            url: format!("http://127.0.0.1:{port}"),
            key: None,
        };
        let stream = open_stream(agent_post(&entry, "/v1/chat/completions"), &json!({}))
            .await
            .expect("an answer, not a refusal");
        server.join().unwrap();
        assert_eq!(stream.status, 502);
        let text: Vec<u8> = stream
            .body
            .map(|c| c.expect("the synthesized body"))
            .collect::<Vec<_>>()
            .await
            .concat();
        let text = String::from_utf8(text).unwrap();
        assert!(
            text.starts_with("the agent gateway failed after the request was sent: "),
            "{text}"
        );
        assert!(!text.contains("127.0.0.1"), "{text}");
        assert!(!text.contains(&port.to_string()), "{text}");
        assert!(!text.contains("for url"), "{text}");
    }

    #[tokio::test]
    async fn canned_stream_reads_back_word_chunks_then_done() {
        let stream = canned_chat_stream("one two", "mock");
        assert_eq!(stream.status, 200);
        let collected: Vec<_> = stream
            .body
            .map(|c| c.expect("canned chunk"))
            .collect::<Vec<_>>()
            .await;
        assert_eq!(collected.len(), 3); // one two [DONE]
        assert!(collected[0].starts_with(b"data: {"));
        assert_eq!(&*collected[2], b"data: [DONE]\n\n");
    }

    /// A session that carried turns before is looked up, and only Hermes' own
    /// `session_not_found` says the room is gone. A bare 404 is a Hermes with
    /// no session API, which waits like a container still booting.
    #[test]
    fn a_session_look_up_reads_gone_only_when_hermes_says_so() {
        assert_eq!(session_lookup_answer(200, None), SessionCreate::Existing);
        assert!(matches!(
            session_lookup_answer(404, Some("session_not_found")),
            SessionCreate::Lost(_)
        ));
        assert_eq!(session_lookup_answer(404, None), SessionCreate::NotReady);
        assert_eq!(session_lookup_answer(503, None), SessionCreate::NotReady);
        assert_eq!(
            session_lookup_answer(401, Some("invalid_api_key")),
            SessionCreate::NotReady
        );
    }

    /// A create that Hermes refuses as `session_deleted` is a room gone for
    /// good, not a container still coming up: the id is fenced and never
    /// reused, so holding the turn for two minutes and answering "restarting"
    /// was wrong every time.
    #[test]
    fn a_deleted_session_is_lost_not_a_container_booting() {
        assert_eq!(session_create_answer(201, None), SessionCreate::Fresh);
        assert_eq!(
            session_create_answer(409, Some("session_exists")),
            SessionCreate::Existing
        );
        assert!(matches!(
            session_create_answer(409, Some("session_deleted")),
            SessionCreate::Lost(_)
        ));
        assert_eq!(session_create_answer(409, None), SessionCreate::NotReady);
        assert_eq!(session_create_answer(404, None), SessionCreate::NotReady);
    }
}
