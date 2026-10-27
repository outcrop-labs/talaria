// The lost turn: what a Hermes session needs after an agent restart dropped a
// turn on the floor, and what the session arm does about it.
//
// v0.22 runs every session's turns through a durable FIFO (its canonical
// session authority). A turn that was running when the container died
// non-gracefully (an OOM, `docker kill`, a host crash) is marked `unknown` on
// the next boot, and the session's FIFO stops behind it. Every later turn is
// admitted, then refused at once with an `error` frame naming
// `unknown_execution`, and its admission STAYS QUEUED. Hermes is waiting for
// a person to say what became of the lost turn. Nothing in Talaria said it,
// so a pinned conversation or a work session answered every later turn with
// that error, forever. All of this was reproduced on
// nousresearch/hermes-agent:v0.22.4.
//
// HERMES' HTTP DOOR FOR THIS DOES NOT OPEN FOR THESE TURNS.
// `POST /v1/runs/{id}/resolve-unknown` finds a run only through an owner held
// in memory or a `run_owner_scope` stored on the admission, and the session
// stream (`/api/sessions/{id}/chat/stream`) stores neither. After the restart
// that run answers 404. What Hermes documents for scripts is
// `hermes sessions discard <session> --yes`, the same fenced
// `prompt.resolve_unknown` its Desktop's Discard button sends. That is what
// the resolver behind RESOLVE_LOST_TURNS runs, over docker exec.
//
// The second half of this file is the RESUME's question. When Talaria lost
// its stream to a session turn while the container lived (a network reset, a
// frozen container that came back), the turn is still running or already
// finished in Hermes: a v0.22 session turn is shielded from its client
// hanging up. Re-sending the message would make the agent do the work twice.
// `reattach_session_run` picks the turn back up through the run door
// instead.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::Bytes;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use crate::fleet_chat::{
    ByteStream, CONTROL_WINDOW, ChatStream, agent_get, agent_post, control_failure, open_stream,
    turn_subject,
};
use talaria_fleet_layout::{ManifestEntry, read_manifest};

/// Acknowledge the turns one Hermes session lost across a non-graceful
/// restart, so its FIFO runs again: `(agent model, session id)` in, the
/// resolver's own account out. Wired by talaria-jobs to
/// talaria-fleet-docker's `discard_lost_turns`. When it is unset (a test
/// build), a lost turn stays the error it was.
pub type ResolveLostTurns =
    Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<String, String>> + Send + Sync>;
pub static RESOLVE_LOST_TURNS: OnceLock<ResolveLostTurns> = OnceLock::new();

/// The code Hermes refuses a turn with when the session's FIFO is paused
/// behind a lost one.
const LOST_TURN_CODE: &str = "unknown_execution";

/// The most the watch below holds while a frame is still arriving. A turn's
/// `run.started` echoes the whole message, attachments and all, so this is
/// generous; past it the stream passes through unwatched.
const WATCH_LIMIT: usize = 8 * 1024 * 1024;

/// How a refused turn is sent again: given the refused run's id (from its
/// `run.started`), withdraw it, acknowledge the lost turn, and open the turn
/// once more. A trait object so the stream watcher below can be exercised
/// without a container.
pub(crate) struct LostTurnRetry {
    session_id: String,
    retry: Box<dyn FnOnce(Option<String>) -> BoxFuture<'static, Result<ByteStream, String>> + Send>,
}

impl LostTurnRetry {
    /// The real retry, against the agent's own gateway with its own bearer.
    pub(crate) fn over_http(
        entry: &ManifestEntry,
        session_id: &str,
        path: String,
        body: Value,
    ) -> Self {
        let entry = ManifestEntry {
            model: entry.model.clone(),
            url: entry.url.clone(),
            key: entry.key.clone(),
        };
        let session = session_id.to_string();
        LostTurnRetry {
            session_id: session_id.to_string(),
            retry: Box::new(move |refused| {
                Box::pin(async move { send_again(entry, session, path, body, refused).await })
            }),
        }
    }
}

/// The repair, in the only order that does not run the turn twice.
///
/// 1. WITHDRAW the refused turn. Its admission is queued behind the lost one,
///    and the moment the FIFO moves it would run with nobody reading it. The
///    turn sent in step 3 is its replacement, so this one goes first, and if
///    it cannot be withdrawn nothing is sent again.
/// 2. ACKNOWLEDGE the lost turn. Hermes closes it with its own boundary row
///    ("This turn did not complete…"), so the agent knows the earlier turn
///    did not finish and its effects need checking.
/// 3. SEND the turn again, on the same session.
async fn send_again(
    entry: ManifestEntry,
    session_id: String,
    path: String,
    body: Value,
    refused: Option<String>,
) -> Result<ByteStream, String> {
    let run = refused
        .filter(|run| plain_id(run))
        .ok_or_else(|| "the refused turn named no run to withdraw".to_string())?;
    withdraw(&entry, &run).await?;
    let resolve = RESOLVE_LOST_TURNS
        .get()
        .ok_or_else(|| "nothing in this process can acknowledge a lost turn".to_string())?;
    let manifest = read_manifest().await;
    let agent = turn_subject(&manifest, &entry);
    let said = resolve(agent.clone(), session_id.clone()).await?;
    tracing::warn!(
        "[fleet] {agent}: session {session_id} had lost a turn in a restart — acknowledged \
         ({}); sending this turn again",
        said.trim()
    );
    match open_stream(agent_post(&entry, &path), &body).await {
        Some(stream) if stream.ok() => Ok(stream.body),
        Some(stream) => Err(format!(
            "the agent answered the turn sent again with HTTP {}",
            stream.status
        )),
        None => Err("the agent went away before the turn could be sent again".to_string()),
    }
}

/// Take a queued turn back out of the session's FIFO. Hermes answers a stop
/// on a still-queued admission by cancelling it (`status: "cancelled"`); any
/// other answer means the turn is not safely gone.
async fn withdraw(entry: &ManifestEntry, run: &str) -> Result<(), String> {
    let sent = tokio::time::timeout(
        CONTROL_WINDOW,
        agent_post(entry, &format!("/v1/runs/{run}/stop")).send(),
    )
    .await;
    let res = match sent {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => return Err(format!("withdrawing the refused turn failed: {e}")),
        Err(_) => return Err("the agent did not answer the withdrawal of the refused turn".into()),
    };
    if !res.status().is_success() {
        return Err(format!(
            "the refused turn could not be withdrawn ({})",
            control_failure(res).await
        ));
    }
    let body: Value = res.json().await.unwrap_or(Value::Null);
    match body["status"].as_str() {
        Some("cancelled") => Ok(()),
        other => Err(format!(
            "the refused turn could not be withdrawn (Hermes reports it {})",
            other.unwrap_or("in no state it names")
        )),
    }
}

/// An id Hermes minted (`run_<hex>`, a session key) that is safe to put in a
/// URL path: plain characters only, never empty, never a flag.
pub(crate) fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// What one frame at the head of a session stream says.
#[derive(Debug, PartialEq)]
enum Head {
    /// The run naming itself (with its id), its message opening, or a
    /// keep-alive: the turn has not started work yet.
    Opening(Option<String>),
    /// Hermes refusing the turn because the session lost an earlier one.
    LostTurn,
    /// Anything else: the turn is working, or ended some other way. There is
    /// nothing left to watch for.
    Work,
}

fn read_head(frame: &[u8]) -> Head {
    let text = String::from_utf8_lossy(frame);
    let mut event = "";
    let mut data: Vec<&str> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("event:") {
            event = name.trim();
        } else if let Some(d) = line.strip_prefix("data:") {
            data.push(d.trim());
        }
    }
    if event.is_empty() && data.is_empty() {
        return Head::Opening(None);
    }
    let json: Value = serde_json::from_str(&data.join("\n")).unwrap_or(Value::Null);
    match event {
        "run.started" => Head::Opening(json["run_id"].as_str().map(str::to_string)),
        "message.started" => Head::Opening(None),
        "error" if json["message"] == LOST_TURN_CODE => Head::LostTurn,
        _ => Head::Work,
    }
}

/// Watch a session stream's opening for the lost-turn refusal, and repair it
/// in place, once. Every frame passes on the moment it is whole, so a healthy
/// turn is not held back at all. The refusal arrives before any work frame
/// (Hermes answers it as soon as the FIFO pauses), so the watch ends at the
/// first frame that is neither the run's opening nor a keep-alive.
///
/// A repaired turn reaches the reader as one stream: the refused run's
/// `run.started` / `message.started`, then the new run's own. Readers stamp the
/// LAST run id they see, which is the live one a stop or a steer must name. If
/// the repair fails, the refusal is replaced by an error frame that says why,
/// and the turn ends as the error it was.
pub(crate) fn heal_lost_turn(mut stream: ChatStream, retry: LostTurnRetry) -> ChatStream {
    let inner = std::mem::replace(&mut stream.body, Box::pin(futures_util::stream::empty()));
    let watch = Watch {
        inner,
        head: Vec::new(),
        out: VecDeque::new(),
        run_id: None,
        retry: Some(retry),
        watching: true,
        ended: false,
    };
    stream.body = Box::pin(futures_util::stream::unfold(watch, Watch::next));
    stream
}

struct Watch {
    inner: ByteStream,
    /// Bytes not yet judged: the tail of a frame still arriving.
    head: Vec<u8>,
    out: VecDeque<reqwest::Result<Bytes>>,
    /// The id the current run named in its `run.started`.
    run_id: Option<String>,
    retry: Option<LostTurnRetry>,
    watching: bool,
    ended: bool,
}

impl Watch {
    async fn next(mut self) -> Option<(reqwest::Result<Bytes>, Self)> {
        loop {
            if let Some(item) = self.out.pop_front() {
                return Some((item, self));
            }
            if self.ended {
                return None;
            }
            let item = self.inner.next().await;
            if !self.watching {
                return item.map(|item| (item, self));
            }
            match item {
                Some(Ok(chunk)) => {
                    self.head.extend_from_slice(&chunk);
                    self.judge().await;
                }
                Some(Err(e)) => {
                    self.release();
                    self.out.push_back(Err(e));
                    self.ended = true;
                }
                None => {
                    self.release();
                    self.ended = true;
                }
            }
        }
    }

    /// Pass the unjudged bytes on as they came.
    fn release(&mut self) {
        if !self.head.is_empty() {
            self.out
                .push_back(Ok(Bytes::from(std::mem::take(&mut self.head))));
        }
    }

    async fn judge(&mut self) {
        while self.watching {
            let Some(end) = self.head.windows(2).position(|w| w == b"\n\n") else {
                // A frame that never ends is not a stream this watch
                // understands; it stops holding bytes back rather than
                // holding them forever.
                if self.head.len() > WATCH_LIMIT {
                    self.watching = false;
                    break;
                }
                return;
            };
            let frame: Vec<u8> = self.head.drain(..end + 2).collect();
            match read_head(&frame) {
                Head::Opening(run) => {
                    if run.is_some() {
                        self.run_id = run;
                    }
                    self.out.push_back(Ok(Bytes::from(frame)));
                }
                Head::Work => {
                    self.out.push_back(Ok(Bytes::from(frame)));
                    self.watching = false;
                }
                Head::LostTurn => {
                    self.watching = false;
                    let Some(retry) = self.retry.take() else {
                        self.out.push_back(Ok(Bytes::from(frame)));
                        break;
                    };
                    let session_id = retry.session_id.clone();
                    match (retry.retry)(self.run_id.clone()).await {
                        Ok(body) => {
                            // The refused stream's remainder (its `done`) goes
                            // with it: the reader carries on with the new run.
                            self.inner = body;
                            self.head.clear();
                            return;
                        }
                        Err(why) => {
                            tracing::warn!(
                                "[fleet] session {session_id} lost a turn in a restart and could \
                                 not be repaired: {why}"
                            );
                            self.out.push_back(Ok(lost_turn_frame(
                                self.run_id.as_deref(),
                                &session_id,
                                &why,
                            )));
                        }
                    }
                }
            }
        }
        self.release();
    }
}

/// The refusal, restated with the reason the repair failed. Session-stamped,
/// so every reader takes it as the session's own error frame.
fn lost_turn_frame(run_id: Option<&str>, session_id: &str, why: &str) -> Bytes {
    sse(
        "error",
        &json!({
            "message": format!(
                "{LOST_TURN_CODE}: this agent's session lost a turn in a restart, and it could \
                 not be cleared ({why})"
            ),
            "session_id": session_id,
            "run_id": run_id,
        }),
    )
}

fn sse(event: &str, data: &Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

// ── Picking a turn back up ───────────────────────────────────────────────────

/// How often a reattached turn asks the agent where it stands. Each answer
/// that says "still going" is a keep-alive to the reader, so the drain's
/// silence ceiling never fires on a turn that is demonstrably alive.
const REATTACH_POLL: Duration = Duration::from_secs(3);
/// Unanswered polls in a row before the reattach gives up: half a minute of
/// an agent that will not say anything.
const REATTACH_MISSES: u32 = 10;

const KEEPALIVE: &[u8] = b": keepalive\n\n";

/// A session turn whose stream Talaria lost, picked back up if the agent is
/// still holding it.
///
/// `GET /v1/runs/{id}` answers for a session-stream run while the container
/// that ran it is up, because that run's owner is held in memory. The
/// projection carries the run's status and, once it ends, its reply
/// (`output`). Running or finished, the run is what the resume should land:
/// re-sending the message would put it into the session a second time, and
/// the agent would do the work twice.
///
/// Any run the container still holds is picked up and landed the way its live
/// stream would have landed it: `completed` as the reply, `cancelled` as a
/// stopped turn, `failed` as an error row. A failed run is never sent again.
/// Hermes' `failed` also covers a turn that did real work and then stopped
/// short (`partial`, or the iteration budget), so a re-send would do that
/// work a second time, and a live `run.failed` ends the turn without a chain
/// for the same reason.
///
/// `None` means there is nothing to pick up, and the caller sends the turn
/// again. That happens in three cases: the run is unknown here (404: the
/// container restarted, so the turn died with it), the agent could not be
/// asked, or Hermes fenced the run as `unknown`. In that last case the
/// session's FIFO waits behind it, and the re-send's refusal is what the
/// repair above acts on. A turn lost in a restart is closed by Hermes' own
/// boundary first, through that repair.
///
/// The returned stream speaks the session frames every reader already knows:
/// `run.started` (so a stop can name the run), a keep-alive per poll while it
/// runs, then `assistant.completed` with the reply and the run's verdict.
/// Only the reply's text comes back. The tool calls and reasoning the lost
/// stream would have carried are not kept by the run door, and its `output`
/// is the reply before Hermes resolves `MEDIA:` tags, so an attachment lands
/// as its raw tag where a live turn would show it.
///
/// A RUN WAITING ON AN APPROVAL IS ANSWERED, as its live stream's would have
/// been. The projection lists the run's open prompts (`pending_controls`),
/// and each approval among them is raised once as the `approval.request`
/// frame the live stream carried, through the same tap the door puts on
/// every live turn (`fleet_approvals::tap_approvals`). It is refused within
/// a poll, or carded on an attended conversation's turn (`turn`), instead of
/// waiting out the agent's approval timeout with the session's FIFO held
/// behind it. `heard_at_ms` is when the lost stream last delivered anything
/// (epoch ms): a prompt its tap never saw was raised after that, and each
/// frame says how early it can have been raised (`ASKED_AFTER_KEY`), so a
/// card raised from it closes before Hermes' timeout does.
pub async fn reattach_session_run(
    agent_model: &str,
    run_id: &str,
    turn: crate::fleet_approvals::ApprovalTurn,
    heard_at_ms: i64,
) -> Option<ChatStream> {
    if !plain_id(run_id) {
        return None;
    }
    let manifest = read_manifest().await;
    let entry = manifest.iter().find(|m| m.model == agent_model)?;
    let first_sent_at = now_ms();
    let RunRead::Found(first) = read_run(entry, run_id).await else {
        return None;
    };
    if run_state(&first) == RunState::Lost {
        return None;
    }
    // No registration: the relay books this run's remaining calls itself,
    // under the session's own key, which the turn's first attempt already
    // attributed to this conversation (fleet_chat's `hold_for_agent`).
    let state = Reattached {
        entry: ManifestEntry {
            model: entry.model.clone(),
            url: entry.url.clone(),
            key: entry.key.clone(),
        },
        run_id: run_id.to_string(),
        session_id: first["session_id"].as_str().unwrap_or_default().to_string(),
        first: Some((first, first_sent_at)),
        raised: HashSet::new(),
        asked_after_ms: heard_at_ms,
        misses: 0,
        opened: false,
        done: false,
    };
    let stream = ChatStream {
        status: 200,
        content_type: "text/event-stream".into(),
        body: Box::pin(futures_util::stream::unfold(state, Reattached::next)),
        canned: None,
        hermes_session: None,
    };
    let subject = turn_subject(&manifest, entry);
    Some(crate::fleet_approvals::tap_approvals(
        stream, entry, &subject, turn,
    ))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The approvals a running projection is waiting on that this reattach has
/// not raised yet, as the `approval.request` frames the live stream carried:
/// `{run_id, session_id, request_id, execution_generation, command,
/// description, choices}`. Hermes lists them as `pending_controls` entries of
/// kind `approval` keyed by `prompt_id`, and the answer names that id as its
/// `request_id`. Each prompt is raised once, however many polls still list
/// it; clarify prompts are not approvals and are left alone.
///
/// The projection carries no time a prompt was raised, so each frame also
/// carries `asked_after_ms` under `ASKED_AFTER_KEY`: the last moment nothing
/// unseen was open (the lost stream's last bytes, then the previous read).
fn pending_approval_frames(
    projection: &Value,
    raised: &mut HashSet<String>,
    asked_after_ms: i64,
) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(pending) = projection["pending_controls"].as_array() else {
        return out;
    };
    for prompt in pending {
        if prompt["kind"].as_str() != Some("approval") {
            continue;
        }
        let Some(prompt_id) = prompt["prompt_id"].as_str().filter(|p| !p.is_empty()) else {
            continue;
        };
        if !raised.insert(prompt_id.to_string()) {
            continue;
        }
        let mut frame = json!({
            "run_id": projection["run_id"],
            "session_id": projection["session_id"],
            "request_id": prompt_id,
            "execution_generation": prompt["execution_generation"],
            "command": prompt["command"],
            "description": prompt["description"],
            "choices": prompt["choices"],
        });
        frame[crate::fleet_approvals::ASKED_AFTER_KEY] = json!(asked_after_ms);
        out.extend_from_slice(&sse("approval.request", &frame));
    }
    out
}

enum RunRead {
    Found(Value),
    /// 404: this container does not know the run.
    Gone,
    Unreachable,
}

async fn read_run(entry: &ManifestEntry, run_id: &str) -> RunRead {
    let sent = tokio::time::timeout(
        CONTROL_WINDOW,
        agent_get(entry, &format!("/v1/runs/{run_id}")).send(),
    )
    .await;
    match sent {
        Ok(Ok(res)) if res.status().is_success() => res
            .json::<Value>()
            .await
            .map(RunRead::Found)
            .unwrap_or(RunRead::Unreachable),
        Ok(Ok(res)) if res.status() == reqwest::StatusCode::NOT_FOUND => RunRead::Gone,
        _ => RunRead::Unreachable,
    }
}

/// Where a run stands, from its projection.
#[derive(Debug, PartialEq)]
enum RunState {
    /// Still going in the container.
    Running,
    /// Over: the frames that land it, as its live stream would have.
    Landed(Bytes),
    /// Fenced as `unknown`: Hermes does not know what became of it, and the
    /// session's FIFO waits behind it until the lost-turn repair runs.
    Lost,
}

fn run_state(projection: &Value) -> RunState {
    let stamp = json!({
        "session_id": projection["session_id"],
        "run_id": projection["run_id"],
    });
    let with = |extra: Value| {
        let mut data = stamp.clone();
        if let (Some(d), Value::Object(e)) = (data.as_object_mut(), extra) {
            d.extend(e);
        }
        data
    };
    let output = projection["output"].as_str().unwrap_or_default();
    let reply = sse("assistant.completed", &with(json!({ "content": output })));
    match projection["status"].as_str().unwrap_or_default() {
        "queued" | "running" | "waiting_for_approval" | "stopping" => RunState::Running,
        "unknown" => RunState::Lost,
        "completed" => RunState::Landed([reply, sse("run.completed", &stamp)].concat().into()),
        "cancelled" => RunState::Landed([reply, sse("run.cancelled", &stamp)].concat().into()),
        // The live stream's own ending for a failed run: Hermes' reply (an
        // apology, or the work it got through), then the verdict that makes
        // the row an error and keeps the chain from firing again.
        "failed" => RunState::Landed([reply, sse("run.failed", &stamp)].concat().into()),
        other => RunState::Landed(sse(
            "error",
            &with(json!({ "message": format!("the agent's run ended {other}") })),
        )),
    }
}

struct Reattached {
    entry: ManifestEntry,
    run_id: String,
    session_id: String,
    /// The projection the decision to reattach was made on, answered first,
    /// and when its read was sent (epoch ms).
    first: Option<(Value, i64)>,
    /// The approval prompts already raised as frames.
    raised: HashSet<String>,
    /// The earliest a prompt the next projection lists for the first time
    /// can have been raised: when the lost stream was last heard from, then
    /// when each read before it was sent (a prompt it did not list was
    /// raised after its snapshot).
    asked_after_ms: i64,
    misses: u32,
    opened: bool,
    done: bool,
}

impl Reattached {
    async fn next(mut self) -> Option<(reqwest::Result<Bytes>, Self)> {
        if self.done {
            return None;
        }
        if !self.opened {
            self.opened = true;
            let opening = sse(
                "run.started",
                &json!({ "run_id": self.run_id, "session_id": self.session_id }),
            );
            return Some((Ok(opening), self));
        }
        let (read, sent_at) = match self.first.take() {
            Some((first, sent_at)) => (RunRead::Found(first), sent_at),
            None => {
                tokio::time::sleep(REATTACH_POLL).await;
                let sent_at = now_ms();
                (read_run(&self.entry, &self.run_id).await, sent_at)
            }
        };
        let gone = |why: &str| {
            sse(
                "error",
                &json!({ "message": why, "run_id": self.run_id, "session_id": self.session_id }),
            )
        };
        let bytes = match read {
            RunRead::Found(projection) => {
                self.misses = 0;
                let asked_after = std::mem::replace(&mut self.asked_after_ms, sent_at);
                match run_state(&projection) {
                    RunState::Running => {
                        let asked =
                            pending_approval_frames(&projection, &mut self.raised, asked_after);
                        if asked.is_empty() {
                            Bytes::from_static(KEEPALIVE)
                        } else {
                            Bytes::from(asked)
                        }
                    }
                    RunState::Landed(frames) => {
                        self.done = true;
                        frames
                    }
                    // Fenced while it was being followed. The next turn's
                    // refusal is what the repair acts on.
                    RunState::Lost => {
                        self.done = true;
                        gone("the agent lost track of this turn before it finished")
                    }
                }
            }
            RunRead::Gone => {
                self.done = true;
                gone("the agent restarted before it finished this turn")
            }
            RunRead::Unreachable => {
                self.misses += 1;
                if self.misses >= REATTACH_MISSES {
                    self.done = true;
                    gone("the agent stopped answering about the turn it was finishing")
                } else {
                    Bytes::from_static(KEEPALIVE)
                }
            }
        };
        Some((Ok(bytes), self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet_chat::{AgentStreamEvent, AgentStreamParser};
    use std::sync::Mutex;

    fn frames(parts: &[&str]) -> ByteStream {
        let owned: Vec<reqwest::Result<Bytes>> = parts
            .iter()
            .map(|p| Ok(Bytes::from(p.to_string())))
            .collect();
        Box::pin(futures_util::stream::iter(owned))
    }

    fn chat(body: ByteStream) -> ChatStream {
        ChatStream {
            status: 200,
            content_type: "text/event-stream".into(),
            body,
            canned: None,
            hermes_session: None,
        }
    }

    async fn drain(stream: ChatStream) -> String {
        let chunks: Vec<_> = stream.body.collect().await;
        chunks
            .into_iter()
            .map(|c| String::from_utf8(c.expect("no transport error").to_vec()).unwrap())
            .collect()
    }

    /// A retry that records the run id it was handed and answers with
    /// `answer`.
    fn retry_with(
        answer: Result<Vec<&'static str>, &'static str>,
        seen: Arc<Mutex<Option<Option<String>>>>,
    ) -> LostTurnRetry {
        LostTurnRetry {
            session_id: "conv-1".into(),
            retry: Box::new(move |refused| {
                *seen.lock().unwrap() = Some(refused);
                Box::pin(async move {
                    match answer {
                        Ok(parts) => Ok(frames(&parts)),
                        Err(why) => Err(why.to_string()),
                    }
                })
            }),
        }
    }

    const STARTED: &str =
        "event: run.started\ndata: {\"run_id\": \"run_1\", \"session_id\": \"conv-1\"}\n\n";
    const OPENED: &str = "event: message.started\ndata: {\"message\": {\"id\": \"msg_1\"}, \"session_id\": \"conv-1\", \"run_id\": \"run_1\"}\n\n";
    const REFUSED: &str = "event: error\ndata: {\"message\": \"unknown_execution\", \"session_id\": \"conv-1\", \"run_id\": \"run_1\"}\n\n";
    const DONE: &str = "event: done\ndata: {\"session_id\": \"conv-1\", \"run_id\": \"run_1\"}\n\n";

    #[test]
    fn the_head_of_a_session_stream_reads_the_way_hermes_writes_it() {
        assert_eq!(
            read_head(STARTED.as_bytes()),
            Head::Opening(Some("run_1".into()))
        );
        assert_eq!(read_head(OPENED.as_bytes()), Head::Opening(None));
        assert_eq!(read_head(b": keepalive\n\n"), Head::Opening(None));
        assert_eq!(read_head(REFUSED.as_bytes()), Head::LostTurn);
        assert_eq!(
            read_head(b"event: error\ndata: {\"message\": \"provider said no\"}\n\n"),
            Head::Work
        );
        assert_eq!(
            read_head(b"event: assistant.delta\ndata: {\"delta\": \"hi\"}\n\n"),
            Head::Work
        );
    }

    /// The refused turn is withdrawn by the run id its own `run.started`
    /// named, and the reader gets one stream: the refused run's opening, then
    /// the new run, and never the refusal.
    #[tokio::test]
    async fn a_refused_turn_is_sent_again_in_place() {
        let seen = Arc::new(Mutex::new(None));
        let again = vec![
            "event: run.started\ndata: {\"run_id\": \"run_2\", \"session_id\": \"conv-1\"}\n\n",
            "event: assistant.delta\ndata: {\"delta\": \"hello again\", \"session_id\": \"conv-1\"}\n\n",
        ];
        let stream = heal_lost_turn(
            // The refusal split across chunks, and `done` riding behind it.
            chat(frames(&[
                STARTED,
                OPENED,
                &REFUSED[..20],
                &REFUSED[20..],
                DONE,
            ])),
            retry_with(Ok(again), seen.clone()),
        );
        let text = drain(stream).await;
        assert_eq!(*seen.lock().unwrap(), Some(Some("run_1".to_string())));
        assert!(!text.contains("unknown_execution"), "{text}");
        assert!(
            !text.contains("event: done"),
            "the refused stream's end is dropped: {text}"
        );
        let mut parser = AgentStreamParser::new();
        let events = parser.feed(text.as_bytes());
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::RunStarted {
                    run_id: "run_1".into()
                },
                AgentStreamEvent::RunStarted {
                    run_id: "run_2".into()
                },
                AgentStreamEvent::Content {
                    text: "hello again".into()
                },
            ]
        );
    }

    /// A repair that cannot be made leaves the turn the error it was, with the
    /// reason in it, and the refused stream ends as Hermes ended it.
    #[tokio::test]
    async fn a_repair_that_fails_says_why() {
        let seen = Arc::new(Mutex::new(None));
        let stream = heal_lost_turn(
            chat(frames(&[STARTED, OPENED, REFUSED, DONE])),
            retry_with(
                Err("the refused turn could not be withdrawn (409 not_found)"),
                seen,
            ),
        );
        let text = drain(stream).await;
        let events = AgentStreamParser::new().feed(text.as_bytes());
        assert_eq!(
            events,
            vec![
                AgentStreamEvent::RunStarted {
                    run_id: "run_1".into()
                },
                AgentStreamEvent::Error {
                    message: "unknown_execution: this agent's session lost a turn in a restart, \
                              and it could not be cleared (the refused turn could not be \
                              withdrawn (409 not_found))"
                        .into()
                },
            ]
        );
        assert!(text.ends_with(DONE), "{text}");
    }

    /// A healthy turn passes through byte for byte, however it is chunked, and
    /// the retry is never touched.
    #[tokio::test]
    async fn a_healthy_turn_passes_untouched() {
        let seen = Arc::new(Mutex::new(None));
        let delta =
            "event: assistant.delta\ndata: {\"delta\": \"hi\", \"session_id\": \"conv-1\"}\n\n";
        let error = "event: error\ndata: {\"message\": \"unknown_execution\", \"session_id\": \"conv-1\"}\n\n";
        let parts = [
            &STARTED[..7],
            &STARTED[7..],
            OPENED,
            ": keepalive\n\n",
            delta,
            error,
            DONE,
        ];
        let stream = heal_lost_turn(chat(frames(&parts)), retry_with(Ok(vec![]), seen.clone()));
        assert_eq!(drain(stream).await, parts.concat());
        assert_eq!(
            *seen.lock().unwrap(),
            None,
            "a refusal after work began is not a lost turn"
        );
    }

    #[test]
    fn only_plain_ids_reach_a_path() {
        assert!(plain_id("run_2f8c74166c6e469297219bdf64bfb1b5"));
        assert!(plain_id("f48a3937-c96c-4907-b8b7-2618a91d1a1e"));
        for bad in ["", "-rf", "run/../x", "run 1", "a?b", &"x".repeat(129)] {
            assert!(!plain_id(bad), "{bad}");
        }
    }

    fn landed(bytes: &Bytes) -> Vec<AgentStreamEvent> {
        AgentStreamParser::new().feed(bytes)
    }

    #[test]
    fn a_run_projection_lands_the_way_a_live_turn_would() {
        let base = |status: &str| {
            json!({ "run_id": "run_1", "session_id": "conv-1", "status": status,
                    "output": "the reply" })
        };
        for status in ["queued", "running", "waiting_for_approval", "stopping"] {
            assert_eq!(run_state(&base(status)), RunState::Running, "{status}");
        }
        let RunState::Landed(done) = run_state(&base("completed")) else {
            panic!("completed lands");
        };
        assert_eq!(
            landed(&done),
            vec![AgentStreamEvent::Content {
                text: "the reply".into()
            }]
        );
        let RunState::Landed(stopped) = run_state(&base("cancelled")) else {
            panic!("cancelled lands as a stopped turn");
        };
        assert_eq!(
            landed(&stopped),
            vec![
                AgentStreamEvent::Content {
                    text: "the reply".into()
                },
                AgentStreamEvent::Stopped
            ]
        );
        // A failed run lands as the error row its live stream would have
        // made, reply first: never sent again, never chained.
        let RunState::Landed(failed) = run_state(&base("failed")) else {
            panic!("failed lands as an error, not a re-send");
        };
        assert_eq!(
            landed(&failed),
            vec![
                AgentStreamEvent::Content {
                    text: "the reply".into()
                },
                AgentStreamEvent::Error {
                    message: "the run failed".into()
                }
            ]
        );
        // Only a run Hermes fenced as unknown is handed back for a re-send,
        // whose refusal the lost-turn repair acts on.
        assert_eq!(run_state(&base("unknown")), RunState::Lost);
        let RunState::Landed(odd) = run_state(&base("steered")) else {
            panic!("a status this build does not know still lands");
        };
        assert_eq!(
            landed(&odd),
            vec![AgentStreamEvent::Error {
                message: "the agent's run ended steered".into()
            }]
        );
    }

    /// `GET /v1/runs/{id}` for a session-stream run parked on an approval,
    /// in the shape v0.22.4's `run_projection` answers: `pending_controls`
    /// is the session's `PendingControls.snapshot`, one entry per open
    /// prompt, and a clarify prompt can sit beside an approval.
    fn parked() -> Value {
        json!({
            "pending_controls": [
                { "kind": "clarify", "prompt_id": "c-1", "execution_generation": 3,
                  "question": "which one?", "choices": ["a", "b"], "multi_select": false },
                { "kind": "approval", "prompt_id": "44e98bc6bc544867860b37c800ac72cc",
                  "execution_generation": 3, "command": "<write to AGENTS.md>",
                  "description": "Write to protected agent-instruction file(s): AGENTS.md.",
                  "choices": ["once", "deny"] }
            ],
            "run_id": "run_d87c07107c4244a89973e5bd2618264b",
            "status": "waiting_for_approval",
            "session_id": "6113b5bc-6935-46e1-a67c-60bd58daf4f0",
            "admission_id": "adm-1",
            "execution_generation": 3,
            "output": "",
            "usage": {}
        })
    }

    #[test]
    fn a_parked_run_raises_each_approval_once_across_polls() {
        let mut raised = HashSet::new();
        let first = pending_approval_frames(&parked(), &mut raised, 1_792_699_000_000);
        let text = String::from_utf8(first.clone()).unwrap();
        // One frame, for the approval and not the clarify prompt.
        assert_eq!(text.matches("event: approval.request").count(), 1, "{text}");
        let asked = crate::fleet_approvals::approval_asked(text.trim_end()).expect("a request");
        assert_eq!(asked.run_id, "run_d87c07107c4244a89973e5bd2618264b");
        assert_eq!(asked.request_id, "44e98bc6bc544867860b37c800ac72cc");
        assert_eq!(asked.execution_generation, Some(3));
        assert_eq!(asked.command, "<write to AGENTS.md>");
        assert_eq!(asked.choices, ["once", "deny"]);
        // The projection says nothing of when it was asked: the frame says
        // how early it can have been, for the card's clock.
        assert_eq!(asked.asked_after_ms, Some(1_792_699_000_000));
        // The answer names the prompt id as its request id, in the generation
        // the run is in.
        assert_eq!(
            crate::fleet_approvals::answer_body(&asked, crate::fleet_approvals::Choice::Deny),
            json!({ "request_id": "44e98bc6bc544867860b37c800ac72cc",
                    "execution_generation": 3, "choice": "deny" })
        );
        // The next poll lists the same prompt: nothing new is raised.
        assert!(pending_approval_frames(&parked(), &mut raised, 1_792_699_003_000).is_empty());
        // A reader lands nothing for the frame.
        assert!(landed(&Bytes::from(first)).is_empty());
        // A running projection with nothing open raises nothing.
        let mut quiet = parked();
        quiet["pending_controls"] = json!([]);
        assert!(pending_approval_frames(&quiet, &mut HashSet::new(), 0).is_empty());
    }

    #[tokio::test]
    async fn the_reattached_stream_s_approval_reaches_the_tap_and_is_refused() {
        use axum::extract::{Json, Path};
        // The agent's door: records each answer it is sent.
        let answers: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let seen = answers.clone();
        let app = axum::Router::new().route(
            "/v1/runs/{run}/approval",
            axum::routing::post(move |Path(run): Path<String>, Json(body): Json<Value>| {
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push((run, body));
                    Json(json!({ "status": "resolved" }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let entry = ManifestEntry {
            model: "dex".into(),
            url,
            key: None,
        };
        let projection = parked();
        let state = Reattached {
            entry: ManifestEntry {
                model: entry.model.clone(),
                url: entry.url.clone(),
                key: None,
            },
            run_id: projection["run_id"].as_str().unwrap().into(),
            session_id: projection["session_id"].as_str().unwrap().into(),
            first: Some((projection, 1_792_699_015_000)),
            raised: HashSet::new(),
            asked_after_ms: 1_792_699_000_000,
            misses: 0,
            opened: false,
            done: false,
        };
        let stream = chat(Box::pin(futures_util::stream::unfold(
            state,
            Reattached::next,
        )));
        let mut tapped = crate::fleet_approvals::tap_approvals(
            stream,
            &entry,
            "dex",
            crate::fleet_approvals::ApprovalTurn::default(),
        );
        // `run.started`, then the parked projection's approval frame.
        let opening = tapped.body.next().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&opening).contains("run.started"));
        let asked = tapped.body.next().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&asked).contains("event: approval.request"));
        // The tap answered it, off to the side, with the prompt's own id.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while answers.lock().unwrap().is_empty() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            answers.lock().unwrap().clone(),
            vec![(
                "run_d87c07107c4244a89973e5bd2618264b".to_string(),
                json!({ "request_id": "44e98bc6bc544867860b37c800ac72cc",
                        "execution_generation": 3, "choice": "deny" })
            )]
        );
    }

    #[tokio::test]
    async fn each_frame_says_how_early_its_prompt_can_have_been_raised() {
        use axum::extract::Path;
        // The agent's run door: the second read lists a prompt the first did
        // not, so it was raised after the first read was sent.
        let app = axum::Router::new().route(
            "/v1/runs/{run}",
            axum::routing::get(move |Path(_run): Path<String>| async move {
                let mut later = parked();
                later["pending_controls"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({ "kind": "approval", "prompt_id": "p-2",
                                  "execution_generation": 3, "command": "<write to CLAUDE.md>",
                                  "description": "Write to protected agent-instruction file(s): CLAUDE.md.",
                                  "choices": ["once", "deny"] }));
                axum::Json(later)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let projection = parked();
        let state = Reattached {
            entry: ManifestEntry {
                model: "dex".into(),
                url,
                key: None,
            },
            run_id: projection["run_id"].as_str().unwrap().into(),
            session_id: projection["session_id"].as_str().unwrap().into(),
            first: Some((projection, 1_792_699_015_000)),
            raised: HashSet::new(),
            asked_after_ms: 1_792_699_000_000,
            misses: 0,
            opened: false,
            done: false,
        };
        let mut body: ByteStream = Box::pin(futures_util::stream::unfold(state, Reattached::next));
        async fn next_asked(
            body: &mut ByteStream,
        ) -> Option<crate::fleet_approvals::ApprovalAsked> {
            let chunk = body.next().await.unwrap().unwrap();
            crate::fleet_approvals::approval_asked(String::from_utf8_lossy(&chunk).trim_end())
        }
        assert_eq!(next_asked(&mut body).await, None); // run.started
        // The prompt the reattach found open was raised after the lost
        // stream was last heard from; no later moment can be assumed.
        let found = next_asked(&mut body).await.expect("the parked prompt");
        assert_eq!(found.request_id, "44e98bc6bc544867860b37c800ac72cc");
        assert_eq!(found.asked_after_ms, Some(1_792_699_000_000));
        // The prompt a later poll found first was raised after the read
        // before it was sent. The one already raised is not raised again.
        let later = next_asked(&mut body).await.expect("the new prompt");
        assert_eq!(later.request_id, "p-2");
        assert_eq!(later.asked_after_ms, Some(1_792_699_015_000));
    }
}
