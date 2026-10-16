// The fleet chat transport — proxy_chat, proxy_session_turn, and the agent SSE
// stream parser the pair of them share: the comms reply loop drives one of the
// two and reads the same events out either way. The difference is only who
// keeps the transcript — we do, as a `messages` array, or the agent does, as
// a session we send one message at a time.
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

use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::provider::http;
use crate::vault::{SecretVault, seal_content};
use talaria_fleet_layout::{ManifestEntry, describe_agent, read_manifest};

use std::pin::Pin;
use std::time::Duration;

pub type ByteStream = Pin<Box<dyn futures_util::Stream<Item = reqwest::Result<Bytes>> + Send>>;

/// How long the turn waits for the agent gateway to OPEN the stream (the
/// response's headers) before the retry/deadline loop takes over. Generous on
/// purpose: the agent's first frame rides its provider's time-to-first-token,
/// and a long context — or a mid-task compaction — can hold that for minutes.
/// Only silence past this reads as "not serving yet".
const HEADERS_WINDOW: Duration = Duration::from_secs(300);

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
}

impl ChatStream {
    /// A 2xx answer is the stream the caller wants; every other status is
    /// the reply, body and all, but not one to persist.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The chat plane's payload — `{model, messages}` plus `reasoning_effort`
/// when one was honored, in that key order.
pub fn chat_payload(model: &str, messages: &Value, effort: Option<&str>) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("model".into(), json!(model));
    payload.insert("messages".into(), messages.clone());
    if let Some(effort) = effort {
        payload.insert("reasoning_effort".into(), json!(effort));
    }
    Value::Object(payload)
}

/// Send one chat turn to the agent's container. Seals every
/// message's content through a per-call vault (an image turn's text part is as
/// credential-prone as any prose turn), retries while the agent's gateway is
/// coming up (502/503/504, backoff capped at 5s), and never waits past two
/// minutes — past the deadline the reply is the honest canned sentence, so
/// history shows what happened rather than a silent failure.
///
/// `payload` is the WHOLE request body the agent's gateway sees — the turn
/// payload plus `stream: true` and `stream_options` — so a persona turn's
/// `temperature`/`response_format` ride alongside model and messages, and the
/// one mapping that decides them (`persona_payload`) is the only place they
/// can be dropped. `messages` is the OpenAI history (system + turns, content
/// string or parts). `wait_ms` is the hold window; None is the two-minute
/// default.
pub async fn proxy_chat(payload: &Value, wait_ms: Option<u64>) -> ChatStream {
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

    hold_for_agent(&model, AgentRequest::ChatCompletions { payload }, wait_ms).await
}

/// Send one turn on a Hermes SESSION — the arm for an agent that keeps its own
/// conversation history. Talaria sends only the NEW message plus the turn's
/// ephemeral system prompt; the transcript itself lives in the container,
/// keyed by `conversation_id`. That is why this is not `proxy_chat` with
/// another path: there is no `messages` array to assemble here, so there is no
/// way for our idea of what was said to drift from the agent's.
///
/// Everything else is `proxy_chat`'s contract, deliberately: the session is
/// created on the way in (idempotently — an existing one answers 409, which is
/// the steady state after the first turn), a container that isn't serving yet
/// is held for, the deadline answers with the honest canned sentence, and no
/// manifest row at all is mock mode. `message` is a string or OpenAI content
/// parts; `system_message` is the per-turn prompt Hermes calls ephemeral. Both
/// are sealed through a per-call vault before they leave the process.
pub async fn proxy_session_turn(
    agent_model: &str,
    conversation_id: &str,
    system_message: Option<&str>,
    message: &Value,
    wait_ms: Option<u64>,
) -> ChatStream {
    let mut vault = SecretVault::default();
    let body = session_turn_body(message, system_message, &mut vault);
    report_sealed(&vault, agent_model);

    hold_for_agent(
        agent_model,
        AgentRequest::SessionTurn {
            conversation_id: conversation_id.to_string(),
            body,
        },
        wait_ms,
    )
    .await
}

/// The session turn's request body, sealed. The system prompt goes through the
/// same sealer as the message because it is assembled from workspace state — a
/// task's own text, a board's titles — and is exactly as likely to carry a
/// credential as anything a person typed.
fn session_turn_body(
    message: &Value,
    system_message: Option<&str>,
    vault: &mut SecretVault,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("message".into(), seal_content(message, vault));
    if let Some(system) = system_message {
        body.insert("system_message".into(), seal_content(&json!(system), vault));
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
    },
}

impl AgentRequest {
    /// One attempt against this agent's own gateway. `None` means "not serving
    /// yet" — the hold loop backs off and tries again until its deadline.
    async fn open(&self, entry: &ManifestEntry) -> Option<ChatStream> {
        match self {
            AgentRequest::ChatCompletions { payload } => {
                let mut request = payload.clone();
                // include_usage: the final chunk reports token counts for the
                // ledger (gateways that don't support it just ignore the option).
                request["stream"] = json!(true);
                request["stream_options"] = json!({ "include_usage": true });
                open_stream(agent_post(entry, "/v1/chat/completions"), &request).await
            }
            AgentRequest::SessionTurn {
                conversation_id,
                body,
            } => {
                // The session must exist before a turn can land on it, and the
                // create is the only place we can learn that it doesn't. A
                // refusal here is treated exactly like a gateway that isn't
                // serving yet — on a container that is still booting that is
                // precisely what it is.
                if !ensure_session(entry, conversation_id).await {
                    return None;
                }
                open_stream(
                    agent_post(
                        entry,
                        &format!("/api/sessions/{conversation_id}/chat/stream"),
                    ),
                    body,
                )
                .await
            }
        }
    }
}

/// The hold: retry while the agent's gateway is coming up (502/503/504,
/// backoff capped at 5s) and never wait past the window — past the deadline
/// the reply is the honest canned sentence, so history shows what happened
/// rather than a silent failure. `wait_ms` None is the two-minute default.
async fn hold_for_agent(model: &str, request: AgentRequest, wait_ms: Option<u64>) -> ChatStream {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms.unwrap_or(120_000));
    let mut attempt: u32 = 0;
    loop {
        let manifest = read_manifest().await;
        let entry = manifest.iter().find(|m| m.model == model);
        // Two strikes with no manifest row at all → mock mode: no agents are
        // rendered yet, and the reply says so instead of hanging.
        if entry.is_none() && attempt >= 2 {
            return canned_chat_stream(&mock_reply(model), "mock");
        }

        if let Some(entry) = entry
            && let Some(stream) = request.open(entry).await
        {
            return stream;
        }

        if tokio::time::Instant::now() >= deadline {
            return canned_chat_stream(&unavailable_reply(model), "hold");
        }
        attempt += 1;
        tokio::time::sleep(Duration::from_millis(
            (1_500u64 * attempt as u64).min(5_000),
        ))
        .await;
    }
}

/// An authenticated POST to one agent's own gateway. The session arm and the
/// chat arm go through the SAME door with the SAME bearer — the manifest row's
/// key, which the renderer put in the container as its API_SERVER_KEY — so the
/// key handling can only be got right or wrong once.
fn agent_post(entry: &ManifestEntry, path: &str) -> reqwest::RequestBuilder {
    let mut req = http()
        .post(format!("{}{path}", entry.url))
        .header("content-type", "application/json");
    if let Some(key) = entry.key.as_deref().filter(|k| !k.is_empty()) {
        req = req.bearer_auth(key);
    }
    req
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
async fn open_stream(req: reqwest::RequestBuilder, body: &Value) -> Option<ChatStream> {
    let sent = tokio::time::timeout(HEADERS_WINDOW, req.json(body).send()).await;
    let Ok(Ok(upstream)) = sent else {
        return None;
    };
    let status = upstream.status().as_u16();
    // 502/503/504 = the gateway process is up but not serving yet — keep
    // waiting. Any other answer (including a real error status) is the reply.
    if [502, 503, 504].contains(&status) {
        return None;
    }
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_string();
    Some(ChatStream {
        status,
        content_type,
        body: Box::pin(upstream.bytes_stream()),
        canned: None,
    })
}

/// Create the Hermes session this turn lands on, idempotently: a success is a
/// fresh session, and 409 `session_exists` is the one an earlier turn made —
/// the normal answer for every turn but the first. Anything else (including a
/// transport error) reads as "this container is not ready", which is the same
/// answer the stream itself gives while a container boots.
///
/// The session carries no `system_prompt` of its own on purpose: every turn
/// sends its own ephemeral one, so there is no stored prompt here to go stale
/// against the workspace it was rendered from. The headers window is the same
/// one the stream uses — a create that answers at all answers in milliseconds,
/// and the bound exists for the container that accepts TCP and then says
/// nothing.
async fn ensure_session(entry: &ManifestEntry, conversation_id: &str) -> bool {
    let body = json!({ "id": conversation_id });
    let sent = tokio::time::timeout(
        HEADERS_WINDOW,
        agent_post(entry, "/api/sessions").json(&body).send(),
    )
    .await;
    let Ok(Ok(res)) = sent else {
        return false;
    };
    let status = res.status();
    if status.is_success() {
        return true;
    }
    // A 409 is "exists" only when it says so: any other conflict is a
    // container refusing the session, and the stream POST would then be
    // talking to a room that was never made.
    if status == reqwest::StatusCode::CONFLICT {
        let body: Value = res.json().await.unwrap_or(Value::Null);
        return body["error"]["code"] == "session_exists";
    }
    // A Hermes without the session API at all. The hold below treats it as
    // a container still booting, which is the one thing it is not — so the
    // log names it, because nothing else in a two-minute hold will.
    if status == reqwest::StatusCode::NOT_FOUND {
        eprintln!(
            "[fleet] {}: POST /api/sessions answered 404 — this Hermes has no session API; the agent cannot be pinned",
            entry.url
        );
    }
    false
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
    pub status: String, // 'running' | 'completed'
}

/// Fold a tool event into a running list (dedupe by id when the frame names
/// one, else by name among the still-running). A later frame never overwrites
/// a good label with an empty one.
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
                status: status.unwrap_or(&existing.status).to_string(),
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
            events.extend(parse_frame(&frame[..frame.len() - 2], &mut self.streamed));
        }
        events
    }

    /// End of body: parse whatever the final frame left behind.
    pub fn finish(&mut self) -> Vec<AgentStreamEvent> {
        let rest = std::mem::take(&mut self.buffer);
        parse_frame(&rest, &mut self.streamed)
    }
}

fn parse_frame(frame: &str, streamed: &mut bool) -> Vec<AgentStreamEvent> {
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
        && let Some(events) = parse_session_frame(&event_name, &json, streamed)
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
    // keeps the prior value; a push says 'running').
    let s = str_of("status").to_lowercase();
    let status = match s.as_str() {
        "running" => Some("running".to_string()),
        "completed" | "complete" => Some("completed".to_string()),
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
            let mut ev = session_tool(json, "running");
            if let AgentStreamEvent::Tool { label, .. } = &mut ev {
                let delta = str_of("delta").trim();
                if !delta.is_empty() {
                    *label = delta.chars().take(200).collect();
                }
            }
            Some(vec![ev])
        }
        "tool.started" => Some(vec![session_tool(json, "running")]),
        // A failure is still a FINISHED call — the persisted vocabulary is two
        // words, running and completed — so the preview is where a failure has
        // to read, and Hermes writes the reason into it.
        "tool.completed" | "tool.failed" => Some(vec![session_tool(json, "completed")]),
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
        // The run's own verdict, after the reply: the counts ride here, and
        // a failed or cancelled run ends the turn as an ERROR so the chain
        // does not re-fire an unanswered message forever. (A failed run's
        // `assistant.completed` carries Hermes' apology sentence as its
        // content; the error row's reason wins over it.)
        "run.completed" => Some(session_usage(json)),
        "run.failed" => Some(vec![AgentStreamEvent::Error {
            message: match session_error_message(json) {
                m if m == "the agent reported an error" => "the run failed".to_string(),
                m => m,
            },
        }]),
        "run.cancelled" => Some(vec![AgentStreamEvent::Error {
            message: "the run was stopped".to_string(),
        }]),
        "run.queued" => Some(Vec::new()),
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
fn session_tool(json: &Value, status: &str) -> AgentStreamEvent {
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
            (!message.is_empty()).then(|| format!("{message}:{name}"))
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
                    "event: tool.progress\n",
                    r#"data: {"message_id":"msg_1","tool_name":"terminal","preview":"ls -la","args":{}}"#,
                    "\n\n",
                    "event: tool.started\n",
                    r#"data: {"message_id":"msg_1","tool_name":"terminal","preview":"🖥 ls -la","args":{}}"#,
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
                    id: Some("msg_1:terminal".into()),
                    name: "terminal".into(),
                    label: "ls -la".into(),
                    status: Some("running".into()),
                },
                AgentStreamEvent::Tool {
                    id: Some("msg_1:terminal".into()),
                    name: "terminal".into(),
                    label: "🖥 ls -la".into(),
                    status: Some("running".into()),
                },
                AgentStreamEvent::Tool {
                    id: Some("msg_1:terminal".into()),
                    name: "terminal".into(),
                    label: String::new(),
                    status: Some("completed".into()),
                },
                AgentStreamEvent::Tool {
                    id: Some("msg_1:memory".into()),
                    name: "memory".into(),
                    label: "memory: write refused".into(),
                    status: Some("completed".into()),
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
                    id: Some("m1:terminal".into()),
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
        assert_eq!(
            events,
            vec![AgentStreamEvent::Error {
                message: "the run was stopped".into()
            }]
        );
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
                id: Some("msg_1:terminal".into()),
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
        let bare = session_turn_body(&json!("hello"), None, &mut vault);
        assert_eq!(bare, json!({ "message": "hello" }));
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
}
