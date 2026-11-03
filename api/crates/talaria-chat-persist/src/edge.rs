// THE CHAT ROUTE ANSWERS INSIDE THE EDGE'S CUTOFF.
//
// `/api/chat` answers with the agent's own status, so its headers used to
// wait for the agent to open the turn. Two waits sit before that, and either
// can outlast the edge in front of every real instance (Cloudflare answers
// 524 for a request whose origin has sent no headers for 100 s,
// `EDGE_CUTOFF`). One is the hold for an agent that is not serving yet
// (`ATTENDED_HOLD`, 80 s). The other is the wait for the headers of an agent
// that took the turn and is slow to its first frame, from a long context or
// a compaction (`HEADERS_WINDOW`, five minutes). Behind the edge a person got
// a 524 for a turn that went on and landed.
//
// So the turn opens in a task of its own, and the route waits for it only
// until [`FIRST_BYTE_WITHIN`], counted from when the request arrived. A turn
// that opens in time is answered exactly as before: the agent's status and
// content type. A turn that has not opened by then is answered 200
// `text/event-stream` at once. Its body says `: waiting for the agent` at
// once and every [`KEEPALIVE_EVERY`] until the turn's first byte, and then
// carries the turn as it would have. Every reader of this stream skips an SSE
// comment: the web's `sse-parse.ts`, the phone's `SseParser`, and the drain's
// own parser, which never sees one because the keepalive rides only the
// client's branch of the tee.
//
// A STATUS THAT COMES LATE IS SAID IN THE STREAM. Once the 200 has gone out,
// an agent that answers the turn with an error status (a refusal, or the
// gateway's own 504 for headers that never came) can no longer set the
// status. Its body goes out as one failure frame, `{"error":{"message":…}}`,
// the shape of the agent's own failure frames. The drain lands the reply as
// an error that says why, and a page that read the stream reads that row
// when it syncs.
//
// THE TURN IS NOT THE REQUEST'S. A reader that leaves while the turn is
// still opening no longer cancels it: the turn opens, and the drain saves
// it, as it always did once the headers were out (the tee).

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use talaria_gateway::fleet_chat::{ByteStream, ChatStream, EDGE_CUTOFF};
use tokio::sync::{mpsc, oneshot};

/// How long the route waits for the agent to open the turn before it opens
/// the answer itself, counted from when the request arrived. Short of the
/// edge by a wide margin. The person sees nothing different (the reply is
/// "working" either way), and a new thread is named sooner, so a Stop
/// pressed on its first turn has an id to name.
pub const FIRST_BYTE_WITHIN: Duration = Duration::from_secs(30);

/// How often an answer opened early says it is still waiting.
pub const KEEPALIVE_EVERY: Duration = Duration::from_secs(15);

// The early answer leaves the route well inside the edge, with room for the
// hops between: an answer that opens later than this does not build.
const _: () = assert!(FIRST_BYTE_WITHIN.as_secs() + 30 <= EDGE_CUTOFF.as_secs());
// Several keepalives fit in every window the edge measures, so one held up
// at a hop never leaves the connection silent long enough to be cut.
const _: () = assert!(KEEPALIVE_EVERY.as_secs() * 4 <= EDGE_CUTOFF.as_secs());

/// The keepalive an early answer sends: an SSE comment.
pub const KEEPALIVE: &[u8] = b": waiting for the agent\n\n";

/// One item of a turn's body, as the agent's gateway streamed it.
type BodyItem = <ByteStream as Stream>::Item;

/// What the agent's answer said about itself: the route's status and
/// content type when the turn opened in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnHead {
    pub status: u16,
    pub content_type: String,
}

/// The client's branch of the tee.
pub type ClientBody = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

/// The route's answer to a turn.
pub struct Answer {
    /// The agent's own head, when the turn opened in time. None is the
    /// answer opened early: 200 `text/event-stream`, kept alive by `body`.
    pub head: Option<TurnHead>,
    pub body: ClientBody,
}

impl Answer {
    pub fn status(&self) -> u16 {
        self.head.as_ref().map_or(200, |h| h.status)
    }

    pub fn content_type(&self) -> &str {
        self.head
            .as_ref()
            .map_or("text/event-stream", |h| h.content_type.as_str())
    }
}

/// Open `turn` in a task of its own, hand its body to `drain` there with the
/// sender of the client's branch, and answer within `within`: with the
/// agent's head if the turn opened in time, else at once with an answer
/// kept alive every `every` until the turn speaks.
pub async fn answer_turn<T, D, F>(turn: T, drain: D, within: Duration, every: Duration) -> Answer
where
    T: Future<Output = ChatStream> + Send + 'static,
    D: FnOnce(ByteStream, mpsc::Sender<Result<Bytes, std::io::Error>>) -> F + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    let (head_tx, mut head_rx) = oneshot::channel::<TurnHead>();
    let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let upstream = turn.await;
        let head = TurnHead {
            status: upstream.status,
            content_type: upstream.content_type.clone(),
        };
        // A head nobody takes is a head that came after the early answer.
        let body = match head_tx.send(head) {
            Ok(()) => upstream.body,
            Err(head) => said_in_stream(head.status, upstream.body),
        };
        drain(body, tx).await;
    });
    let head = match tokio::time::timeout(within, &mut head_rx).await {
        Ok(Ok(head)) => Some(head),
        // The turn's task ended without a head: it panicked.
        Ok(Err(_)) => {
            return Answer {
                head: Some(TurnHead {
                    status: 502,
                    content_type: "text/plain".into(),
                }),
                body: Box::pin(futures_util::stream::once(async {
                    Ok(Bytes::from_static(
                        b"the turn ended before the agent opened it",
                    ))
                })),
            };
        }
        // Closed first, then read once: a head sent in the meantime is still
        // this answer's, and none can be sent after.
        Err(_) => {
            head_rx.close();
            head_rx.try_recv().ok()
        }
    };
    let body = match head {
        Some(_) => relayed(rx),
        None => kept_alive(rx, every),
    };
    Answer { head, body }
}

/// The client's branch as the drain feeds it.
fn relayed(rx: mpsc::Receiver<Result<Bytes, std::io::Error>>) -> ClientBody {
    Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }))
}

#[derive(Clone, Copy)]
enum Phase {
    Opening,
    Waiting,
    Speaking,
}

/// The client's branch of an answer opened early: a keepalive at once and
/// every `every` until the first byte of the turn, then the turn. Only
/// before the first byte, because a keepalive between two of the turn's
/// chunks could land inside a frame.
fn kept_alive(rx: mpsc::Receiver<Result<Bytes, std::io::Error>>, every: Duration) -> ClientBody {
    Box::pin(futures_util::stream::unfold(
        (rx, Phase::Opening),
        move |(mut rx, phase)| async move {
            match phase {
                Phase::Opening => Some((Ok(Bytes::from_static(KEEPALIVE)), (rx, Phase::Waiting))),
                // `recv` is cancel-safe: a timeout drops nothing.
                Phase::Waiting => match tokio::time::timeout(every, rx.recv()).await {
                    Err(_) => Some((Ok(Bytes::from_static(KEEPALIVE)), (rx, Phase::Waiting))),
                    Ok(item) => item.map(|item| (item, (rx, Phase::Speaking))),
                },
                Phase::Speaking => rx.recv().await.map(|item| (item, (rx, Phase::Speaking))),
            }
        },
    ))
}

/// The longest error body read for its sentence.
const LATE_BODY_LIMIT: usize = 4 * 1024;

/// A turn's body after the early answer: as it is when the agent answered
/// 2xx, else one failure frame saying what the agent's error status did.
fn said_in_stream(status: u16, body: ByteStream) -> ByteStream {
    if (200..300).contains(&status) {
        return body;
    }
    Box::pin(futures_util::stream::once(async move {
        let mut body = body;
        let mut read: Vec<u8> = Vec::new();
        while read.len() < LATE_BODY_LIMIT {
            match body.next().await {
                Some(Ok(chunk)) => read.extend_from_slice(&chunk),
                _ => break,
            }
        }
        let message = late_failure(status, &String::from_utf8_lossy(&read));
        let frame: BodyItem = Ok(Bytes::from(failure_frame(&message)));
        frame
    }))
}

/// What an error answer said: its `{"error":{"message":…}}` (or
/// `{"error":"…"}`), else its text, else its status.
fn late_failure(status: u16, body: &str) -> String {
    let said = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            let error = v.get("error")?;
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .map(str::to_string)
        })
        .or_else(|| Some(body.trim().to_string()))
        .map(|s| s.chars().take(500).collect::<String>())
        .filter(|s| !s.trim().is_empty());
    said.unwrap_or_else(|| format!("the agent answered HTTP {status}"))
}

/// One failure frame, in the shape the agent's own failure frames have.
fn failure_frame(message: &str) -> String {
    format!("data: {}\n\n", json!({ "error": { "message": message } }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use talaria_gateway::fleet_chat::{AgentStreamEvent, AgentStreamParser};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A mock agent's turn: nothing for `withheld`, then `status` with these
    /// SSE frames, the way an agent slow to its first frame opens its stream.
    async fn slow_turn(
        withheld: Duration,
        status: u16,
        content_type: &'static str,
        frames: &'static [&'static str],
    ) -> ChatStream {
        tokio::time::sleep(withheld).await;
        let body = futures_util::stream::iter(
            frames
                .iter()
                .map(|f| -> BodyItem { Ok(Bytes::from_static(f.as_bytes())) }),
        );
        ChatStream {
            status,
            content_type: content_type.into(),
            body: Box::pin(body),
            canned: None,
            hermes_session: None,
        }
    }

    /// A drain that keeps a copy of everything it was handed (what the
    /// persist loop would parse) and forwards it, as the persist loop does.
    fn copying_drain(
        seen: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    ) -> impl FnOnce(
        ByteStream,
        mpsc::Sender<Result<Bytes, std::io::Error>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>
    + Send
    + 'static {
        move |mut body, tx| {
            Box::pin(async move {
                while let Some(Ok(chunk)) = body.next().await {
                    seen.lock().unwrap().extend_from_slice(&chunk);
                    let _ = tx.send(Ok(chunk)).await;
                }
            })
        }
    }

    /// Serve one answer the way the route does, on a loopback port.
    async fn serve(answer: Answer) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cell = std::sync::Arc::new(std::sync::Mutex::new(Some(answer)));
        let app = axum::Router::new().route(
            "/api/chat",
            axum::routing::post(move || {
                let cell = cell.clone();
                async move {
                    let answer = cell.lock().unwrap().take().expect("one request");
                    axum::response::Response::builder()
                        .status(answer.status())
                        .header("cache-control", "no-cache")
                        .header("content-type", answer.content_type().to_string())
                        .body(axum::body::Body::from_stream(answer.body))
                        .unwrap()
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    const FRAMES: &[&str] = &[
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        "data: [DONE]\n\n",
    ];

    /// THE CASE THE EDGE CUT: an agent that took the turn and withholds its
    /// headers past the route's first-byte bound. The client has the head and
    /// a keepalive on the wire long before the agent speaks, and then the
    /// turn's own frames, whole. The route that awaited the turn first sent
    /// its first byte only once the agent did.
    #[tokio::test]
    async fn a_turn_slow_to_open_is_answered_before_the_agent_speaks() {
        let withheld = Duration::from_millis(1500);
        let within = Duration::from_millis(200);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sent = Instant::now();
        let answer = answer_turn(
            slow_turn(withheld, 200, "text/event-stream", FRAMES),
            copying_drain(seen.clone()),
            within,
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(answer.head, None, "past its bound the answer opens itself");
        let addr = serve(answer).await;

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(
            b"POST /api/chat HTTP/1.1\r\nhost: t\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        let mut first = vec![0u8; 4096];
        let n = conn.read(&mut first).await.unwrap();
        let first_byte = sent.elapsed();
        assert!(
            first_byte < withheld - Duration::from_millis(500),
            "the first byte left at {first_byte:?}, with the agent's headers due at {withheld:?}"
        );
        let mut all = first[..n].to_vec();
        conn.read_to_end(&mut all).await.unwrap();
        let wire = String::from_utf8_lossy(&all);
        assert!(wire.starts_with("HTTP/1.1 200"), "{wire}");
        assert!(wire.contains("content-type: text/event-stream"), "{wire}");
        let keepalive = wire.find(": waiting for the agent").expect("a keepalive");
        let reply = wire.find("hello").expect("the turn's own frame");
        assert!(keepalive < reply, "{wire}");
        assert!(wire.contains("data: [DONE]"), "{wire}");
        // Several keepalives while it waited, none after it spoke.
        assert!(
            wire.matches(": waiting for the agent").count() >= 3,
            "{wire}"
        );
        assert!(!wire[reply..].contains(": waiting"), "{wire}");
        // The drain had the turn, and no keepalive.
        let drained = String::from_utf8(seen.lock().unwrap().clone()).unwrap();
        assert_eq!(drained, FRAMES.concat());
    }

    /// A turn that opens in time is answered as the route always answered
    /// it: the agent's status, its content type and its body as sent.
    #[tokio::test]
    async fn a_turn_that_opens_in_time_keeps_its_own_status() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let answer = answer_turn(
            slow_turn(
                Duration::from_millis(20),
                429,
                "application/json",
                &["{\"error\":{\"message\":\"slow down\"}}"],
            ),
            copying_drain(seen.clone()),
            Duration::from_secs(5),
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(
            answer.head,
            Some(TurnHead {
                status: 429,
                content_type: "application/json".into()
            })
        );
        let body: Vec<Bytes> = answer.body.map(|r| r.unwrap()).collect().await;
        assert_eq!(body.concat(), b"{\"error\":{\"message\":\"slow down\"}}");
    }

    /// An error status that comes after the early answer is said in the
    /// stream, as a failure frame the drain lands as the reply's error.
    #[tokio::test]
    async fn an_error_after_the_answer_opened_is_said_in_the_stream() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let answer = answer_turn(
            slow_turn(
                Duration::from_millis(250),
                504,
                "text/plain",
                &["the agent gateway accepted the request and sent no response headers within 300s"],
            ),
            copying_drain(seen.clone()),
            Duration::from_millis(50),
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(answer.status(), 200);
        assert_eq!(answer.content_type(), "text/event-stream");
        let body: Vec<Bytes> = answer.body.map(|r| r.unwrap()).collect().await;
        let wire = String::from_utf8(body.concat()).unwrap();
        assert!(wire.starts_with(": waiting for the agent\n\n"), "{wire}");
        let drained = seen.lock().unwrap().clone();
        assert!(
            wire.ends_with(std::str::from_utf8(&drained).unwrap()),
            "{wire}"
        );
        let mut parser = AgentStreamParser::new();
        let mut events = parser.feed(&drained);
        events.extend(parser.finish());
        assert_eq!(
            events,
            vec![AgentStreamEvent::Error {
                message: "the agent gateway accepted the request and sent no response headers \
                          within 300s"
                    .into()
            }]
        );
    }

    #[test]
    fn a_late_error_says_what_the_agent_said_or_its_status() {
        assert_eq!(
            late_failure(400, "{\"error\":{\"message\":\"bad turn\"}}"),
            "bad turn"
        );
        assert_eq!(late_failure(409, "{\"error\":\"busy\"}"), "busy");
        assert_eq!(
            late_failure(502, "  upstream went away \n"),
            "upstream went away"
        );
        assert_eq!(late_failure(503, ""), "the agent answered HTTP 503");
        assert_eq!(late_failure(500, "{\"error\":{}}"), "{\"error\":{}}");
    }

    /// The bound and the beat, measured against the edge rather than taken
    /// on trust: the consts above say the same at build time.
    #[test]
    fn the_answer_opens_well_inside_the_edge() {
        assert!(FIRST_BYTE_WITHIN + Duration::from_secs(30) <= EDGE_CUTOFF);
        assert!(KEEPALIVE_EVERY * 4 <= EDGE_CUTOFF);
        assert!(KEEPALIVE.starts_with(b":") && KEEPALIVE.ends_with(b"\n\n"));
    }
}
