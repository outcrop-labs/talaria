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
// binding is dropped when the request ends, so the order matters: the stop is
// sent while the stream is still open, and the stream is closed after the
// agent answers it (or the control window runs out).
//
// WHEN. A stream let go of before it ended: before `data: [DONE]` passed and
// before the body ran out. A stream that broke (a read error) has no request
// left for an id to name, and one that never named a completion id has nothing
// to stop. A turn the lost-turn watch moved to a fresh session names the fresh
// completion in its next role chunk, and that is the one stopped. A move still
// waiting for its headers when the reader leaves is closed with the stream,
// unstopped: Hermes ends it at its first failed write.
//
// The session arm is not covered here. Its reader names the run from
// `run.started` and stops it by that id itself (talaria-app-agents).

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use serde_json::Value;
use talaria_fleet_layout::ManifestEntry;

use crate::fleet_chat::{ByteStream, CONTROL_WINDOW, ChatStream, agent_post, control_failure};

/// The longest frame read for its id. A chunk is a few hundred bytes; a
/// frame past this is not one the guard understands, so it stops reading
/// frames (the bytes still pass) rather than hold them.
const FRAME_LIMIT: usize = 64 * 1024;

/// Wrap a completions turn's stream so that letting go of it early stops the
/// turn in the agent (see the header).
pub(crate) fn stop_when_abandoned(mut stream: ChatStream, entry: &ManifestEntry) -> ChatStream {
    let inner = std::mem::replace(&mut stream.body, Box::pin(futures_util::stream::empty()));
    stream.body = Box::pin(Guarded {
        inner: Some(inner),
        entry: ManifestEntry {
            model: entry.model.clone(),
            url: entry.url.clone(),
            key: entry.key.clone(),
        },
        run: None,
        ended: false,
        head: Vec::new(),
        reading: true,
    });
    stream
}

struct Guarded {
    inner: Option<ByteStream>,
    entry: ManifestEntry,
    /// The completion the stream named last.
    run: Option<String>,
    /// The stream ran out, broke, or said `[DONE]`: nothing is left to stop.
    ended: bool,
    /// Bytes of a frame still arriving.
    head: Vec<u8>,
    reading: bool,
}

impl Guarded {
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
                this.note(&chunk);
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(e))) => {
                this.ended = true;
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                this.ended = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        let (Some(run), Some(inner)) = (self.run.take(), self.inner.take()) else {
            return;
        };
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
        handle.spawn(async move {
            // The stop first, while the request that binds `run` to its
            // admission is still open; the stream closes after.
            let answer = stop(&entry, &run).await;
            drop(inner);
            match answer {
                Ok(status) => tracing::info!(
                    "[fleet] {}: a turn its reader let go of was stopped in the agent ({run}: {status})",
                    entry.model
                ),
                // Most often the turn had just finished and Hermes forgot the id.
                Err(why) => tracing::debug!(
                    "[fleet] {}: stopping {run}, which its reader let go of, answered {why}",
                    entry.model
                ),
            }
        });
    }
}

/// `POST /v1/runs/{run}/stop` on this agent, inside the control window: the
/// run's status as Hermes answers it, or why there was no stop.
async fn stop(entry: &ManifestEntry, run: &str) -> Result<String, String> {
    let sent = tokio::time::timeout(
        CONTROL_WINDOW,
        agent_post(entry, &format!("/v1/runs/{run}/stop")).send(),
    )
    .await;
    match sent {
        Ok(Ok(res)) if res.status().is_success() => Ok(res
            .json::<Value>()
            .await
            .ok()
            .and_then(|b| b["status"].as_str().map(str::to_string))
            .unwrap_or_else(|| "stopped".into())),
        Ok(Ok(res)) => Err(control_failure(res).await),
        Ok(Err(e)) => Err(e.without_url().to_string()),
        Err(_) => Err("no answer inside the control window".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures_util::StreamExt;

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

    const KEEPALIVE: &str = ": keepalive\n\n";
    const DONE: &str = "data: {\"id\": \"chatcmpl-a1\", \"choices\": [{\"index\": 0, \"delta\": {}, \"finish_reason\": \"stop\"}]}\n\ndata: [DONE]\n\n";

    /// What the stand-in agent was asked to stop: the run, the bearer, and
    /// whether the turn's stream was still open when the stop arrived.
    #[derive(Clone, Default)]
    struct Stops(Arc<Mutex<Vec<(String, Option<String>, bool)>>>);

    /// The agent's `/v1/runs/{id}/stop`, answering as Hermes does for a
    /// queued admission it cancelled.
    async fn agent(stream_closed: Arc<AtomicBool>) -> (ManifestEntry, Stops) {
        let stops = Stops::default();
        let kept = stops.clone();
        let app = axum::Router::new().route(
            "/v1/runs/{run}/stop",
            axum::routing::post(
                move |axum::extract::Path(run): axum::extract::Path<String>,
                      headers: axum::http::HeaderMap| {
                    let kept = kept.clone();
                    let closed = stream_closed.clone();
                    async move {
                        let bearer = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_string);
                        kept.0.lock().unwrap().push((
                            run.clone(),
                            bearer,
                            closed.load(Ordering::SeqCst),
                        ));
                        axum::Json(serde_json::json!({ "run_id": run, "status": "cancelled" }))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let entry = ManifestEntry {
            model: "ada-ops".into(),
            url,
            key: Some("agent-door-key".into()),
        };
        (entry, stops)
    }

    /// The turn's own stream: these chunks, then (unless `ends`) a wait that
    /// never ends, as a queued turn's stream is between keep-alives. Its drop
    /// sets `closed`, which is the request closing.
    struct Turn {
        chunks: std::collections::VecDeque<Item>,
        ends: bool,
        closed: Arc<AtomicBool>,
    }

    impl Stream for Turn {
        type Item = Item;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Item>> {
            let this = self.get_mut();
            match this.chunks.pop_front() {
                Some(c) => Poll::Ready(Some(c)),
                None if this.ends => Poll::Ready(None),
                None => Poll::Pending,
            }
        }
    }

    impl Drop for Turn {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    fn guarded(
        entry: &ManifestEntry,
        chunks: Vec<Item>,
        ends: bool,
        closed: Arc<AtomicBool>,
    ) -> ByteStream {
        let turn = Turn {
            chunks: chunks.into(),
            ends,
            closed,
        };
        let stream = ChatStream {
            status: 200,
            content_type: "text/event-stream".into(),
            body: Box::pin(turn),
            canned: None,
            hermes_session: None,
        };
        stop_when_abandoned(stream, entry).body
    }

    /// Read `n` chunks, then let go.
    async fn read_then_drop(mut body: ByteStream, n: usize) {
        for _ in 0..n {
            body.next().await.expect("a chunk").expect("not an error");
        }
        drop(body);
    }

    async fn settled(stops: &Stops) -> Vec<(String, Option<String>, bool)> {
        for _ in 0..100 {
            if !stops.0.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Long enough for a second, wrong stop to have arrived too.
        tokio::time::sleep(Duration::from_millis(100)).await;
        stops.0.lock().unwrap().clone()
    }

    /// A turn let go of mid-stream is stopped by the completion id its chunks
    /// named, with the agent's own bearer — and the stop lands while the
    /// turn's stream is still open (Hermes knows the id only while the
    /// request lives); the stream closes after the answer.
    #[tokio::test]
    async fn a_turn_let_go_of_is_stopped_while_its_stream_is_still_open() {
        let closed = Arc::new(AtomicBool::new(false));
        let (entry, stops) = agent(closed.clone()).await;
        let body = guarded(
            &entry,
            vec![chunk(&role("chatcmpl-a1")), chunk(KEEPALIVE)],
            false,
            closed.clone(),
        );
        read_then_drop(body, 2).await;
        let seen = settled(&stops).await;
        assert_eq!(
            seen,
            vec![(
                "chatcmpl-a1".to_string(),
                Some("Bearer agent-door-key".to_string()),
                false
            )]
        );
        assert!(
            closed.load(Ordering::SeqCst),
            "the stream closes once the stop answered"
        );
    }

    /// A turn the lost-turn watch moved names the fresh completion in its
    /// next role chunk; that one is stopped, not the refused one.
    #[tokio::test]
    async fn a_moved_turn_is_stopped_on_the_session_it_moved_to() {
        let closed = Arc::new(AtomicBool::new(false));
        let (entry, stops) = agent(closed.clone()).await;
        let body = guarded(
            &entry,
            vec![
                chunk(&role("chatcmpl-refused")),
                chunk(&role("chatcmpl-fresh")),
                chunk(&words("chatcmpl-fresh", "work")),
            ],
            false,
            closed,
        );
        read_then_drop(body, 3).await;
        let seen = settled(&stops).await;
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].0, "chatcmpl-fresh");
    }

    /// A turn read to its end is not stopped: neither one read until the body
    /// ran out, nor one whose reader stopped at `[DONE]`.
    #[tokio::test]
    async fn a_turn_read_to_its_end_is_not_stopped() {
        let closed = Arc::new(AtomicBool::new(false));
        let (entry, stops) = agent(closed.clone()).await;
        let mut body = guarded(
            &entry,
            vec![
                chunk(&role("chatcmpl-a1")),
                chunk(&words("chatcmpl-a1", "hi")),
                chunk(DONE),
            ],
            true,
            closed.clone(),
        );
        while body.next().await.is_some() {}
        drop(body);
        let at_done = guarded(
            &entry,
            vec![chunk(&role("chatcmpl-a2")), chunk(DONE)],
            false,
            closed,
        );
        read_then_drop(at_done, 2).await;
        assert!(settled(&stops).await.is_empty());
    }

    /// The id is read from a frame that arrived in pieces, and only from the
    /// chunk's own `id`: a completion id the reply's words quote is not one.
    #[tokio::test]
    async fn the_id_is_the_chunks_own_even_split_or_quoted() {
        let closed = Arc::new(AtomicBool::new(false));
        let (entry, stops) = agent(closed.clone()).await;
        let first = role("chatcmpl-split9");
        let (a, b) = first.split_at(20);
        let body = guarded(
            &entry,
            vec![
                chunk(a),
                chunk(b),
                chunk(&words("chatcmpl-split9", "stop chatcmpl-other please")),
            ],
            false,
            closed,
        );
        read_then_drop(body, 3).await;
        let seen = settled(&stops).await;
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].0, "chatcmpl-split9");
    }

    /// Nothing to stop: a stream that never named a completion, and one that
    /// broke (no request is left for an id to name).
    #[tokio::test]
    async fn nothing_is_stopped_without_a_named_live_completion() {
        let closed = Arc::new(AtomicBool::new(false));
        let (entry, stops) = agent(closed.clone()).await;
        let nameless = guarded(&entry, vec![chunk(KEEPALIVE)], false, closed.clone());
        read_then_drop(nameless, 1).await;
        let broken_err = reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("nothing listens on port 1");
        let mut broken = guarded(
            &entry,
            vec![chunk(&role("chatcmpl-b1")), Err(broken_err)],
            false,
            closed,
        );
        assert!(broken.next().await.unwrap().is_ok());
        assert!(broken.next().await.unwrap().is_err());
        drop(broken);
        assert!(settled(&stops).await.is_empty());
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
