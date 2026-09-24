mod support;
// Live-wire proof of the TALA-80 sender-identity stamp (cargo test --
// --ignored). The unit tests pin the block's shape; this file pins the only
// fact that matters at the door: the message actually lands on the WIRE the
// gateway receives, on every door that stamps it — the direct chat turn
// (live path) and the channel reply (the same path ticket task rooms take).
//
// The rig: the real router over the real Postgres and Redis, plus a FAKE
// agent in TALARIA_FLEET_DIR pointed at a local TCP recorder that captures
// the exact chat/completions body Talaria sends. The proxy reads the
// manifest per request (talaria_fleet_layout::fleet_dir → std::env), so a
// dedicated test binary can set the env var without racing any other test.
//
// House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=... REDIS_URL=... TALARIA_SECRET_KEY=ci-dummy \
//   cargo test --test sender_identity_live -- --ignored

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::Request;
use talaria_api::channels::{CreatedChannel, add_channel_agent, create_channel};
use talaria_api::config::Config;
use talaria_api::notify::NotifyDeps;
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use tower::ServiceExt; // oneshot

/// The fleet env var is process-global and the two tests run in parallel:
/// whoever holds this lock owns TALARIA_FLEET_DIR for their whole turn,
/// because the proxy re-reads it per request.
static FLEET_ENV: Mutex<()> = Mutex::new(());

/// The fixture tag this binary owns — the sweep pattern matches every user
/// row it ever made, so a failed previous run cannot shadow the next one.
const AGENT: &str = "live-test-agent";

/// The recorder: a local listener standing in for one fleet agent. Each
/// accepted connection is one gateway POST; the body is stashed raw and an
/// OpenAI-shaped SSE stream (two deltas then [DONE]) is sent back so the
/// persist side drains a normal completion instead of erroring.
struct Recorder {
    bodies: Arc<Mutex<Vec<String>>>,
    port: u16,
}

impl Recorder {
    fn start() -> Recorder {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind recorder");
        let port = listener.local_addr().unwrap().port();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let sink = bodies.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                // Read until the body is fully in: the headers arrive first,
                // Content-Length says how much body follows them.
                let mut header_end = None;
                let mut content_length = 0usize;
                loop {
                    match stream.read(&mut chunk) {
                        // A read error is the client hanging up — the recorder
                        // just drops this request's tail and answers anyway.
                        Err(_) => break,
                        Ok(n) => {
                            if n == 0 {
                                break;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                            if header_end.is_none()
                                && let Some(pos) = find_header_end(&buf)
                            {
                                header_end = Some(pos);
                                let headers = String::from_utf8_lossy(&buf[..pos]).to_string();
                                for line in headers.split("\r\n") {
                                    let lower = line.to_ascii_lowercase();
                                    if let Some(v) = lower.strip_prefix("content-length:") {
                                        content_length = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                            if let Some(pos) = header_end
                                && buf.len() >= pos + 4 + content_length
                            {
                                break;
                            }
                        }
                    }
                }
                if let Some(pos) = header_end {
                    let body = String::from_utf8_lossy(&buf[pos + 4..]).into_owned();
                    sink.lock().unwrap().push(body);
                }
                // The OpenAI-shaped answer: two content deltas, then DONE.
                let sse = concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\" noted\"}}]}\n\n",
                    "data: [DONE]\n\n",
                );
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{sse}",
                        sse.len()
                    )
                    .as_bytes(),
                );
                let _ = stream.flush();
            }
        });
        Recorder { bodies, port }
    }

    /// The captured request bodies so far (parsed as JSON, in arrival order).
    fn captured(&self) -> Vec<serde_json::Value> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .filter_map(|b| serde_json::from_str(b).ok())
            .collect()
    }

    /// Wait for the nth captured request, then return it — the channel path
    /// is a detached spawn, so the POST arrives at the recorder on its own
    /// schedule and the test polls rather than assumes.
    async fn await_body(&self, index: usize, timeout: Duration) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(v) = self.captured().into_iter().nth(index) {
                return v;
            }
            if Instant::now() >= deadline {
                panic!(
                    "recorder never saw request {index} within {timeout:?} \
                     ({} captured)",
                    self.bodies.lock().unwrap().len()
                );
            }
            // Async sleep: a blocking sleep here would starve the very
            // runtime the detached trigger spawn needs to run on.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Write the one-agent fleet manifest pointing at the recorder and point
/// TALARIA_FLEET_DIR at it. /tmp is fine for a throwaway manifest the test
/// owns alone; the env var is read per request by the proxy.
fn point_fleet_at(recorder_port: u16, tag: &str) {
    let dir = std::env::temp_dir().join(format!("talaria-fleet-{tag}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("fleet.json"),
        format!(
            "[{{\"model\":\"{AGENT}\",\"url\":\"http://127.0.0.1:{recorder_port}\",\"key\":null}}]"
        ),
    )
    .unwrap();
    // SAFETY: called only while FLEET_ENV is held, so no other thread is
    // reading or writing the environment here.
    unsafe { std::env::set_var("TALARIA_FLEET_DIR", dir) };
}

/// Real services, both of them: Redis carries the minted sessions, and the
/// gateway proxy must reach the recorder, so a dead redis is not an option
/// here (unlike the support module's default app_state).
async fn app_state() -> AppState {
    let cfg = Config::from_parts(
        std::env::var("DATABASE_URL").expect("set DATABASE_URL"),
        std::env::var("REDIS_URL").expect("set REDIS_URL"),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg))
}

/// The speaker this file proves the stamp names. Real name, real email —
/// the assertions are about the WIRE, so the row must carry what a row
/// carries.
async fn speaker(pg: &sqlx::PgPool, tag: &str) -> SessionUser {
    let email = format!("sender-identity-{tag}@link-test.invalid");
    let sub = format!("sender-live:{email}");
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') \
         returning id::text",
    )
    .bind(&sub)
    .bind(&email)
    .bind("Zach Siegel")
    .fetch_one(pg)
    .await
    .unwrap();
    SessionUser {
        id,
        sub,
        email: Some(email),
        name: Some("Zach Siegel".into()),
        picture: None,
        role: "member".into(),
        provider: "google".into(),
    }
}

/// Delete this binary's users; the cascade takes their channels and
/// conversations with them, the direction production deletes run.
async fn reset(pg: &sqlx::PgPool) {
    sqlx::query("delete from users where sub like 'sender-live:%'")
        .execute(pg)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live rig"]
async fn the_chat_turn_carries_the_identity_on_the_wire() {
    let _env_guard = FLEET_ENV.lock();
    let state = app_state().await;
    reset(&state.pg).await;
    let recorder = Recorder::start();
    point_fleet_at(recorder.port, "chat");
    let user = speaker(&state.pg, "chat").await;
    let sid = create_session(&state, &user).await.unwrap();

    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat")
                .header("cookie", format!("talaria_session={sid}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "model": AGENT,
                        "content": "hey — who am I?",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200, "the chat door refused the turn");

    let sent = recorder.await_body(0, Duration::from_secs(15)).await;
    let messages = sent["messages"].as_array().expect("messages on the wire");

    // The stamp: role system, naming the live sender, their id, and the
    // clause whose whole job is the memory-bleed guard.
    let first = &messages[0];
    assert_eq!(first["role"], "system");
    let content = first["content"].as_str().expect("string content");
    assert!(content.contains("WHO YOU ARE SPEAKING WITH"), "{content}");
    assert!(content.contains("Zach Siegel"), "{content}");
    assert!(content.contains(&user.id), "{content}");
    assert!(
        content.contains("never attribute it to the person speaking now"),
        "{content}"
    );

    // The ordering invariant: identity row first, history after, the live
    // user turn last — and the last turn stays UNPREFIXED (single voice: the
    // name prefixes are the transcript's convention, not identity).
    let last = messages.last().unwrap();
    assert_eq!(last["role"], "user");
    let spoken = last["content"].as_str().expect("string content");
    assert_eq!(spoken, "hey — who am I?", "{spoken}");
    // No OTHER system row may carry the name — the identity rides exactly
    // once, on its own row.
    for m in &messages[1..] {
        let c = m["content"].to_string();
        assert!(
            !c.contains("Zach Siegel"),
            "the name leaked beyond the identity row: {c}"
        );
    }

    reset(&state.pg).await;
}

#[tokio::test]
#[ignore = "needs a live rig"]
async fn the_channel_reply_carries_the_identity_on_the_wire() {
    let _env_guard = FLEET_ENV.lock();
    let state = app_state().await;
    reset(&state.pg).await;
    let recorder = Recorder::start();
    point_fleet_at(recorder.port, "channel");
    let user = speaker(&state.pg, "channel").await;
    let sid = create_session(&state, &user).await.unwrap();

    // A plain rail channel carrying the test agent, the same registration
    // path every channel reply (and every ticket task room) replies through.
    let channel: CreatedChannel =
        create_channel(&state.pg, &user.id, "sender-identity-room", None, "channel")
            .await
            .unwrap();
    let deps = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    add_channel_agent(&deps, &channel.id, AGENT).await.unwrap();

    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/channels/{}/messages", channel.id))
                .header("cookie", format!("talaria_session={sid}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "content": format!("@{AGENT} ping"),
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status().as_u16(),
        200,
        "the channel door refused the post"
    );

    // The agent's reply is a detached spawn — it reaches the recorder on
    // its own schedule, so this is a poll, not a read.
    let sent = recorder.await_body(0, Duration::from_secs(15)).await;
    let messages = sent["messages"].as_array().expect("messages on the wire");

    // The room's own system prompt first, the identity row second, the
    // transcript after — the ordering the stamp was placed to keep.
    assert_eq!(messages[0]["role"], "system");
    assert!(
        messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("group channel"),
        "messages[0] is not the room prompt"
    );
    assert_eq!(messages[1]["role"], "system");
    let identity = messages[1]["content"].as_str().expect("string content");
    assert!(identity.contains("WHO YOU ARE SPEAKING WITH"), "{identity}");
    assert!(identity.contains("Zach Siegel"), "{identity}");
    assert!(identity.contains(&user.id), "{identity}");
    // The transcript follows: the mention itself, prefixed by name — the
    // prefix convention, riding AFTER the identity row that gives it truth.
    assert!(messages.len() > 2, "no transcript followed the stamp");
    let transcript = messages[2]["content"].as_str().unwrap_or_default();
    assert!(
        transcript.contains("@live-test-agent"),
        "the transcript's first turn is not the mention: {transcript}"
    );

    reset(&state.pg).await;
}
