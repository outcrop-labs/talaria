// The fleet chat transport — proxy_chat plus the agent SSE stream parser, the
// pair the comms reply loop drives. Every Hermes persona turn in the tree is
// sent from here to the agent's own container, which then talks to a provider
// we do not control the request assembly for — the door that matters most,
// because a persona holds workspace context all day and is the agent most
// likely to have a credential in front of it.
//
// WHY THIS IS NOT THE GATEWAY RELAY: the manifest entry IS the agent — a
// container with its own local gateway at `url`. The registry relay
// (routes/llm_chat.rs) serves workspace API keys against provider endpoints;
// this path serves TALKING TO THE AGENTS THEMSELVES, and only the comms/chat
// planes do it.

use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::fleet::{describe_agent, read_manifest};
use crate::gateway::provider::http;
use crate::gateway::vault::{SecretVault, seal_content};

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
    for s in &vault.sealed {
        tracing::warn!("[secrets] sealed {} out of a turn to {model}", s.label);
    }

    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms.unwrap_or(120_000));
    let mut attempt: u32 = 0;
    loop {
        let manifest = read_manifest().await;
        let entry = manifest.iter().find(|m| m.model == model);
        // Two strikes with no manifest row at all → mock mode: no agents are
        // rendered yet, and the reply says so instead of hanging.
        if entry.is_none() && attempt >= 2 {
            return canned_chat_stream(&mock_reply(&model), "mock");
        }

        if let Some(entry) = entry {
            let mut request = payload.clone();
            // include_usage: the final chunk reports token counts for the
            // ledger (gateways that don't support it just ignore the option).
            request["stream"] = json!(true);
            request["stream_options"] = json!({ "include_usage": true });
            let mut req = http()
                .post(format!("{}/v1/chat/completions", entry.url))
                .header("content-type", "application/json");
            if let Some(key) = entry.key.as_deref().filter(|k| !k.is_empty()) {
                req = req.bearer_auth(key);
            }
            // NO TOTAL TIMEOUT ON THE TURN. A per-request .timeout() owns the
            // body end to end, and an agent turn is one long stream: a
            // knowledgebase buildout works for an hour, and the 600s ceiling
            // this replaces killed it at ten minutes while the container was
            // still demonstrably working — the API hung up on a live agent and
            // called the turn "interrupted". Liveness is the DRAIN LOOP's
            // idle ceiling (chat_persist's STREAM_IDLE): as long as frames
            // arrive, the turn runs; only silence ends it. What still needs a
            // bound here is the wait for the response's HEADERS — a container
            // that accepted the request and never opened the stream — because
            // connect_timeout covers TCP only.
            let sent = tokio::time::timeout(HEADERS_WINDOW, req.json(&request).send()).await;
            if let Ok(Ok(upstream)) = sent {
                let status = upstream.status().as_u16();
                // 502/503/504 = the gateway process is up but not serving yet
                // — keep waiting. Any other answer (including a real error
                // status) is the reply.
                if ![502, 503, 504].contains(&status) {
                    let content_type = upstream
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("text/event-stream")
                        .to_string();
                    return ChatStream {
                        status,
                        content_type,
                        body: Box::pin(upstream.bytes_stream()),
                        canned: None,
                    };
                }
            }
        }

        if tokio::time::Instant::now() >= deadline {
            return canned_chat_stream(&unavailable_reply(&model), "hold");
        }
        attempt += 1;
        tokio::time::sleep(Duration::from_millis(
            (1_500u64 * attempt as u64).min(5_000),
        ))
        .await;
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
        /// 'running' | 'completed' — a later frame for the same call flips it.
        status: Option<String>,
    },
    Usage {
        prompt_tokens: i64,
        completion_tokens: i64,
    },
    /// The agent gateway's failure contract: a 200-SSE body whose final chunk
    /// carries `finish_reason: "error"` with a top-level `error: {message}`
    /// (and a `hermes: {failed: true}` block) — the provider call died inside
    /// the container and this frame is the only place the reason exists. The
    /// frame parses to no content, so a parser that ignores it reads a failed
    /// turn as an EMPTY COMPLETE one, and the chat chain re-fires it forever
    /// (prior_messages sees through empty rows, so the user's message reads as
    /// forever-unanswered). Consumers surface the text and stop the turn.
    Error {
        message: String,
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
            events.extend(parse_frame(&frame[..frame.len() - 2]));
        }
        events
    }

    /// End of body: parse whatever the final frame left behind.
    pub fn finish(&mut self) -> Vec<AgentStreamEvent> {
        let rest = std::mem::take(&mut self.buffer);
        parse_frame(&rest)
    }
}

fn parse_frame(frame: &str) -> Vec<AgentStreamEvent> {
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
