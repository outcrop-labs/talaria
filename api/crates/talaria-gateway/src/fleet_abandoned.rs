// A completions turn whose reader lets go of it before it ends is stopped in
// the agent.
//
// WHY. Dropping the stream does not end the turn in Hermes v0.22. An admitted
// `/v1/chat/completions` turn outlives its request: the turn's observe is
// shielded, and an admission committed while its request was cancelled still
// has its drain run (`observe_api_turn`, `schedule_orphan` in
// gateway/session_api_turn.py). So when Talaria lets go of a turn early — a
// harness run past its deadline (the inbox brief's), an app turn whose caller
// hung up — a turn still QUEUED is not withdrawn. Behind a running turn it
// runs later, for nobody; behind a turn lost in a restart (a wedged derived
// session, see fleet_lost_turns.rs) it stays parked there for good. A turn
// already running is interrupted only when Hermes next fails a write to the
// closed socket, which on a quiet turn is the next keep-alive (10 s).
//
// WHAT. Every chunk of a completions stream names its completion id
// (`chatcmpl-…`, the chunk's `id`). While the request is open that id is also
// the turn's run id: the stream's approval registration binds it to the
// admission (`_run_admission_aliases`, `run_api_turn`), and
// `POST /v1/runs/{id}/stop` cancels a queued admission (`cancel_queued`) or
// interrupts a started one (`stop_run` in api_server_authority_runs.py). The
// binding is dropped when the request ends, so the order matters: the request
// is held open while the stop is sent, and closed only once Hermes has ended
// the turn's stream (or the windows below run out).
//
// So the stream a reader lets go of is not closed there. A detached task
// takes it over and reads it on, to do three things (each read-on is bounded:
// the control window to name the turn and, once a stop lands, for the stream
// to end; the headers window while Hermes cannot place the stop):
// - Name the turn. A reader that never read a frame (an opening cancelled
//   between the headers and the first read — an app caller hanging up while
//   the turn's key is recorded) left no id; the role chunk Hermes has
//   already written names it.
// - Place the stop. Hermes writes the role chunk BEFORE the turn's admission
//   commits and its id is bound (the turn is spawned, then the writer
//   starts; admissions run one at a time per agent, each write in a worker
//   thread: `run_steps_off_loop`), so a stop sent at once can answer
//   `not_found`. It is sent again, backing off, while the stream is open,
//   until it lands or the turn ends. Usually the gap is milliseconds. On a
//   starved agent store it is not: on v0.22.4 under IO starvation the id was
//   still unbound minutes after its role chunk. So this phase gets the
//   patience Talaria gives the agent's headers (HEADERS_WINDOW, five
//   minutes), not the control window.
// - Follow a move. A turn the lost-turn watch moved to a fresh session names
//   the fresh completion in its next role chunk, and that is the one stopped,
//   also when the move was still waiting for its headers as the reader left.
// And no move starts after the reader leaves: the guard marks the turn let
// go of ([`LetGo`]), and the move refuses a turn so marked (fleet_chat's
// FreshThread), so a refusal read on here is not sent again for nobody.
//
// WHEN. A stream let go of before it ended: before `data: [DONE]` passed and
// before the body ran out. A stream that broke (a read error) has no request
// left for an id to name, and one that never names a completion inside the
// control window has nothing to stop.
//
// WHAT IS NOT COVERED. A turn let go of before its response headers arrived
// has no stream to read: the request is dropped with nothing naming the turn,
// and what Hermes does with a request that disconnects mid-admission is its
// own (the upstream issue in #610: a queued admission is not withdrawn). A
// stop Hermes still cannot place when the headers window runs out is given
// up, said at warn, and the stream closed. A stream dropped outside the
// runtime, or as it shuts down, just closes.
//
// The session arm is not covered here. Its reader names the run from
// `run.started` and stops it by that id itself (talaria-app-agents).

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use serde_json::Value;
use talaria_fleet_layout::ManifestEntry;
use tokio::time::Instant;

use crate::fleet_chat::{ByteStream, CONTROL_WINDOW, ChatStream, HEADERS_WINDOW, agent_post};

/// The longest frame read for its id. A chunk is a few hundred bytes; a
/// frame past this is not one the guard understands, so it stops reading
/// frames (the bytes still pass) rather than hold them.
const FRAME_LIMIT: usize = 64 * 1024;

/// How long the guard reads a let-go turn on (see the header).
#[derive(Clone, Copy, Debug)]
struct Windows {
    /// To see the turn named, and, once a stop landed (or was refused), for
    /// Hermes to end the turn's stream. Each stop call is bounded by it too.
    control: Duration,
    /// While Hermes cannot place the stop yet: its id is not bound.
    place: Duration,
    /// The first resend of a stop Hermes could not place, doubling up to
    /// `retry_max`.
    retry: Duration,
    retry_max: Duration,
}

const LIVE: Windows = Windows {
    control: CONTROL_WINDOW,
    place: HEADERS_WINDOW,
    retry: Duration::from_millis(250),
    retry_max: Duration::from_secs(5),
};

/// Whether a completions turn's reader let go of it. Shared by the guard,
/// which marks it, and the lost-turn move, which then refuses to send the
/// turn again (nobody would read the fresh one).
#[derive(Clone, Default)]
pub(crate) struct LetGo(Arc<AtomicBool>);

impl LetGo {
    pub(crate) fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// The reader let go of this turn before it ended.
    pub(crate) fn happened(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Wrap a completions turn's stream so that letting go of it early stops the
/// turn in the agent (see the header). `let_go` is marked when that happens.
pub(crate) fn stop_when_abandoned(
    stream: ChatStream,
    entry: &ManifestEntry,
    let_go: LetGo,
) -> ChatStream {
    guard(stream, entry, let_go, LIVE)
}

fn guard(
    mut stream: ChatStream,
    entry: &ManifestEntry,
    let_go: LetGo,
    windows: Windows,
) -> ChatStream {
    let inner = std::mem::replace(&mut stream.body, Box::pin(futures_util::stream::empty()));
    stream.body = Box::pin(Guarded {
        inner: Some(inner),
        entry: ManifestEntry {
            model: entry.model.clone(),
            url: entry.url.clone(),
            key: entry.key.clone(),
        },
        frames: Frames::default(),
        let_go,
        windows,
    });
    stream
}

struct Guarded {
    inner: Option<ByteStream>,
    entry: ManifestEntry,
    frames: Frames,
    let_go: LetGo,
    windows: Windows,
}

/// What the frames read so far say about the turn.
struct Frames {
    /// The completion the stream named last.
    run: Option<String>,
    /// The stream ran out, broke, or said `[DONE]`: nothing is left to stop.
    ended: bool,
    /// Bytes of a frame still arriving.
    head: Vec<u8>,
    reading: bool,
}

impl Default for Frames {
    fn default() -> Self {
        Frames {
            run: None,
            ended: false,
            head: Vec::new(),
            reading: true,
        }
    }
}

impl Frames {
    fn note(&mut self, chunk: &[u8]) {
        if !self.reading {
            return;
        }
        self.head.extend_from_slice(chunk);
        while let Some(end) = self.head.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = self.head.drain(..end + 2).collect();
            match completion_of(&frame) {
                Frame::Named(id) => self.run = Some(id),
                Frame::Done => self.ended = true,
                Frame::Other => {}
            }
        }
        if self.head.len() > FRAME_LIMIT {
            self.reading = false;
            self.head = Vec::new();
        }
    }
}

#[derive(Debug, PartialEq)]
enum Frame {
    Named(String),
    Done,
    Other,
}

/// What one SSE frame says: the completion it belongs to (the chunk's own
/// top-level `id`, never an id quoted in the reply's words), the end of the
/// stream, or nothing this guard reads.
fn completion_of(frame: &[u8]) -> Frame {
    let text = String::from_utf8_lossy(frame);
    let data: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .collect();
    if data.is_empty() {
        return Frame::Other;
    }
    let data = data.join("\n");
    if data == "[DONE]" {
        return Frame::Done;
    }
    if !data.contains("chatcmpl-") {
        return Frame::Other;
    }
    match serde_json::from_str::<Value>(&data) {
        Ok(json) => match json.get("id").and_then(Value::as_str) {
            Some(id) if id.starts_with("chatcmpl-") && id.len() > "chatcmpl-".len() => {
                Frame::Named(id.to_string())
            }
            _ => Frame::Other,
        },
        Err(_) => Frame::Other,
    }
}

impl Stream for Guarded {
    type Item = reqwest::Result<axum::body::Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let Some(inner) = this.inner.as_mut() else {
            return Poll::Ready(None);
        };
        match inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                this.frames.note(&chunk);
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(e))) => {
                this.frames.ended = true;
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                this.frames.ended = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        if self.frames.ended {
            return;
        }
        let Some(inner) = self.inner.take() else {
            return;
        };
        self.let_go.mark();
        // Outside the runtime (a shutdown) the stream just closes; Hermes ends
        // a started turn at its next write.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let entry = ManifestEntry {
            model: self.entry.model.clone(),
            url: self.entry.url.clone(),
            key: self.entry.key.clone(),
        };
        handle.spawn(settle(
            entry,
            inner,
            std::mem::take(&mut self.frames),
            self.windows,
        ));
    }
}

/// The stop a reader let go of, after the fact (see the header): read the
/// turn's stream on, stop the newest completion it names until a stop lands,
/// and close the stream once Hermes ends it.
async fn settle(entry: ManifestEntry, mut inner: ByteStream, mut frames: Frames, w: Windows) {
    let start = Instant::now();
    // The read-on bound for the phase the stop is in: naming the turn, a stop
    // Hermes could not place yet, or the end after a stop landed.
    let mut until = start + w.control;
    // The completion a stop landed on, or that one cannot be stopped.
    let mut settled: Option<String> = None;
    // A stop Hermes could not place yet: which completion, when to send it
    // again, the wait before that, and what Hermes answered.
    let mut unplaced: Option<(String, Instant, Duration, String)> = None;
    loop {
        if let Some(run) = frames.run.clone()
            && settled.as_ref() != Some(&run)
            && unplaced
                .as_ref()
                .is_none_or(|(r, at, _, _)| *r != run || Instant::now() >= *at)
        {
            match stop(&entry, &run, Instant::now() + w.control).await {
                Stop::Landed(status) => {
                    tracing::info!(
                        "[fleet] {}: a turn its reader let go of was stopped in the agent ({run}: {status})",
                        entry.model
                    );
                    settled = Some(run);
                    unplaced = None;
                    until = Instant::now() + w.control;
                }
                Stop::NotYet(why) => {
                    let wait = match &unplaced {
                        Some((r, _, wait, _)) if *r == run => (*wait * 2).min(w.retry_max),
                        _ => w.retry,
                    };
                    unplaced = Some((run, Instant::now() + wait, wait, why));
                    until = start + w.place;
                }
                // Most often the turn had just finished and Hermes forgot it.
                Stop::Refused(why) => {
                    tracing::debug!(
                        "[fleet] {}: stopping {run}, which its reader let go of, answered {why}",
                        entry.model
                    );
                    settled = Some(run);
                    unplaced = None;
                    until = Instant::now() + w.control;
                }
            }
        }
        if frames.ended || Instant::now() >= until {
            break;
        }
        let wake = unplaced
            .as_ref()
            .map_or(until, |(_, at, _, _)| (*at).min(until));
        match tokio::time::timeout_at(wake, inner.next()).await {
            Ok(Some(Ok(chunk))) => frames.note(&chunk),
            Ok(Some(Err(_)) | None) => frames.ended = true,
            // A stop is due again, or the window is out.
            Err(_) => {}
        }
    }
    if let Some((run, _, _, why)) = unplaced
        && !frames.ended
    {
        tracing::warn!(
            "[fleet] {}: {run}, which its reader let go of, could not be stopped: Hermes still could not place it after {} s ({why}); it is closed unstopped",
            entry.model,
            w.place.as_secs()
        );
    }
    drop(inner);
}

/// What the agent answered a stop.
enum Stop {
    /// The run's status as Hermes answers it.
    Landed(String),
    /// Hermes cannot place the run yet: its id is not bound to an admission
    /// (409 `not_found`) or its store was busy (503 `storage_unavailable`).
    /// Sent again while the stream is open.
    NotYet(String),
    /// Anything else, sent once.
    Refused(String),
}

/// `POST /v1/runs/{run}/stop` on this agent, answered before `deadline`.
async fn stop(entry: &ManifestEntry, run: &str, deadline: Instant) -> Stop {
    let asked = tokio::time::timeout_at(deadline, async {
        let res = match agent_post(entry, &format!("/v1/runs/{run}/stop"))
            .send()
            .await
        {
            Ok(res) => res,
            Err(e) => return Stop::Refused(e.without_url().to_string()),
        };
        let status = res.status().as_u16();
        let body = res.json::<Value>().await.ok();
        if (200..300).contains(&status) {
            return Stop::Landed(
                body.as_ref()
                    .and_then(|b| b["status"].as_str())
                    .unwrap_or("stopped")
                    .to_string(),
            );
        }
        // The code only, as control_failure does: the prose never travels.
        let code = body
            .as_ref()
            .and_then(|b| b["error"]["code"].as_str())
            .filter(|c| !c.is_empty())
            .map(str::to_string);
        let why = match &code {
            Some(code) => format!("{status} {code}"),
            None => status.to_string(),
        };
        match (status, code.as_deref()) {
            (409, Some("not_found")) | (503, Some("storage_unavailable")) => Stop::NotYet(why),
            _ => Stop::Refused(why),
        }
    })
    .await;
    asked.unwrap_or_else(|_| Stop::Refused("no answer inside the control window".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use tokio::sync::mpsc;

    type Item = <ByteStream as Stream>::Item;

    fn chunk(s: &str) -> Item {
        Ok(s.as_bytes().to_vec().into())
    }

    fn role(id: &str) -> String {
        format!(
            "data: {{\"id\": \"{id}\", \"object\": \"chat.completion.chunk\", \"choices\": [{{\"index\": 0, \"delta\": {{\"role\": \"assistant\"}}, \"finish_reason\": null}}]}}\n\n"
        )
    }

    fn words(id: &str, text: &str) -> String {
        format!(
            "data: {{\"id\": \"{id}\", \"object\": \"chat.completion.chunk\", \"choices\": [{{\"index\": 0, \"delta\": {{\"content\": \"{text}\"}}, \"finish_reason\": null}}]}}\n\n"
        )
    }

    /// The end Hermes writes once a stopped turn settles: the finish chunk,
    /// then `[DONE]`.
    fn finish(id: &str) -> String {
        format!(
            "data: {{\"id\": \"{id}\", \"choices\": [{{\"index\": 0, \"delta\": {{}}, \"finish_reason\": \"stop\"}}]}}\n\ndata: [DONE]\n\n"
        )
    }

    const KEEPALIVE: &str = ": keepalive\n\n";

    /// One stop the stand-in agent was sent: the run, the bearer, whether
    /// the turn's stream was still open when it arrived, and what it
    /// answered.
    #[derive(Clone, Debug, PartialEq)]
    struct Seen {
        run: String,
        bearer: Option<String>,
        open: bool,
        answered: u16,
    }

    /// The turn's stream, as the stand-in agent drives it: the test's
    /// scripted chunks first, then whatever arrives on `tail` (a stop that
    /// lands sends the finish, then ends it). Dropping it sets `closed`,
    /// which is the request closing.
    struct Turn {
        chunks: std::collections::VecDeque<Item>,
        tail: mpsc::UnboundedReceiver<Item>,
        closed: Arc<AtomicBool>,
    }

    impl Stream for Turn {
        type Item = Item;
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Item>> {
            let this = self.get_mut();
            match this.chunks.pop_front() {
                Some(c) => Poll::Ready(Some(c)),
                None => this.tail.poll_recv(cx),
            }
        }
    }

    impl Drop for Turn {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    /// The stand-in agent and the one turn it holds open.
    struct Agent {
        entry: ManifestEntry,
        stops: Arc<Mutex<Vec<Seen>>>,
        closed: Arc<AtomicBool>,
        /// The turn's tail: what the agent writes after the test's chunks.
        tail: Arc<Mutex<Option<mpsc::UnboundedSender<Item>>>>,
        turn: Option<Turn>,
    }

    /// The agent's `/v1/runs/{id}/stop`, answering as Hermes does: 409
    /// `not_found` for a run in `unbound` (its id was never bound, or its
    /// request ended) and for each of the first `not_yet` stops (the role
    /// chunk came before the admission bound the id); otherwise the queued
    /// admission is cancelled, and the turn's stream gets its finish and
    /// ends.
    async fn agent(unbound: &[&str], not_yet: usize) -> Agent {
        let stops: Arc<Mutex<Vec<Seen>>> = Default::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::unbounded_channel();
        let tail = Arc::new(Mutex::new(Some(tx)));
        let unbound: Vec<String> = unbound.iter().map(|s| s.to_string()).collect();
        let (kept, open, ender) = (stops.clone(), closed.clone(), tail.clone());
        let app = axum::Router::new().route(
            "/v1/runs/{run}/stop",
            axum::routing::post(
                move |axum::extract::Path(run): axum::extract::Path<String>,
                      headers: axum::http::HeaderMap| {
                    let (kept, open, ender) = (kept.clone(), open.clone(), ender.clone());
                    let unbound = unbound.clone();
                    async move {
                        let bearer = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string);
                        let mut stops = kept.lock().unwrap();
                        let placed = !unbound.contains(&run) && stops.len() >= not_yet;
                        let answered = if placed { 200 } else { 409 };
                        stops.push(Seen {
                            run: run.clone(),
                            bearer,
                            open: !open.load(Ordering::SeqCst),
                            answered,
                        });
                        if !placed {
                            return (
                                axum::http::StatusCode::CONFLICT,
                                axum::Json(serde_json::json!({
                                    "error": { "message": "not_found", "code": "not_found" }
                                })),
                            );
                        }
                        if let Some(tx) = ender.lock().unwrap().take() {
                            let _ = tx.send(chunk(&finish(&run)));
                        }
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({ "run_id": run, "status": "cancelled" })),
                        )
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let turn = Turn {
            chunks: Default::default(),
            tail: rx,
            closed: closed.clone(),
        };
        Agent {
            entry: ManifestEntry {
                model: "ada-ops".into(),
                url,
                key: Some("agent-door-key".into()),
            },
            stops,
            closed,
            tail,
            turn: Some(turn),
        }
    }

    /// The live windows, shortened so the tests run in seconds.
    const QUICK: Windows = Windows {
        control: Duration::from_millis(800),
        place: Duration::from_secs(4),
        retry: Duration::from_millis(50),
        retry_max: Duration::from_millis(400),
    };

    impl Agent {
        /// The turn's stream, guarded: `chunks` first, then the agent's
        /// tail. `ends` closes the tail at once (a turn whose body runs out).
        fn guarded(&mut self, chunks: Vec<Item>, ends: bool) -> (ByteStream, LetGo) {
            self.guarded_with(chunks, ends, QUICK)
        }

        fn guarded_with(
            &mut self,
            chunks: Vec<Item>,
            ends: bool,
            windows: Windows,
        ) -> (ByteStream, LetGo) {
            let mut turn = self.turn.take().expect("one turn per agent");
            turn.chunks = chunks.into();
            if ends {
                self.tail.lock().unwrap().take();
            }
            let let_go = LetGo::default();
            let stream = ChatStream {
                status: 200,
                content_type: "text/event-stream".into(),
                body: Box::pin(turn),
                canned: None,
                hermes_session: None,
            };
            (
                guard(stream, &self.entry, let_go.clone(), windows).body,
                let_go,
            )
        }

        /// Write `item` on the turn after `after` (a frame Hermes sends late).
        fn later(&self, after: Duration, item: String) {
            let tx = self.tail.lock().unwrap().clone().expect("the turn is open");
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                let _ = tx.send(chunk(&item));
            });
        }

        /// The stops sent, once they stopped arriving.
        async fn settled(&self) -> Vec<Seen> {
            for _ in 0..300 {
                if self.stops.lock().unwrap().iter().any(|s| s.answered == 200) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // Long enough for a second, wrong stop to have arrived too.
            tokio::time::sleep(Duration::from_millis(300)).await;
            self.stops.lock().unwrap().clone()
        }

        fn closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }
    }

    fn landed(run: &str) -> Seen {
        Seen {
            run: run.into(),
            bearer: Some("Bearer agent-door-key".into()),
            open: true,
            answered: 200,
        }
    }

    /// Read `n` chunks, then let go.
    async fn read_then_drop(mut body: ByteStream, n: usize) {
        for _ in 0..n {
            body.next().await.expect("a chunk").expect("not an error");
        }
        drop(body);
    }

    /// A turn let go of mid-stream is stopped by the completion id its chunks
    /// named, with the agent's own bearer, while the turn's stream is still
    /// open (Hermes knows the id only while the request lives). The stream
    /// closes once the agent ended it, and the turn is marked let go of.
    #[tokio::test]
    async fn a_turn_let_go_of_is_stopped_while_its_stream_is_still_open() {
        let mut agent = agent(&[], 0).await;
        let (body, let_go) =
            agent.guarded(vec![chunk(&role("chatcmpl-a1")), chunk(KEEPALIVE)], false);
        read_then_drop(body, 2).await;
        assert!(let_go.happened());
        assert_eq!(agent.settled().await, vec![landed("chatcmpl-a1")]);
        assert!(agent.closed(), "the stream closes once the turn ended");
    }

    /// A reader that let go before it read a frame (an opening cancelled
    /// after the headers came) named nothing; the role chunk Hermes already
    /// wrote does, and that turn is stopped.
    #[tokio::test]
    async fn a_turn_let_go_of_before_its_first_frame_is_stopped() {
        let mut agent = agent(&[], 0).await;
        let (body, let_go) = agent.guarded(vec![chunk(&role("chatcmpl-unread"))], false);
        drop(body);
        assert!(let_go.happened());
        assert_eq!(agent.settled().await, vec![landed("chatcmpl-unread")]);
        assert!(agent.closed());
    }

    /// Hermes writes the role chunk before the turn's id is bound, so the
    /// first stop can find nothing; it is sent again, on the open stream,
    /// until it lands.
    #[tokio::test]
    async fn a_stop_hermes_could_not_place_yet_is_sent_again() {
        let mut agent = agent(&[], 2).await;
        let (body, _) = agent.guarded(vec![chunk(&role("chatcmpl-early"))], false);
        read_then_drop(body, 1).await;
        let seen = agent.settled().await;
        let answers: Vec<u16> = seen.iter().map(|s| s.answered).collect();
        assert_eq!(answers, vec![409, 409, 200], "{seen:?}");
        assert!(
            seen.iter().all(|s| s.run == "chatcmpl-early" && s.open),
            "{seen:?}"
        );
        assert!(agent.closed());
    }

    /// On a starved agent store Hermes can take minutes to bind the id, so a
    /// stop it cannot place is sent again, backing off, past the control
    /// window, for as long as the place window allows, and lands once it can.
    #[tokio::test]
    async fn a_stop_is_sent_again_past_the_control_window_while_hermes_cannot_place_it() {
        let mut agent = agent(&[], 6).await;
        let (body, _) = agent.guarded(vec![chunk(&role("chatcmpl-slow"))], false);
        let dropped = Instant::now();
        read_then_drop(body, 1).await;
        let seen = agent.settled().await;
        let answers: Vec<u16> = seen.iter().map(|s| s.answered).collect();
        assert_eq!(answers, [vec![409; 6], vec![200]].concat(), "{seen:?}");
        // 50 + 100 + 200 + 400 + 400 + 400 ms of waits: past the 800 ms
        // control window.
        assert!(dropped.elapsed() > QUICK.control, "{:?}", dropped.elapsed());
        assert!(seen.iter().all(|s| s.open), "{seen:?}");
        assert!(agent.closed());
    }

    /// A stop Hermes never places is given up at the place window, and the
    /// stream is closed then, unstopped: the guard does not hold a request
    /// open for good.
    #[tokio::test]
    async fn a_stop_never_placed_is_given_up_at_the_place_window() {
        let mut agent = agent(&["chatcmpl-never"], 0).await;
        let windows = Windows {
            control: Duration::from_millis(200),
            place: Duration::from_millis(900),
            retry: Duration::from_millis(50),
            retry_max: Duration::from_millis(200),
        };
        let (body, _) = agent.guarded_with(vec![chunk(&role("chatcmpl-never"))], false, windows);
        read_then_drop(body, 1).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!agent.closed(), "still held open inside the place window");
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(agent.closed(), "closed once the place window ran out");
        let seen = agent.stops.lock().unwrap().clone();
        assert!(seen.len() >= 3, "{seen:?}");
        assert!(seen.iter().all(|s| s.answered == 409 && s.open), "{seen:?}");
    }

    /// A turn the lost-turn watch moved names the fresh completion in its
    /// next role chunk; that one is stopped, not the refused one.
    #[tokio::test]
    async fn a_moved_turn_is_stopped_on_the_session_it_moved_to() {
        let mut agent = agent(&["chatcmpl-refused"], 0).await;
        let (body, _) = agent.guarded(
            vec![
                chunk(&role("chatcmpl-refused")),
                chunk(&role("chatcmpl-fresh")),
                chunk(&words("chatcmpl-fresh", "work")),
            ],
            false,
        );
        read_then_drop(body, 3).await;
        assert_eq!(agent.settled().await, vec![landed("chatcmpl-fresh")]);
    }

    /// A move still waiting for its fresh stream when the reader let go: the
    /// refused completion is no longer Hermes' to stop, and the fresh one,
    /// named once the move's stream comes, is.
    #[tokio::test]
    async fn a_move_in_flight_when_the_reader_left_is_stopped_once_it_lands() {
        let mut agent = agent(&["chatcmpl-refused"], 0).await;
        let (body, _) = agent.guarded(vec![chunk(&role("chatcmpl-refused"))], false);
        agent.later(Duration::from_millis(600), role("chatcmpl-moved"));
        read_then_drop(body, 1).await;
        let seen = agent.settled().await;
        assert_eq!(seen.last(), Some(&landed("chatcmpl-moved")), "{seen:?}");
        assert!(
            seen.iter()
                .filter(|s| s.run == "chatcmpl-refused")
                .all(|s| s.answered == 409),
            "{seen:?}"
        );
        assert!(agent.closed());
    }

    /// A turn read to its end is not stopped: neither one read until the body
    /// ran out, nor one whose reader stopped at `[DONE]`.
    #[tokio::test]
    async fn a_turn_read_to_its_end_is_not_stopped() {
        let mut ran_out = agent(&[], 0).await;
        let (mut body, let_go) = ran_out.guarded(
            vec![
                chunk(&role("chatcmpl-a1")),
                chunk(&words("chatcmpl-a1", "hi")),
                chunk(&finish("chatcmpl-a1")),
            ],
            true,
        );
        while body.next().await.is_some() {}
        drop(body);
        assert!(!let_go.happened());
        let mut at_done = agent(&[], 0).await;
        let (body, let_go) = at_done.guarded(
            vec![chunk(&role("chatcmpl-a2")), chunk(&finish("chatcmpl-a2"))],
            false,
        );
        read_then_drop(body, 2).await;
        assert!(!let_go.happened());
        assert!(ran_out.settled().await.is_empty());
        assert!(at_done.settled().await.is_empty());
    }

    /// The id is read from a frame that arrived in pieces, and only from the
    /// chunk's own `id`: a completion id the reply's words quote is not one.
    #[tokio::test]
    async fn the_id_is_the_chunks_own_even_split_or_quoted() {
        let mut agent = agent(&[], 0).await;
        let first = role("chatcmpl-split9");
        let (a, b) = first.split_at(20);
        let (body, _) = agent.guarded(
            vec![
                chunk(a),
                chunk(b),
                chunk(&words("chatcmpl-split9", "stop chatcmpl-other please")),
            ],
            false,
        );
        read_then_drop(body, 3).await;
        assert_eq!(agent.settled().await, vec![landed("chatcmpl-split9")]);
    }

    /// Nothing to stop: a stream that never named a completion, and one that
    /// broke (no request is left for an id to name).
    #[tokio::test]
    async fn nothing_is_stopped_without_a_named_live_completion() {
        let mut nameless = agent(&[], 0).await;
        let (body, _) = nameless.guarded(vec![chunk(KEEPALIVE)], false);
        read_then_drop(body, 1).await;
        let broken_err = reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("nothing listens on port 1");
        let mut broke = agent(&[], 0).await;
        let (mut body, let_go) =
            broke.guarded(vec![chunk(&role("chatcmpl-b1")), Err(broken_err)], false);
        assert!(body.next().await.unwrap().is_ok());
        assert!(body.next().await.unwrap().is_err());
        drop(body);
        assert!(!let_go.happened());
        assert!(nameless.settled().await.is_empty());
        assert!(broke.settled().await.is_empty());
    }

    #[test]
    fn frames_read() {
        assert_eq!(
            completion_of(role("chatcmpl-x1").as_bytes()),
            Frame::Named("chatcmpl-x1".into())
        );
        assert_eq!(completion_of(b"data: [DONE]\n\n"), Frame::Done);
        assert_eq!(completion_of(KEEPALIVE.as_bytes()), Frame::Other);
        assert_eq!(
            completion_of(b"event: hermes.tool.progress\ndata: {\"toolCallId\": \"c1\", \"status\": \"running\"}\n\n"),
            Frame::Other
        );
        // A session-arm frame names a `run_…`, which is not this guard's.
        assert_eq!(
            completion_of(b"event: run.started\ndata: {\"run_id\": \"run_1\"}\n\n"),
            Frame::Other
        );
        assert_eq!(
            completion_of(b"data: {\"id\": \"chatcmpl-\"}\n\n"),
            Frame::Other
        );
    }
}
