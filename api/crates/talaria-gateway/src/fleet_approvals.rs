// Hermes' approval requests on the turns Talaria drives, refused at the
// door and recorded.
//
// WHY THIS EXISTS. A STREAMED turn (POST /api/sessions/{id}/chat/stream, or
// /v1/chat/completions with `stream: true`, which between them carry every
// chat, channel reply and work session the fleet chat door sends) registers
// an approval notifier inside Hermes (v0.22 and later). Hermes then treats
// the turn as attended. A flagged terminal command no longer resolves from
// `approvals.unattended_mode`, the key the render pins; it is put to whoever
// reads the stream, as an `approval.request` frame, and the agent's turn
// blocks until someone answers `POST /v1/runs/{run_id}/approval` or
// `approvals.timeout` runs out. Nothing here read that frame. Every flagged
// command on a streamed turn stalled its turn, and the session's next turn
// queued behind it. Under Hermes' default `smart` mode an LLM guardian
// answered first, and could approve a command on an agent whose policy is
// deny.
//
// THE FIX HAS TWO HALVES. The render pins `approvals.mode`
// (talaria-fleet-render's `fleet_approvals`: `manual` with deny, or `off`
// for a Developer Agent), so no model approves anything. This module is the
// other half. The door taps each stream as its bytes go past and answers
// every request `deny`, then records it in the audit log. The bytes pass
// through untouched, so every consumer downstream still sees the frame and
// ignores it as it always did.
//
// WHY THE DOOR NEVER CONSENTS, NOT EVEN FOR A DEVELOPER AGENT. A Developer
// Agent's policy is that flagged commands run, and `mode: off` already says
// so inside the container: Hermes skips every gate that policy covers (the
// dangerous-command and execute_code guards, plugin rules, ssh config and
// computer-use writes) without asking. What a Developer Agent still asks is
// a gate Hermes built never to be auto-approved: a write to a project's
// AGENTS.md / CLAUDE.md / SOUL.md / .cursorrules / `.hermes/` (the
// prompt-injection persistence vector), an untrusted MCP server's write tool
// or spend limit, a saved payment card fill. Each ignores yolo and
// `mode: off`, and each wants a person. This door is not one. Nor can it
// tell those apart from a command: v0.22's session authority projects a
// request down to its command, description, choices and generation, and
// drops Hermes' `pattern_key`, so a card fill reads exactly like `rm -rf`.
// A door that answered `once` would be consenting, blind, to whichever one
// it got. The only answer it can give for every request is `deny`.
//
// The answer goes through the agent's own door with its own bearer, the same
// as a steer or a stop, inside the same control window. If the answer cannot
// be delivered (the agent went away, or Hermes refused the body), Hermes
// refuses the call itself when `approvals.timeout` runs out. It fails closed
// either way.

use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use axum::body::Bytes;
use futures_util::Stream;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use crate::fleet_chat::{ByteStream, CONTROL_WINDOW, ChatStream, agent_post, control_failure};
use talaria_fleet_layout::ManifestEntry;

/// One `approval.request` frame, as Hermes writes it on both streams.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalAsked {
    /// The run the answer is addressed to: `run_…` on the session stream,
    /// the completion id (`chatcmpl-…`) on the completions stream.
    pub run_id: String,
    pub request_id: String,
    /// The generation fence a gateway with a session authority (every v0.22
    /// gateway) puts on the request. The answer must carry it back, and that
    /// door refuses any key beyond the three. Absent on the older in-memory
    /// bridge, which takes `{request_id, choice}`.
    pub execution_generation: Option<i64>,
    /// The flagged command as Hermes redacted it before it left the
    /// container. It is a record, never re-sent.
    pub command: String,
    /// Hermes' own name for why it is flagged ("delete in root path").
    pub description: String,
}

/// The one answer this door gives (see the header for why it is never
/// `once`). `session` and `always` would also write into a config.yaml the
/// render owns.
pub const ANSWER: &str = "deny";

/// One answered request, for the audit log.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRecord {
    /// The agent the turn belongs to, the name its def is found under.
    pub agent_model: String,
    pub run_id: String,
    /// Redacted again with Talaria's own credential patterns, and clamped.
    pub command: String,
    pub description: String,
    /// None when Hermes took the answer, otherwise why it did not.
    pub failure: Option<String>,
}

pub type RecordFn = Arc<dyn Fn(ApprovalRecord) -> BoxFuture<'static, ()> + Send + Sync>;

/// Where an answered request is recorded. Wired from the boot assembly
/// (talaria-jobs), because this crate holds no pool. Unset records nothing,
/// and the answer still goes out.
pub static RECORD_APPROVAL: OnceLock<RecordFn> = OnceLock::new();

/// The body `POST /v1/runs/{run_id}/approval` takes: exactly
/// `{request_id, execution_generation, choice}` when the request carried a
/// generation, and `{request_id, choice}` when it did not.
pub fn answer_body(asked: &ApprovalAsked) -> Value {
    let mut body = json!({ "request_id": asked.request_id, "choice": ANSWER });
    if let Some(generation) = asked.execution_generation {
        body["execution_generation"] = json!(generation);
    }
    body
}

/// The run id goes into a URL path, and it came from the container: only
/// the alphabet Hermes mints ids in passes.
fn safe_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Read one SSE frame (the text between blank lines) as an approval request.
/// Only a frame NAMED `approval.request` counts: the name line is Hermes'
/// own, while a payload is JSON on one line, so a reply that merely talks
/// about approvals cannot pass for one.
pub fn approval_asked(frame: &str) -> Option<ApprovalAsked> {
    let mut named = false;
    let mut data: Vec<&str> = Vec::new();
    for line in frame.split('\n') {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("event:") {
            named = name.trim() == "approval.request";
        } else if let Some(d) = line.strip_prefix("data:") {
            data.push(d.trim());
        }
    }
    if !named {
        return None;
    }
    let json: Value = serde_json::from_str(&data.join("\n")).ok()?;
    let text = |k: &str| {
        json.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let run_id = text("run_id");
    let request_id = text("request_id");
    if !safe_run_id(&run_id) || request_id.is_empty() || request_id.len() > 256 {
        return None;
    }
    Some(ApprovalAsked {
        run_id,
        request_id,
        execution_generation: json.get("execution_generation").and_then(Value::as_i64),
        command: text("command"),
        description: text("description"),
    })
}

/// Splits a byte stream into SSE frames and keeps the ones that mention an
/// approval request, the same framing as `AgentStreamParser` (frames end on a
/// blank line). Bytes are buffered, not decoded per chunk, so a multi-byte
/// character split across two chunks stays whole. Only the trailing partial
/// frame is ever held.
#[derive(Default)]
struct ApprovalFrames {
    buffer: Vec<u8>,
    /// How far into `buffer` the boundary search already looked, so a long
    /// frame arriving in many chunks is not rescanned from its start.
    searched: usize,
}

impl ApprovalFrames {
    fn feed(&mut self, chunk: &[u8]) -> Vec<ApprovalAsked> {
        self.buffer.extend_from_slice(chunk);
        let mut asked = Vec::new();
        loop {
            // Back one byte: the blank line may straddle the old end.
            let from = self.searched.saturating_sub(1);
            let Some(at) = self.buffer[from..].windows(2).position(|w| w == b"\n\n") else {
                self.searched = self.buffer.len();
                break;
            };
            let end = from + at;
            let frame: Vec<u8> = self.buffer.drain(..end + 2).collect();
            self.searched = 0;
            if frame
                .windows(b"approval.request".len())
                .any(|w| w == b"approval.request")
                && let Some(a) = approval_asked(&String::from_utf8_lossy(&frame))
            {
                asked.push(a);
            }
        }
        asked
    }
}

/// The stream with the tap on it: every chunk passes through unchanged, and
/// each approval request it completes is refused off to the side.
struct ApprovalTap {
    inner: ByteStream,
    frames: ApprovalFrames,
    entry: ManifestEntry,
    agent_model: String,
}

impl Stream for ApprovalTap {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = this.inner.as_mut().poll_next(cx);
        if let Poll::Ready(Some(Ok(chunk))) = &polled {
            for asked in this.frames.feed(chunk) {
                tokio::spawn(answer(owned(&this.entry), this.agent_model.clone(), asked));
            }
        }
        polled
    }
}

/// Put the tap on a turn's stream. `agent_model` is the agent the turn
/// belongs to (a tier row dials the same container under a longer name), the
/// name the audit row is filed under. A stream that is not a 2xx carries no
/// frames to answer and is returned as it came.
pub fn refuse_approvals(
    mut stream: ChatStream,
    entry: &ManifestEntry,
    agent_model: &str,
) -> ChatStream {
    if !stream.ok() {
        return stream;
    }
    let inner = std::mem::replace(&mut stream.body, Box::pin(futures_util::stream::empty()));
    stream.body = Box::pin(ApprovalTap {
        inner,
        frames: ApprovalFrames::default(),
        entry: owned(entry),
        agent_model: agent_model.to_string(),
    });
    stream
}

/// The manifest row, owned: the tap outlives the manifest read it came from,
/// and each answer runs on its own task.
fn owned(entry: &ManifestEntry) -> ManifestEntry {
    ManifestEntry {
        model: entry.model.clone(),
        url: entry.url.clone(),
        key: entry.key.clone(),
    }
}

/// How much of a flagged command the audit row keeps.
const RECORDED_COMMAND_CHARS: usize = 500;

/// Refuse one request, then record what happened.
async fn answer(entry: ManifestEntry, agent_model: String, asked: ApprovalAsked) {
    let sent = tokio::time::timeout(
        CONTROL_WINDOW,
        agent_post(&entry, &format!("/v1/runs/{}/approval", asked.run_id))
            .json(&answer_body(&asked))
            .send(),
    )
    .await;
    let failure = match sent {
        Ok(Ok(res)) if res.status().is_success() => None,
        Ok(Ok(res)) => Some(control_failure(res).await),
        Ok(Err(e)) => Some(e.to_string()),
        Err(_) => Some("the agent did not answer the approval".into()),
    };
    // The command stays out of the log line: the description names the
    // flag, and the audit row carries the (redacted) command.
    match &failure {
        None => tracing::info!(
            "[approvals] {agent_model}: answered {ANSWER} to a flagged call ({})",
            asked.description
        ),
        Some(why) => tracing::warn!(
            "[approvals] {agent_model}: could not answer {ANSWER} to a flagged call ({}): {why}; \
             Hermes refuses it when approvals.timeout runs out",
            asked.description
        ),
    }
    if let Some(record) = RECORD_APPROVAL.get() {
        let (command, _) = crate::guard::redact_secrets(&asked.command, None);
        record(ApprovalRecord {
            agent_model,
            run_id: asked.run_id,
            command: command.chars().take(RECORDED_COMMAND_CHARS).collect(),
            description: asked.description,
            failure,
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    // Both frames verbatim from a nousresearch/hermes-agent v0.22.4 container
    // (2026-10-22): the session stream's, then the completions stream's.
    const SESSION_FRAME: &str = "event: approval.request\ndata: {\"execution_generation\": 1, \"command\": \"rm -rf /tmp/hx-canary\", \"description\": \"delete in root path\", \"request_id\": \"f27f3b67938548a1af19d5c2d9707b64\", \"allow_session\": true, \"allow_permanent\": true, \"event\": \"approval.request\", \"run_id\": \"run_ea8f1e18253c456db8faeb3133d2c661\", \"timestamp\": 1792694320.0035346, \"message_id\": \"msg_7a2d9fce5f5d40d1804c6b0420f93ea0\", \"choices\": [\"once\", \"session\", \"always\", \"deny\"], \"session_id\": \"probe-s1\", \"seq\": 4, \"ts\": 1792694320.0035727}";
    const COMPLETIONS_FRAME: &str = "event: approval.request\ndata: {\"execution_generation\": 1, \"command\": \"rm -rf /tmp/hx-canary\", \"description\": \"delete in root path\", \"request_id\": \"2eb28921d87d45a9b965df26e0770a11\", \"allow_session\": true, \"allow_permanent\": true, \"event\": \"approval.request\", \"run_id\": \"chatcmpl-d484a8983c0e44eebebe78e2bcd02\", \"timestamp\": 1792694477.077072, \"session_id\": \"api-a38460a763e01fd5\", \"choices\": [\"once\", \"session\", \"always\", \"deny\"]}";
    // A Developer Agent (`approvals.mode: off`) asking to write a project's
    // AGENTS.md, verbatim off a /api/chat turn through this door, same image
    // (2026-10-22). Hermes' protected-instruction gate ignores `mode: off`,
    // and the frame carries no `pattern_key` to say so.
    const PROTECTED_WRITE_FRAME: &str = "event: approval.request\ndata: {\"execution_generation\": 1, \"command\": \"<write to AGENTS.md>\", \"description\": \"Write to protected agent-instruction file(s): AGENTS.md. These files steer future agent behavior; approval is always required (not bypassed by auto-approve).\", \"request_id\": \"44e98bc6bc544867860b37c800ac72cc\", \"allow_session\": false, \"allow_permanent\": false, \"event\": \"approval.request\", \"run_id\": \"run_d87c07107c4244a89973e5bd2618264b\", \"timestamp\": 1792698967.578477, \"message_id\": \"msg_e9afc6df39084ea694dde46b25b83d00\", \"choices\": [\"once\", \"deny\"], \"session_id\": \"6113b5bc-6935-46e1-a67c-60bd58daf4f0\", \"seq\": 4, \"ts\": 1792698967.5785139}";

    #[test]
    fn both_streams_frames_read_as_the_request_they_are() {
        let s = approval_asked(SESSION_FRAME).expect("session frame");
        assert_eq!(s.run_id, "run_ea8f1e18253c456db8faeb3133d2c661");
        assert_eq!(s.request_id, "f27f3b67938548a1af19d5c2d9707b64");
        assert_eq!(s.execution_generation, Some(1));
        assert_eq!(s.command, "rm -rf /tmp/hx-canary");
        assert_eq!(s.description, "delete in root path");

        let c = approval_asked(COMPLETIONS_FRAME).expect("completions frame");
        assert_eq!(c.run_id, "chatcmpl-d484a8983c0e44eebebe78e2bcd02");
        assert_eq!(c.execution_generation, Some(1));
    }

    #[test]
    fn only_a_frame_named_approval_request_counts() {
        // A reply that talks about approvals is a delta, not a request.
        let delta = "event: assistant.delta\ndata: {\"delta\": \"event: approval.request\", \"run_id\": \"run_1\", \"request_id\": \"r\"}";
        assert_eq!(approval_asked(delta), None);
        // Unnamed (a completions chunk) is not one either.
        assert_eq!(
            approval_asked("data: {\"run_id\": \"run_1\", \"request_id\": \"r\"}"),
            None
        );
        // A keep-alive comment.
        assert_eq!(approval_asked(": keepalive"), None);
    }

    #[test]
    fn a_request_without_a_usable_address_is_not_answered() {
        for data in [
            r#"{"request_id": "r1"}"#,
            r#"{"run_id": "run_1"}"#,
            r#"{"run_id": "../../v1/admin", "request_id": "r1"}"#,
            r#"{"run_id": "run 1", "request_id": "r1"}"#,
            "not json",
        ] {
            let frame = format!("event: approval.request\ndata: {data}");
            assert_eq!(approval_asked(&frame), None, "{data}");
        }
    }

    #[test]
    fn every_request_is_refused_whatever_gate_raised_it() {
        // A dangerous command on either stream and a Developer Agent's write
        // to a protected instruction file all get the same body: the door
        // cannot tell them apart, and must not consent to the second.
        for (frame, request_id, run_id) in [
            (
                SESSION_FRAME,
                "f27f3b67938548a1af19d5c2d9707b64",
                "run_ea8f1e18253c456db8faeb3133d2c661",
            ),
            (
                COMPLETIONS_FRAME,
                "2eb28921d87d45a9b965df26e0770a11",
                "chatcmpl-d484a8983c0e44eebebe78e2bcd02",
            ),
            (
                PROTECTED_WRITE_FRAME,
                "44e98bc6bc544867860b37c800ac72cc",
                "run_d87c07107c4244a89973e5bd2618264b",
            ),
        ] {
            let asked = approval_asked(frame).expect(request_id);
            assert_eq!(asked.run_id, run_id);
            assert_eq!(
                answer_body(&asked),
                json!({ "request_id": request_id, "execution_generation": 1, "choice": "deny" })
            );
        }
        let protected = approval_asked(PROTECTED_WRITE_FRAME).unwrap();
        assert_eq!(protected.command, "<write to AGENTS.md>");
    }

    #[test]
    fn the_body_carries_the_generation_back_and_nothing_else() {
        // The older bridge's request has no generation; its body has none.
        let older = ApprovalAsked {
            execution_generation: None,
            ..approval_asked(SESSION_FRAME).unwrap()
        };
        assert_eq!(
            answer_body(&older),
            json!({ "request_id": "f27f3b67938548a1af19d5c2d9707b64", "choice": "deny" })
        );
    }

    #[test]
    fn frames_split_anywhere_across_chunks_are_found_once() {
        let stream = format!(
            "event: tool.started\ndata: {{\"tool_name\": \"terminal\"}}\n\n: keepalive\n\n{SESSION_FRAME}\n\nevent: done\ndata: {{}}\n\n"
        );
        let bytes = stream.as_bytes();
        for cut in 1..bytes.len() {
            let mut frames = ApprovalFrames::default();
            let mut found = frames.feed(&bytes[..cut]);
            found.extend(frames.feed(&bytes[cut..]));
            assert_eq!(found.len(), 1, "cut at {cut}");
            assert_eq!(found[0].request_id, "f27f3b67938548a1af19d5c2d9707b64");
        }
        // A multi-byte character split across chunks survives in the record.
        let mut frames = ApprovalFrames::default();
        let frame = "event: approval.request\ndata: {\"run_id\": \"run_1\", \"request_id\": \"r1\", \"command\": \"echo héllo\"}\n\n";
        let bytes = frame.as_bytes();
        let split = frame.find('é').unwrap() + 1; // inside the two-byte é
        let mut found = frames.feed(&bytes[..split]);
        found.extend(frames.feed(&bytes[split..]));
        assert_eq!(found[0].command, "echo héllo");
    }

    #[tokio::test]
    async fn the_tap_passes_every_byte_through_unchanged() {
        // No manifest row behind it, so the spawned answer fails quietly;
        // what is under test is that the consumer reads the same bytes.
        let chunks = [
            "event: run.started\ndata: {\"run_id\": \"run_1\"}\n\n",
            "event: approval.request\ndata: {\"run_id\": \"run_1\", \"request_id\": \"r1\"}\n\n",
            "event: done\ndata: {}\n\n",
        ];
        let body: ByteStream = Box::pin(futures_util::stream::iter(
            chunks.map(|c| Ok::<Bytes, reqwest::Error>(Bytes::from(c))),
        ));
        let stream = ChatStream {
            status: 200,
            content_type: "text/event-stream".into(),
            body,
            canned: None,
        };
        let entry = ManifestEntry {
            model: "nobody".into(),
            url: "http://127.0.0.1:9".into(),
            key: None,
        };
        let tapped = refuse_approvals(stream, &entry, "nobody");
        let read: Vec<Bytes> = tapped.body.map(|c| c.unwrap()).collect().await;
        assert_eq!(read.concat(), chunks.concat().as_bytes());
    }
}
