// Real-time fan-out over Redis pub/sub → SSE.
// Mutations publish a small event to a topic
// (`board:<id>`, `channel:<id>`, `run:<id>`, `user:<id>`); each connected
// client holds an SSE stream fed by the process's ONE Redis subscriber (the
// hub, below), which subscribes a topic while any stream wants it and fans
// each message out in-process. Multiplayer without websockets.
//
// The person's own stream (`user:<id>`) is also the one that resumes: a frame
// carrying a notification has an SSE `id:` (its seq), a reconnect with
// `?since=`/`Last-Event-ID` replays what it missed from Postgres, and the
// stream ends — with a `session` frame — when its session does (I-19,
// docs/notifications/live-and-inbox.md).
//
// THE RULE THAT MAKES THIS SAFE, and it is worth stating once at the top
// because two of the topics below are the first ones carrying a thing a person
// might not be allowed to read: AN EVENT SAYS WHAT CHANGED, NEVER WHAT IT SAYS.
// The client re-fetches through the ordinary route, which applies the ordinary
// read ACL. A payload that carried the content would be a second read path with
// no gate on it — the fan-out would become the disclosure.

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use futures_util::stream::{StreamExt, once, unfold};
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use talaria_runs_define::RunState;
use tokio::sync::{broadcast, mpsc, watch};

const LOG: &str = "[realtime]";

// ── The two edges to Redis ───────────────────────────────────────────────────

/// The publish edge: fire-and-forget, errors logged not propagated.
/// A publish that BLOCKED the caller
/// would make every mutation wait on Redis; a publish that ERRORED the caller
/// would fail the write it is announcing, when the write already landed.
pub type PublishFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The subscribe edge: register one client's interest in a topic with the
/// process's hub and hand back the receiving end of its forwarder, once Redis
/// has the subscription (so a read made after this returns cannot miss a
/// publish made after it). NEVER FAILS — while Redis is unreachable the
/// receiver is simply quiet (`: connected`, pings, no events) until the hub
/// reconnects, and it ENDS when the hub drops it: a lagging client, or a
/// shutdown.
pub type SubscribeFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, mpsc::Receiver<String>> + Send + Sync>;

/// Every edge to the outside world, so publish shaping and stream framing are
/// testable with no Redis — the same rule the runs engine follows.
#[derive(Clone)]
pub struct RealtimeDeps {
    pub publish: PublishFn,
    pub subscribe: SubscribeFn,
}

/// The subscribe edge nobody feeds: a receiver that never yields. The streams
/// stay honest — `: connected`, pings, no events — instead of erroring a route
/// over a connection problem the client will retry anyway.
///
/// THE SENDER IS KEPT until the receiver drops. A receiver whose sender is
/// already gone answers `None` at once, which ENDS the stream — the "quiet"
/// stream used to close the moment it opened, and the browser reconnected
/// every `retry:` interval forever. A parked task holding the sender until
/// the client hangs up is what makes quiet mean open.
fn quiet_subscribe() -> SubscribeFn {
    Arc::new(|_| {
        async {
            let (tx, rx) = mpsc::channel::<String>(1);
            tokio::spawn(async move { tx.closed().await });
            rx
        }
        .boxed()
    })
}

impl RealtimeDeps {
    /// Both halves over real Redis: publishes ride the shared
    /// ConnectionManager, streams ride the process's hub for this URL.
    pub fn new(conn: redis::aio::ConnectionManager, redis_url: &str) -> Self {
        Self {
            publish: redis_publish(conn),
            subscribe: hub_subscribe(redis_url),
        }
    }

    /// The stream half alone — what an SSE route needs. The publish edge is a
    /// documented no-op: streams only ever subscribe, and forcing the shared
    /// manager's first-connect handshake on them would make opening a watch
    /// depend on the publisher's connection. Every stream on every route
    /// shares the hub's one subscriber connection; building these deps per
    /// request costs a map lookup.
    pub fn streams_only(redis_url: &str) -> Self {
        Self {
            publish: Arc::new(|_, _| {}),
            subscribe: hub_subscribe(redis_url),
        }
    }

    /// The publish half alone — what a notification write needs. The subscribe
    /// edge is the quiet one: nothing on this plane will ever open a stream.
    /// `None` is Redis unreachable at construction, resolved to a logged
    /// silence — the row underneath is the record, the fan-out is a delivery.
    pub fn publish_only(conn: Option<redis::aio::ConnectionManager>) -> Self {
        Self {
            publish: match conn {
                Some(conn) => redis_publish(conn),
                None => {
                    tracing::error!(
                        "{LOG} no Redis connection for a publisher — its events will not fan out"
                    );
                    Arc::new(|_, _| {})
                }
            },
            subscribe: quiet_subscribe(),
        }
    }
}

fn redis_publish(conn: redis::aio::ConnectionManager) -> PublishFn {
    Arc::new(move |topic: &str, payload: &str| {
        let mut conn = conn.clone();
        let topic = topic.to_string();
        let payload = payload.to_string();
        tokio::spawn(async move {
            // Not swallowed. A fan-out that fails silently is how a device sits
            // on a stale run forever while the row underneath it finished ten
            // minutes ago.
            if let Err(e) = redis::cmd("PUBLISH")
                .arg(&topic)
                .arg(&payload)
                .query_async::<()>(&mut conn)
                .await
            {
                tracing::error!("{LOG} publish {topic} failed: {e}");
            }
        });
    })
}

// ── The hub: one subscriber connection per process ──────────────────────────
//
// WAVE 1 OPENED A REDIS CONNECTION PER STREAM. Every open tab held one for its
// user stream, one per board or channel it watched, and one per run pane, so
// the connection count was the number of browser tabs times the number of
// views (SE-7). The hub replaces that with ONE pubsub connection per process
// and per Redis URL:
//
//   · a topic is SUBSCRIBEd while at least one stream wants it, and
//     UNSUBSCRIBEd when the last one leaves (refcounted in `topics`);
//   · each topic fans out in-process over a `broadcast` channel of 256
//     messages — a stream that falls further behind than that has LAGGED, and
//     it ENDS (its client reconnects, and the user stream resumes by seq);
//   · one driver task owns the connection. It applies the subscription set
//     (one channel per command, so every reply belongs to the request that
//     sent it — a multi-channel SUBSCRIBE answers once per channel, and the
//     client resolves requests by reply order), reads messages, PINGs every
//     30 s so a half-open socket is noticed, and when the connection drops it
//     reconnects with backoff (immediately, then 100 ms doubling to 5 s),
//     resubscribes every live topic and tells every user stream to resync
//     (`HUB_RECONNECTED`): whatever was published in the gap is gone from
//     pub/sub, and only a `?since` read recovers it;
//   · `end_streams` closes it: every topic's channel drops, so every stream
//     built on the hub ends, the route-local ones included.
//
// The publish edge is untouched: publishes ride the shared ConnectionManager.

/// Messages a topic buffers per stream before that stream has lagged.
const TOPIC_BUFFER: usize = 256;

/// How long a new subscriber waits for Redis to confirm its topic before it
/// goes ahead anyway. Only reached while Redis is down; the hub's reconnect
/// then sends the user streams a resync.
const LIVE_WAIT: Duration = Duration::from_secs(3);

/// The bound on one SUBSCRIBE/UNSUBSCRIBE/PING round trip; past it the
/// connection is treated as dead and replaced.
const HUB_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

const HUB_PING_EVERY: Duration = Duration::from_secs(30);
const HUB_BACKOFF_MIN: Duration = Duration::from_millis(100);
const HUB_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// The in-band marker the hub sends every `user:` topic after a reconnect —
/// never a Redis payload (every publisher here sends a JSON object), and
/// turned into a `resync` frame by the user stream before anything reads it.
pub const HUB_RECONNECTED: &str = "\u{1}hub:reconnected";

/// The topic logout publishes a session's hash on, so every stream that
/// session opened — on any api process — ends at once.
pub const SESSION_REVOKED_TOPIC: &str = "session:revoked";

struct TopicEntry {
    tx: broadcast::Sender<Arc<str>>,
    refs: usize,
    /// True while Redis holds the subscription.
    live: watch::Sender<bool>,
}

struct Hub {
    topics: Mutex<HashMap<String, TopicEntry>>,
    /// Topics whose wanted state changed; the driver reconciles each.
    wake: mpsc::UnboundedSender<String>,
    closed: AtomicBool,
}

static HUBS: std::sync::LazyLock<Mutex<HashMap<String, Arc<Hub>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Hub {
    fn new() -> (Arc<Hub>, mpsc::UnboundedReceiver<String>) {
        let (wake, wake_rx) = mpsc::unbounded_channel();
        (
            Arc::new(Hub {
                topics: Mutex::new(HashMap::new()),
                wake,
                closed: AtomicBool::new(false),
            }),
            wake_rx,
        )
    }

    /// The process's hub for `url`, started on first use. `None` when the URL
    /// does not parse (every stream then opens quiet, as before) or the
    /// process is shutting down.
    fn for_url(url: &str) -> Option<Arc<Hub>> {
        let mut hubs = lock(&HUBS);
        let shutting_down = *STREAMS_END.borrow();
        if let Some(h) = hubs.get(url) {
            // A closed hub outside a shutdown lost its driver (the runtime it
            // ran on ended, or it panicked): its streams have ended, and the
            // next one gets a fresh hub instead of a dead one.
            if !h.closed.load(Ordering::SeqCst) || shutting_down {
                return Some(h.clone());
            }
            tracing::warn!("{LOG} the hub's driver is gone — starting a new one");
        }
        if shutting_down {
            return None;
        }
        let client = match redis::Client::open(url) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("{LOG} cannot parse REDIS_URL — every stream will open quiet: {e}");
                return None;
            }
        };
        let (hub, wake_rx) = Hub::new();
        hubs.insert(url.to_string(), hub.clone());
        // The guard rides the spawned future itself, so it drops with it
        // whether or not the driver was ever polled.
        let close = CloseOnDrop(hub.clone());
        let driver = drive(hub.clone(), client, wake_rx);
        tokio::spawn(async move {
            let _close = close;
            driver.await;
        });
        Some(hub)
    }

    /// Count one more stream on `topic`: its receiver (made BEFORE Redis is
    /// asked, so nothing published after the subscription lands is missed)
    /// and the topic's live flag. `None` once the hub is closed.
    fn register(
        &self,
        topic: &str,
    ) -> Option<(broadcast::Receiver<Arc<str>>, watch::Receiver<bool>)> {
        let mut topics = lock(&self.topics);
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }
        let fresh = !topics.contains_key(topic);
        let entry = topics
            .entry(topic.to_string())
            .or_insert_with(|| TopicEntry {
                tx: broadcast::channel(TOPIC_BUFFER).0,
                refs: 0,
                live: watch::channel(false).0,
            });
        entry.refs += 1;
        let out = (entry.tx.subscribe(), entry.live.subscribe());
        drop(topics);
        if fresh {
            let _ = self.wake.send(topic.to_string());
        }
        Some(out)
    }

    /// One stream left `topic`; the last one out asks the driver to
    /// UNSUBSCRIBE.
    fn release(&self, topic: &str) {
        let mut topics = lock(&self.topics);
        let Some(entry) = topics.get_mut(topic) else {
            return; // closed: the map was cleared
        };
        entry.refs = entry.refs.saturating_sub(1);
        if entry.refs == 0 {
            topics.remove(topic);
            drop(topics);
            let _ = self.wake.send(topic.to_string());
        }
    }

    /// Streams on `topic` right now (diagnostics and tests).
    fn refs(&self, topic: &str) -> usize {
        lock(&self.topics).get(topic).map_or(0, |e| e.refs)
    }

    fn wanted(&self) -> Vec<String> {
        lock(&self.topics).keys().cloned().collect()
    }

    fn set_live(&self, topic: &str, live: bool) {
        if let Some(e) = lock(&self.topics).get(topic) {
            e.live.send_replace(live);
        }
    }

    fn all_not_live(&self) {
        for e in lock(&self.topics).values() {
            e.live.send_replace(false);
        }
    }

    fn deliver(&self, topic: &str, payload: Arc<str>) {
        if let Some(e) = lock(&self.topics).get(topic) {
            // No receiver is not an error: the last stream may have left
            // while its UNSUBSCRIBE was in flight.
            let _ = e.tx.send(payload);
        }
    }

    /// After a reconnect: every user stream resyncs.
    fn announce_reconnect(&self) {
        let marker: Arc<str> = Arc::from(HUB_RECONNECTED);
        for (topic, e) in lock(&self.topics).iter() {
            if topic.starts_with("user:") {
                let _ = e.tx.send(marker.clone());
            }
        }
    }

    /// Shutdown: drop every topic's channel, so every forwarder — and every
    /// stream built on one — ends.
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        lock(&self.topics).clear();
    }

    /// The subscribe edge itself: register, wait (bounded) for Redis, then
    /// forward to an mpsc receiver the routes already know how to read.
    async fn subscribe(self: Arc<Self>, topic: String) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel::<String>(16);
        let Some((mut messages, mut live)) = self.register(&topic) else {
            return rx; // closed: the sender drops here, and the stream ends
        };
        let guard = Release {
            hub: self.clone(),
            topic,
        };
        let live_now = tokio::time::timeout(LIVE_WAIT, live.wait_for(|l| *l))
            .await
            .is_ok_and(|r| r.is_ok());
        // Redis did not confirm the topic in time (it is slow, or the hub is
        // reconnecting): whatever was published before it does is not this
        // stream's, so a user stream is told to resync the moment it is.
        let resync_when_live = !live_now && guard.topic.starts_with("user:");
        tokio::spawn(async move {
            let release = guard;
            if resync_when_live {
                // The flag's guard must not live across the send below.
                let became = tokio::select! {
                    _ = tx.closed() => return,
                    became = live.wait_for(|l| *l) => became.is_ok(),
                };
                if !became || tx.send(HUB_RECONNECTED.to_string()).await.is_err() {
                    return;
                }
            }
            loop {
                // THE CLIENT LEAVING ENDS THE FORWARDER, not the next publish
                // (wave 1's fix, kept): `tx.closed()` fires the moment the
                // receiver drops, and the release unsubscribes the topic when
                // this was its last stream.
                tokio::select! {
                    _ = tx.closed() => break,
                    msg = messages.recv() => match msg {
                        Ok(p) => {
                            if tx.send(p.to_string()).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                "{LOG} a stream on {} fell {n} messages behind — ending it; its client reconnects",
                                release.topic
                            );
                            break;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        });
        rx
    }
}

/// Decrements a topic's count when its forwarder ends, however it ends.
struct Release {
    hub: Arc<Hub>,
    topic: String,
}

impl Drop for Release {
    fn drop(&mut self) {
        self.hub.release(&self.topic);
    }
}

fn hub_subscribe(redis_url: &str) -> SubscribeFn {
    let url = redis_url.to_string();
    Arc::new(move |topic: String| {
        let hub = Hub::for_url(&url);
        async move {
            match hub {
                Some(hub) => hub.subscribe(topic).await,
                // An unparseable URL keeps wave 1's quiet stream; a closed
                // hub (shutdown) answers a receiver that ends at once.
                None if STREAMS_END.borrow().to_owned() => mpsc::channel::<String>(1).1,
                None => quiet_subscribe()(topic).await,
            }
        }
        .boxed()
    })
}

/// Streams on `topic` in this process's hub for `redis_url` — what the live
/// tests read beside `PUBSUB NUMSUB`.
pub fn hub_refs(redis_url: &str, topic: &str) -> usize {
    lock(&HUBS).get(redis_url).map_or(0, |h| h.refs(topic))
}

/// Topics this process's hub for `redis_url` holds right now.
pub fn hub_topics(redis_url: &str) -> usize {
    lock(&HUBS).get(redis_url).map_or(0, |h| h.wanted().len())
}

async fn bounded<T>(
    f: impl std::future::Future<Output = redis::RedisResult<T>>,
) -> Result<T, String> {
    match tokio::time::timeout(HUB_COMMAND_TIMEOUT, f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out".into()),
    }
}

/// Closes the hub when its driver ends, however it ends — a shutdown, a
/// panic, or the runtime it was spawned on going away — so no stream is left
/// on a hub nothing feeds.
struct CloseOnDrop(Arc<Hub>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// The driver: connect, apply the subscription set, read, and on any failure
/// reconnect and resubscribe. Ends on shutdown, dropping the connection.
async fn drive(hub: Arc<Hub>, client: redis::Client, mut wake: mpsc::UnboundedReceiver<String>) {
    let mut backoff = Duration::ZERO;
    // A gap during which a subscribed stream could have missed a publish: a
    // dropped connection, or a connect that failed while streams waited.
    let mut gap = false;
    loop {
        if hub.closed.load(Ordering::SeqCst) {
            return;
        }
        if !backoff.is_zero() {
            tokio::select! {
                _ = streams_ending() => return,
                _ = tokio::time::sleep(backoff) => {}
            }
        }
        backoff = (backoff * 2).clamp(HUB_BACKOFF_MIN, HUB_BACKOFF_MAX);
        let pubsub = match bounded(client.get_async_pubsub()).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("{LOG} hub could not connect (retrying in {backoff:?}): {e}");
                gap = true;
                continue;
            }
        };
        let (mut sink, mut messages) = pubsub.split();
        // Every topic wanted right now, one command each. Wakes queued
        // meanwhile are reconciled after; reconciling is idempotent.
        let mut actual: HashSet<String> = HashSet::new();
        let mut failed = None;
        for topic in hub.wanted() {
            match bounded(sink.subscribe(&topic)).await {
                Ok(()) => {
                    actual.insert(topic.clone());
                    hub.set_live(&topic, true);
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = failed {
            tracing::warn!("{LOG} hub could not resubscribe (reconnecting): {e}");
            hub.all_not_live();
            gap = true;
            continue;
        }
        if gap {
            tracing::info!(
                "{LOG} hub reconnected — {} topic(s) resubscribed, user streams told to resync",
                actual.len()
            );
            hub.announce_reconnect();
        }
        backoff = Duration::ZERO;
        let mut ping =
            tokio::time::interval_at(tokio::time::Instant::now() + HUB_PING_EVERY, HUB_PING_EVERY);
        let why: String = loop {
            tokio::select! {
                biased;
                _ = streams_ending() => return,
                topic = wake.recv() => {
                    let Some(topic) = topic else { return };
                    let wanted = lock(&hub.topics).contains_key(&topic);
                    let result = match (wanted, actual.contains(&topic)) {
                        (true, false) => bounded(sink.subscribe(&topic)).await.map(|()| {
                            actual.insert(topic.clone());
                        }),
                        (false, true) => bounded(sink.unsubscribe(&topic)).await.map(|()| {
                            actual.remove(&topic);
                        }),
                        _ => Ok(()),
                    };
                    if let Err(e) = result {
                        break format!("subscription change for {topic} failed: {e}");
                    }
                    if wanted {
                        hub.set_live(&topic, true);
                    }
                }
                msg = messages.next() => {
                    let Some(msg) = msg else {
                        break "the connection closed".to_string();
                    };
                    if let Ok(payload) = msg.get_payload::<String>() {
                        hub.deliver(msg.get_channel_name(), Arc::from(payload));
                    }
                }
                _ = ping.tick() => {
                    if let Err(e) = bounded(sink.ping::<redis::Value>()).await {
                        break format!("ping failed: {e}");
                    }
                }
            }
        };
        tracing::warn!("{LOG} hub lost its Redis connection ({why}) — reconnecting");
        hub.all_not_live();
        gap = true;
    }
}

// ── board:<id> and channel:<id> ──────────────────────────────────────────────

/// What goes over `board:<id>`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardEvent {
    #[serde(rename = "type")]
    pub kind_tag: &'static str, // "task" | "comment" | "board"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
}

pub fn publish_board(deps: &RealtimeDeps, board_id: &str, event: &BoardEvent) {
    let payload = serde_json::to_string(event).expect("typed struct serializes");
    (deps.publish)(&format!("board:{board_id}"), &payload);
}

/// What goes over `channel:<id>`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelEvent {
    #[serde(rename = "type")]
    pub kind_tag: &'static str, // "message" | "channel"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
}

pub fn publish_channel(deps: &RealtimeDeps, channel_id: &str, event: &ChannelEvent) {
    let payload = serde_json::to_string(event).expect("typed struct serializes");
    (deps.publish)(&format!("channel:{channel_id}"), &payload);
}

/// What goes over `channel:<id>` while a member types — `{type:"typing",
/// userId, typing}`, plus `threadRootId` when they are typing a thread reply
/// (the thread panel shows it; the channel's own composer line does not). Its own struct rather than fields on `ChannelEvent`:
/// typing is ephemeral presence, never stored and never re-read, and keeping
/// it apart means no persisted event can grow a "who is typing" field by
/// accident. Clients expire a `typing: true` on their own after a few seconds
/// of silence, so a lost `typing: false` costs a few seconds of dots, not a
/// stuck indicator.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypingEvent {
    #[serde(rename = "type")]
    pub kind_tag: &'static str, // always "typing"
    pub user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_root_id: Option<String>,
    pub typing: bool,
}

pub fn publish_typing(
    deps: &RealtimeDeps,
    channel_id: &str,
    user_id: &str,
    thread_root_id: Option<&str>,
    typing: bool,
) {
    let event = TypingEvent {
        kind_tag: "typing",
        user_id: user_id.to_string(),
        thread_root_id: thread_root_id.map(str::to_string),
        typing,
    };
    let payload = serde_json::to_string(&event).expect("typed struct serializes");
    (deps.publish)(&format!("channel:{channel_id}"), &payload);
}

/// An SSE response of a board's events.
pub async fn board_event_stream(deps: &RealtimeDeps, board_id: &str) -> Response {
    sse_response((deps.subscribe)(format!("board:{board_id}")).await)
}

/// An SSE response of a chat channel's events.
pub async fn channel_event_stream(deps: &RealtimeDeps, channel_id: &str) -> Response {
    sse_response((deps.subscribe)(format!("channel:{channel_id}")).await)
}

// ── Runs: `run:<id>` and `user:<id>` ─────────────────────────────────────────
//
// TWO TOPICS BECAUSE THERE ARE TWO QUESTIONS, and they have different answers
// about who may listen:
//
//   run:<id>    "how is THIS run doing" — a detail view, a progress strip. Its
//               gate is the run's READ ACL (see `may_watch_run`).
//   user:<id>   "what do I have in flight" — the per-user firehose a SECOND
//               DEVICE attaches to. Its gate is identity: it is your own id and
//               nobody else's, which is the only reason it can be one topic per
//               person rather than one per thing.
//
// THE SECOND ONE is the point of the whole file for runs. A run is durable
// server state, so "I started it on my laptop and I am now on my phone" has to
// be a live view and not a poll — and it must update for a run the person is
// NOT looking at, because the transition that matters most (`awaiting`: this
// run has stopped and is asking you something) is exactly the one that happens
// while their attention is elsewhere.
//
// Notifications publish here too (`add_notification` → `{type:'notification'}`,
// the bell's signal) alongside runs and the brief — which is what the topic was
// NAMED for: it is the person's, not any one feature's.

/// What goes over `run:<id>`.
///
/// Deliberately the run's OWN fields and nothing else. Compare `RunEvent` in
/// runs/run.rs, which additionally carries the `DecisionRequest` a run parked
/// on — that one field is NOT published here and must not be:
///
/// WHO MAY READ THE QUESTION IS A DIFFERENT QUESTION FROM WHO MAY WATCH THE
/// RUN. A decision's text is governed by the definition's `audience`, an
/// approvals `Authority`; this stream is gated by the run's read ACL, which is
/// its owner plus whoever may read its SUBJECT. Those overlap in the common
/// case and they are not the same set — a run about a board whose audience is
/// `{ by: 'user' }` would hand every board member the question's `detail` if
/// this payload carried it. A device that sees `state: 'awaiting'` re-fetches
/// the run through the route that resolves the audience properly, and pays one
/// round trip on the rare transition rather than routing content around the
/// resolver on every transition.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunWireEvent {
    #[serde(rename = "type")]
    pub kind_tag: &'static str, // always "run"
    pub run_id: String,
    pub kind: String,
    pub state: RunState,
    /// The human sentence a waiting person reads. Governed by the same read ACL
    /// as the run itself, which is what this topic is gated on.
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What goes over `user:<id>`. STRICTLY ID-SHAPED — ids, and closed enums that
/// are ids of a sort. No phase, no error text, no question.
///
/// Not because this topic is less trusted (it is the person's own firehose)
/// but because it is the one place a fan-out could quietly acquire a second
/// disclosure rule: a `run` event here is published for the run's OWNER, while
/// a run's content is governed by its audience, and the day something publishes
/// a shared run to a participant's user topic the payload must already be
/// incapable of leaking. Keeping it id-shaped from the start means that day is
/// a routing change and not a disclosure review.
///
/// The enum IS the wire shape — serialized directly, with no wider object to
/// accidentally pass.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum UserEvent {
    Run {
        run_id: String,
        state: RunState,
    },
    /// A row landed or re-alerted. `seq` is the person's notify seq the write
    /// took (I-10); absent from a writer that predates the contract. A frame
    /// carrying it gets an SSE `id:` once the hub replays (W2-H).
    Notification {
        notification_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    Brief {
        brief_id: String,
        seq: i64,
    },
    Channel {
        channel_id: String,
    },
    Conversation {
        conversation_id: String,
    },
    /// Someone in one of your conversations (a DM, channel or relay) started
    /// or stopped typing — the Comms rail's dots beside the row. Ids and a
    /// flag only, like everything here. Thread replies don't come this way.
    Typing {
        channel_id: String,
        user_id: String,
        typing: bool,
    },
    /// Your notifications were READ somewhere — a bell click in another tab,
    /// a channel read to its end, a mark-all. `unread` is the count after it,
    /// a number and nothing else, so a second tab's bell drops the moment the
    /// first one clears it instead of on its next poll. Consumers that do not
    /// know it ignore it (the shell's watcher, older pages).
    ///
    /// The v2 fields (I-16), each absent when not known: `unreadForYou` (Needs
    /// you + Direct), `readIds` — exactly the rows just read, at most 100, so
    /// every client can close their OS banners — `all` (a mark-all), and
    /// `resync` (more than 100 were read: refetch instead of reading ids).
    Notifications {
        unread: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        unread_for_you: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        read_ids: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        all: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resync: Option<bool>,
    },
    /// Which of the person's clients is the one they are on (I-17), published
    /// only when that answer changes: the most recent `looking` client, the
    /// most recent `active` one, and how many report at all.
    Presence {
        looking: Option<String>,
        active: Option<String>,
        clients: u32,
    },
    /// The stream's replay state (I-19, implemented by W2-H): every row up to
    /// `seq` was sent (`complete`), or the client must read `?since` itself.
    Resync {
        seq: i64,
        complete: bool,
    },
    /// The session behind this stream ended (`state: "ended"`); the stream
    /// closes after it (W2-H).
    Session {
        state: String,
    },
}

/// Publish a run transition to `run:<id>`.
pub fn publish_run(deps: &RealtimeDeps, run_id: &str, event: &RunWireEvent) {
    let payload = serde_json::to_string(event).expect("typed struct serializes");
    (deps.publish)(&format!("run:{run_id}"), &payload);
}

/// Publish to a person's own firehose.
pub fn publish_user(deps: &RealtimeDeps, user_id: &str, event: &UserEvent) {
    let payload = serde_json::to_string(event).expect("typed struct serializes");
    (deps.publish)(&format!("user:{user_id}"), &payload);
}

/// An SSE response of one run's events. Gate the caller with `may_watch_run`
/// FIRST and to completion — see routes/runs_events.rs for why the order is
/// load-bearing.
pub async fn run_event_stream(deps: &RealtimeDeps, run_id: &str) -> Response {
    sse_response((deps.subscribe)(format!("run:{run_id}")).await)
}

/// An SSE response of one person's own firehose. The only legitimate argument
/// is the authenticated caller's own id. No replay and no session checks: the
/// route builds those (`resumable_event_stream`); this is the bare stream, with
/// `id:` framing and the hub's resync.
pub async fn user_event_stream(deps: &RealtimeDeps, user_id: &str) -> Response {
    let events = (deps.subscribe)(format!("user:{user_id}")).await;
    resumable_event_stream(UserStream::live(events))
}

// ── The person's own stream: replay, resync, the session (I-19) ──────────────
//
// ON THE WIRE (docs/notifications/contract.md, "The live stream"):
//
//   : connected / retry: 3000
//   presence                         the person's presence right now
//   notification × n  (id: <seq>)    every row with seq > since, in seq order
//   resync {seq, complete: true}     ... or, past 100 rows, only
//   resync {seq, complete: false}    "read ?since yourself"
//   live frames                      a `notification` keeps its `id: <seq>`
//
// THE ORDER THAT MAKES A RECONNECT LOSSLESS is the route's: subscribe first
// (the hub answers once Redis holds the topic), then read the replay in one
// snapshot. A write that committed before the snapshot is in the replay, and
// its live frame — published after its commit, so after the subscribe — is
// dropped here as covered (`seq <= covered_through`); a write after the
// snapshot has a higher seq and arrives live. Each row comes exactly once.
//
// AND IT ENDS WITH ITS SESSION. A signed-out or expired session must not keep
// a firehose open (SE-16: a stream delivered a notification 127 ms after its
// `sess:` key was deleted). Three doors, each answering
// `{"type":"session","state":"ended"}` and then ending the stream:
//   · logout publishes the session's hash on `session:revoked` — at once;
//   · every live frame is sent only after the session is re-read, so nothing
//     is delivered to a session that is gone, however it went;
//   · every 20 s the session is re-read anyway — a quiet stream finds out its
//     session expired (the 7-day TTL) or was deleted within one tick.

/// One frame of a user stream before SSE framing: its JSON, and the `id:` it
/// carries (a notification's seq).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFrame {
    pub id: Option<i64>,
    pub data: String,
}

impl UserFrame {
    /// The frame for a typed event; a `notification` with a seq carries it as
    /// its `id:`.
    pub fn of(event: &UserEvent) -> Self {
        let id = match event {
            UserEvent::Notification { seq, .. } => *seq,
            _ => None,
        };
        UserFrame {
            id,
            data: serde_json::to_string(event).expect("typed struct serializes"),
        }
    }

    fn event(&self) -> Event {
        let ev = Event::default();
        let ev = match self.id {
            Some(id) => ev.id(id.to_string()),
            None => ev,
        };
        ev.data(&self.data)
    }
}

/// The seq a live payload's `notification` frame carries, if it is one.
fn notification_seq(payload: &str) -> Option<i64> {
    // Cheap first: only a notification frame is parsed.
    if !payload.contains("\"type\":\"notification\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    (v.get("type")?.as_str()? == "notification")
        .then(|| v.get("seq").and_then(serde_json::Value::as_i64))
        .flatten()
}

/// One pub/sub payload as an SSE event: `data:`, and `id: <seq>` before it for
/// a notification that carries one.
fn data_event(payload: &str) -> Event {
    match notification_seq(payload) {
        Some(seq) => Event::default().id(seq.to_string()).data(payload),
        None => Event::default().data(payload),
    }
}

/// What a session re-read answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Alive,
    /// Gone: deleted, expired, or now somebody else's.
    Ended,
    /// The read failed. Not an ending: the stream carries on, and the next
    /// read decides.
    Unknown,
}

pub type SessionCheckFn = Arc<dyn Fn() -> BoxFuture<'static, SessionState> + Send + Sync>;
pub type SeqNowFn = Arc<dyn Fn() -> BoxFuture<'static, Option<i64>> + Send + Sync>;

/// A `SessionCheckFn` from a plain async closure.
pub fn session_check_fn<F, Fut>(f: F) -> SessionCheckFn
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = SessionState> + Send + 'static,
{
    Arc::new(move || f().boxed())
}

/// A `SeqNowFn` from a plain async closure.
pub fn seq_now_fn<F, Fut>(f: F) -> SeqNowFn
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Option<i64>> + Send + 'static,
{
    Arc::new(move || f().boxed())
}

/// What a user stream is built from. The route subscribes, reads the replay,
/// and hands both here.
pub struct UserStream {
    /// `user:<id>`, subscribed — and live in Redis — before the replay was read.
    pub events: mpsc::Receiver<String>,
    /// `session:revoked`, and the hash of the session this stream belongs to.
    pub revoked: Option<(mpsc::Receiver<String>, String)>,
    /// Sent first, in order: presence, then the replay and its resync.
    pub preamble: Vec<UserFrame>,
    /// Live `notification` frames at or below this seq were covered by the
    /// replay (or will be by the client's own `?since` read): dropped.
    pub covered_through: Option<i64>,
    /// Re-reads the session — before each live frame, and every tick.
    pub session: Option<SessionCheckFn>,
    /// The person's seq now, for the resync after a hub reconnect.
    pub seq_now: Option<SeqNowFn>,
    /// How often a quiet stream re-reads its session (20 s, the ping's
    /// interval; tests shorten it).
    pub session_every: Duration,
}

impl UserStream {
    /// The bare stream: live frames only.
    pub fn live(events: mpsc::Receiver<String>) -> Self {
        UserStream {
            events,
            revoked: None,
            preamble: Vec::new(),
            covered_through: None,
            session: None,
            seq_now: None,
            session_every: Duration::from_millis(SESSION_CHECK_EVERY_MS),
        }
    }
}

/// The `session` frame a stream ends with.
fn session_ended() -> Event {
    UserFrame::of(&UserEvent::Session {
        state: "ended".into(),
    })
    .event()
}

/// How often a user stream re-reads its session while nothing else does.
const SESSION_CHECK_EVERY_MS: u64 = PING_EVERY_MS;

struct UserState {
    events: mpsc::Receiver<String>,
    revoked: Option<(mpsc::Receiver<String>, String)>,
    pending: VecDeque<Event>,
    covered_through: Option<i64>,
    /// The highest seq this stream has carried — the resync's fallback when
    /// the seq read fails.
    last_seq: Option<i64>,
    session: Option<SessionCheckFn>,
    seq_now: Option<SeqNowFn>,
    tick: tokio::time::Interval,
    done: bool,
}

enum UserStep {
    Live(Option<String>),
    Revoked(Option<String>),
    Tick,
}

async fn next_revoked(revoked: &mut Option<(mpsc::Receiver<String>, String)>) -> Option<String> {
    match revoked {
        Some((rx, _)) => rx.recv().await,
        None => std::future::pending().await,
    }
}

impl UserState {
    /// Ends the stream with the `session` frame.
    fn end_session(&mut self) {
        self.pending.push_back(session_ended());
        self.done = true;
    }

    async fn session_alive(&self) -> bool {
        match &self.session {
            Some(check) => check().await != SessionState::Ended,
            None => true,
        }
    }

    async fn step(&mut self) {
        let step = tokio::select! {
            biased;
            hash = next_revoked(&mut self.revoked) => UserStep::Revoked(hash),
            payload = self.events.recv() => UserStep::Live(payload),
            _ = self.tick.tick(), if self.session.is_some() => UserStep::Tick,
        };
        match step {
            // The subscription ended — a lag, the hub closing, a shutdown —
            // and so does the stream: the client reconnects with its seq.
            UserStep::Live(None) => self.done = true,
            UserStep::Live(Some(p)) if p == HUB_RECONNECTED => {
                let seq = match &self.seq_now {
                    Some(f) => f().await.or(self.last_seq),
                    None => self.last_seq,
                };
                self.pending.push_back(
                    UserFrame::of(&UserEvent::Resync {
                        seq: seq.unwrap_or(0),
                        complete: false,
                    })
                    .event(),
                );
            }
            UserStep::Live(Some(p)) => {
                let seq = notification_seq(&p);
                if let (Some(seq), Some(covered)) = (seq, self.covered_through)
                    && seq <= covered
                {
                    return; // already sent by the replay
                }
                if !self.session_alive().await {
                    return self.end_session();
                }
                if let Some(seq) = seq {
                    self.last_seq = Some(self.last_seq.map_or(seq, |l| l.max(seq)));
                }
                self.pending.push_back(data_event(&p));
            }
            UserStep::Revoked(Some(hash)) => {
                if self.revoked.as_ref().is_some_and(|(_, mine)| *mine == hash) {
                    self.end_session();
                }
            }
            // The revocation topic ended on its own (a lag): the per-frame
            // and per-tick checks still hold, so the stream carries on.
            UserStep::Revoked(None) => self.revoked = None,
            UserStep::Tick => {
                if !self.session_alive().await {
                    self.end_session();
                }
            }
        }
    }
}

fn user_frames(s: UserStream) -> impl futures_util::Stream<Item = Result<Event, Infallible>> {
    let every = s.session_every;
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let last_seq = s
        .preamble
        .iter()
        .filter_map(|f| f.id)
        .max()
        .or(s.covered_through);
    let state = UserState {
        events: s.events,
        revoked: s.revoked,
        pending: s.preamble.iter().map(UserFrame::event).collect(),
        covered_through: s.covered_through,
        last_seq,
        session: s.session,
        seq_now: s.seq_now,
        tick,
        done: false,
    };
    once(async { Ok::<Event, Infallible>(connected()) }).chain(unfold(state, |mut st| async move {
        loop {
            if let Some(ev) = st.pending.pop_front() {
                return Some((Ok::<Event, Infallible>(ev), st));
            }
            if st.done {
                return None;
            }
            st.step().await;
        }
    }))
}

/// A user stream as the SSE response every event route sends (headers,
/// `retry:`, pings, the end on shutdown).
pub fn resumable_event_stream(s: UserStream) -> Response {
    sse_wrap(user_frames(s).take_until(streams_ending()))
}

// ── SSE framing ──────────────────────────────────────────────────────────────

/// How long a browser waits before its own reconnect after a stream ends
/// (the `retry:` field). Matches the EventSource default most engines use,
/// stated so an intermediary or an engine with another default agrees.
const RETRY_MS: u64 = 3_000;

/// The idle gap after which a `: ping` comment goes out. Cloudflare — the
/// edge every real instance sits behind — cuts a connection that has carried
/// nothing for about 100 s, and so do several corporate proxies at 60 s;
/// 20 s keeps every one of them seeing traffic well inside its window.
const PING_EVERY_MS: u64 = 20_000;

fn connected() -> Event {
    Event::default()
        .comment("connected")
        .retry(Duration::from_millis(RETRY_MS))
}

/// The topic stream's frames: the `: connected` preamble (carrying the
/// `retry:` field), then one `data:` frame per pub/sub message (a
/// notification's with its `id:`). Payloads are one line by construction —
/// `serde_json::to_string` never emits a literal newline, and axum asserts on
/// one rather than corrupting the stream.
fn topic_frames(
    rx: mpsc::Receiver<String>,
) -> impl futures_util::Stream<Item = Result<Event, Infallible>> {
    once(async { Ok::<Event, Infallible>(connected()) }).chain(unfold(rx, |mut rx| async move {
        // `recv` ending (every sender dropped) is the stream ending — the
        // subscriber is gone and the response finishes, which is how the
        // client learns to reconnect with a fresh one.
        loop {
            let payload = rx.recv().await?;
            // The hub's in-band marker is a user-stream concern; a topic
            // stream never forwards it.
            if payload != HUB_RECONNECTED {
                return Some((Ok::<Event, Infallible>(data_event(&payload)), rx));
            }
        }
    }))
}

/// The SSE response the event routes build: `text/event-stream` (set by `Sse`
/// itself), `Cache-Control: no-cache, no-transform`, `X-Accel-Buffering: no`,
/// a `retry:` at open, and a 20 s `: ping` keep-alive.
///
/// THE HEADERS ARE FOR WHAT SITS BETWEEN. `no-transform` stops a proxy from
/// compressing (and so buffering) the stream; `X-Accel-Buffering: no` is the
/// nginx-family switch Cloudflare and most reverse proxies honor for "flush
/// every chunk". Without them a frame can sit in an edge buffer until enough
/// bytes pile up behind it, which for a stream of one id per minute is never.
///
/// axum pings only when the stream has been IDLE for the interval — any data
/// frame resets the timer, doing the same job a ping does for an
/// intermediary; no `Connection: keep-alive` header is set, because HTTP/1.1
/// defaults to keep-alive and hyper owns that header; and a stream whose
/// subscriber dies ENDS (the forwarder's sender drops, the receiver closes)
/// rather than pinging into the void until the client gives up — an
/// EventSource reconnects on stream end and gets a fresh subscriber, recovery
/// from the server's own initiative.
///
/// AND IT ENDS ON SHUTDOWN (`end_streams`): a graceful shutdown waits for
/// every response to finish, and an event stream never does on its own — so
/// a SIGTERM left the process serving open streams, and ticking its
/// scheduler, until the container's stop timeout killed it.
fn sse_response(rx: mpsc::Receiver<String>) -> Response {
    sse_response_until(rx, streams_ending())
}

/// Flipped once, by the binary's shutdown signal.
static STREAMS_END: std::sync::LazyLock<tokio::sync::watch::Sender<bool>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(false).0);

/// End every open event stream, and every one opened after: the process is
/// shutting down. Each client sees a clean end and reconnects after the
/// `retry:` it was given — to the instance that replaces this one. Called by
/// the binary when the shutdown signal lands, before the graceful drain
/// waits on in-flight responses.
///
/// The hub closes with it: every topic's channel drops, so a stream a route
/// built on the subscribe edge by hand (the run-watch tail, which has no
/// shutdown hook of its own) ends too, and the subscriber connection goes.
pub fn end_streams() {
    STREAMS_END.send_replace(true);
    let hubs: Vec<Arc<Hub>> = lock(&HUBS).values().cloned().collect();
    for hub in hubs {
        hub.close();
    }
}

/// Resolves once `end_streams` has run (at once, if it already has).
fn streams_ending() -> impl std::future::Future<Output = ()> + Send + 'static {
    let mut rx = STREAMS_END.subscribe();
    async move {
        let _ = rx.wait_for(|ended| *ended).await;
    }
}

fn sse_response_until(
    rx: mpsc::Receiver<String>,
    until: impl std::future::Future<Output = ()> + Send + 'static,
) -> Response {
    sse_wrap(topic_frames(rx).take_until(until))
}

fn sse_wrap(
    frames: impl futures_util::Stream<Item = Result<Event, Infallible>> + Send + 'static,
) -> Response {
    let mut res = Sse::new(frames)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_millis(PING_EVERY_MS))
                .text("ping"),
        )
        .into_response();
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    res.headers_mut().insert(
        axum::http::HeaderName::from_static("x-accel-buffering"),
        axum::http::HeaderValue::from_static("no"),
    );
    res
}

/// Tell every stream the session behind `session_hash` opened that it has
/// ended (logout). Rides `session:revoked`, so every api process hears it.
pub fn publish_session_revoked(deps: &RealtimeDeps, session_hash: &str) {
    (deps.publish)(SESSION_REVOKED_TOPIC, session_hash);
}

// ── Who may watch a run ──────────────────────────────────────────────────────
//
// THIS IS THE WHOLE RISK OF THE TWO RUN TOPICS. `user:<id>` gates itself — it
// is the caller's own id. `run:<id>` does not: a run id is an opaque string a
// caller can guess at or hold onto after losing access, and the topic name
// carries no authority of its own.
//
// IT DERIVES FROM WHAT ALREADY DECIDES, and invents nothing:
//
//   the run's OWNER            your own run is yours to watch
//   the run's SUBJECT          `boardRole` for a board or a task, `channelRole`
//                              for a channel, `accessibleConversation` for a
//                              conversation — the same predicates the routes
//                              that SERVE those subjects already call
//
//   ORG-WIDE WORK          no owner AND no subject — an ADMIN. See below.
//
// and where it cannot answer — a subject type nobody taught it — IT REFUSES.
// That direction is not a coin flip: a refusal on a run somebody should have
// seen is a missing live update they can still get by reloading, while an
// allow on a run they should not have seen is a disclosure with no floor under
// it, because a `subject_type` is free text by design and a future run
// definition can
// introduce one at any time without touching this file. Defaulting the unknown
// case to "allow" would make every new subject type a silent widening.
//
// ── THE ORG-WIDE CASE, argued ────────────────────────────────────────────────
//
// A run with NO OWNER AND NO SUBJECT is not an edge case, it is a whole class
// of this system's work: `fitness-sweep`, `rag-reindex` and `rag-backfill` are
// all exactly that shape. Nobody's run, about no ticket, started from Admin.
// Under the original rule the refusal above caught every one of them, so
// NOBODY — not even the admin who pressed the button — could attach to
// /api/runs/:id/events for a fitness sweep or a retrieval migration. The
// durable, attachable, survives-a-deploy progress view existed and had no
// audience. That is a bug, and the widening is deliberate rather than
// convenient.
//
// IT IS NOT "ADMINS MAY READ ANYTHING". Three things keep it narrow, and all
// three are load-bearing:
//
//   · BOTH halves must be absent. A run with an owner stays that person's
//     (an admin watching somebody's research run is refused here, exactly as
//     it was), and a run with a subject is still governed by that subject's
//     read ACL (an admin with no membership of a private board still cannot
//     watch its ticket's work session).
//   · Ownerless-and-subjectless is not a gap in the ACL, it is a COMPLETE
//     description: there is no owner to ask and no subject to resolve, and the
//     three definitions of that shape all declare `audience: () => ({ by:
//     'admin' })` independently. This makes the watch gate agree with what the
//     definitions already say instead of contradicting them.
//   · What is disclosed is the run's `phase` — "rebuilding rag_docs (3 of 7)",
//     "tier 2: 41 of 70 cases" — which is operational text about the
//     deployment's own infrastructure, and which the same admin already reads
//     synchronously from /api/admin/rag and /api/admin/model-fitness. This is
//     the live version of a page they can already open, not a new one.
//
// The gate still is NOT the definition's `audience`. That answers "who may
// DECIDE when this pauses"; this answers "who may WATCH", and it must be
// answerable for a run whose `kind` is not registered on this instance at all
// (a row enqueued by a newer deploy). Conflating the two would give a run with
// an `{ by: 'admin' }` audience a stream every admin could attach to, which is
// precisely the "being an admin is not a read grant" mistake the invariants
// watch for elsewhere.

/// Why a watch was refused. Reported rather than collapsed to a boolean so the
/// unknown-subject case is distinguishable in a log — it is the one that means
/// THIS FILE IS OUT OF DATE, not that the caller did anything wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunWatchVerdict {
    Ok,
    Missing,
    NotAudience,
    UnknownSubject,
}

/// The three columns of a run row the gate reads. `PgRunStore::get` answers
/// with the whole `RunRow`; this is the projection so a fake can answer
/// without inventing a row.
#[derive(Debug, Clone)]
pub struct RunWatchRow {
    pub owner_user_id: Option<String>,
    pub subject_type: Option<String>,
    pub subject_id: Option<String>,
}

pub type GetRunFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<Option<RunWatchRow>, String>> + Send + Sync>;
pub type RoleFn =
    Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<Option<String>, String>> + Send + Sync>;
pub type TaskBoardFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<Option<String>, String>> + Send + Sync>;
pub type AccessFn =
    Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<bool, String>> + Send + Sync>;
pub type AdminFn = Arc<dyn Fn(String) -> BoxFuture<'static, Result<bool, String>> + Send + Sync>;

/// Every edge to the outside world, so the predicate is testable with no
/// database — the same rule the runs engine follows.
pub struct RunWatchDeps {
    pub get_run: GetRunFn,
    pub board_role: RoleFn,
    pub channel_role: RoleFn,
    pub task_board_id: TaskBoardFn,
    pub conversation_access: AccessFn,
    /// ONLY consulted for the org-wide shape (no owner, no subject). Kept as a
    /// dep rather than inlined so the widening is visible in every test's fake
    /// and so a future caller cannot reach admin-ness through this file by
    /// accident.
    pub is_admin: AdminFn,
}

/// May this person attach to `run:<id>`? An `Err` is an edge failing — kept
/// an error rather than converted to a refusal, because a broken predicate
/// and an answered refusal are different facts.
pub async fn may_watch_run(
    user_id: &str,
    run_id: &str,
    deps: &RunWatchDeps,
) -> Result<RunWatchVerdict, String> {
    let run = match (deps.get_run)(run_id.to_string()).await? {
        Some(run) => run,
        None => return Ok(RunWatchVerdict::Missing),
    };
    // An empty string is as absent as a null in all three of these columns,
    // and every branch below reads the filtered Options.
    let owner = run.owner_user_id.as_deref().filter(|s| !s.is_empty());
    let subject_type = run.subject_type.as_deref().filter(|s| !s.is_empty());
    let subject_id = run.subject_id.as_deref().filter(|s| !s.is_empty());

    if owner == Some(user_id) {
        return Ok(RunWatchVerdict::Ok);
    }

    // ORG-WIDE WORK: nobody's run, about nothing in particular — a fitness
    // sweep, a retrieval migration. See the argument above the deps: both
    // halves must be absent, and the disclosure is a phase line about the
    // deployment's own infrastructure. A run with an owner, or with a subject,
    // never reaches here.
    if owner.is_none() && subject_type.is_none() {
        return if (deps.is_admin)(user_id.to_string()).await? {
            Ok(RunWatchVerdict::Ok)
        } else {
            Ok(RunWatchVerdict::NotAudience)
        };
    }

    // Everything else needs a subject to resolve. A run with an OWNER who is
    // not this caller falls through to here and is refused unless its subject
    // admits them — sharing a run is done by sharing the thing it is about.
    let (Some(subject_type), Some(subject_id)) = (subject_type, subject_id) else {
        return Ok(RunWatchVerdict::NotAudience);
    };

    match subject_type {
        "board" => {
            if (deps.board_role)(user_id.to_string(), subject_id.to_string())
                .await?
                .is_some()
            {
                Ok(RunWatchVerdict::Ok)
            } else {
                Ok(RunWatchVerdict::NotAudience)
            }
        }
        "task" => {
            // A ticket's read ACL is its BOARD's, which is why this hop exists
            // rather than a `task_members` check that does not exist.
            let board_id = (deps.task_board_id)(subject_id.to_string()).await?;
            let Some(board_id) = board_id else {
                return Ok(RunWatchVerdict::NotAudience);
            };
            if (deps.board_role)(user_id.to_string(), board_id)
                .await?
                .is_some()
            {
                Ok(RunWatchVerdict::Ok)
            } else {
                Ok(RunWatchVerdict::NotAudience)
            }
        }
        "channel" => {
            if (deps.channel_role)(user_id.to_string(), subject_id.to_string())
                .await?
                .is_some()
            {
                Ok(RunWatchVerdict::Ok)
            } else {
                Ok(RunWatchVerdict::NotAudience)
            }
        }
        "conversation" => {
            // Accessible, not owned: a PLAN
            // admits its collaborators and a chat does not, and that
            // distinction is already decided in one place (conversations.rs).
            // Re-deciding it here would be a second answer.
            if (deps.conversation_access)(user_id.to_string(), subject_id.to_string()).await? {
                Ok(RunWatchVerdict::Ok)
            } else {
                Ok(RunWatchVerdict::NotAudience)
            }
        }
        other => {
            // LOUD, because this is a gap in this file and not a misbehaving
            // caller. `subject_type` is free text so a run definition can add
            // one without a migration; the cost of that freedom is that a new
            // subject type must be taught here or its runs have no live view.
            // Refusing keeps the failure visible and recoverable (reload
            // works); allowing would make it invisible and not.
            tracing::warn!(
                "{LOG} refusing run watch: no read predicate for subject_type {other:?} (run {run_id}). Teach may_watch_run about it."
            );
            Ok(RunWatchVerdict::UnknownSubject)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_of(res: Response) -> String {
        let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .expect("the stream ends once its sender is gone");
        String::from_utf8(bytes.to_vec()).expect("sse is utf-8")
    }

    // What every intermediary between the api and the browser reads: the
    // event-stream type, no caching or transforms, and no buffering. The
    // edge in front of every real instance (Cloudflare) honors the last two.
    #[tokio::test]
    async fn the_stream_carries_the_headers_proxies_honor() {
        let (tx, rx) = mpsc::channel::<String>(1);
        drop(tx);
        let res = sse_response(rx);
        let h = res.headers();
        assert_eq!(h["content-type"], "text/event-stream");
        assert_eq!(h["cache-control"], "no-cache, no-transform");
        assert_eq!(h["x-accel-buffering"], "no");
    }

    // The preamble names the reconnect delay, then every message is one
    // `data:` frame, and the stream ENDS when its subscriber does.
    #[tokio::test]
    async fn the_preamble_states_retry_then_frames_follow() {
        let (tx, rx) = mpsc::channel::<String>(4);
        tx.send("{\"type\":\"notification\",\"notificationId\":\"n1\"}".into())
            .await
            .unwrap();
        drop(tx);
        let body = body_of(sse_response(rx)).await;
        assert!(
            body.starts_with(": connected\nretry: 3000\n\n"),
            "preamble was {body:?}"
        );
        assert!(body.contains("data: {\"type\":\"notification\",\"notificationId\":\"n1\"}\n\n"));
    }

    // A quiet stream stays OPEN: its receiver yields nothing and does not
    // end, so the browser holds one connection instead of reconnecting on a
    // loop. Its parked sender goes when the receiver does.
    #[tokio::test]
    async fn a_quiet_stream_stays_open_until_its_client_leaves() {
        let mut rx = quiet_subscribe()("board:x".into()).await;
        let waited = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(waited.is_err(), "a quiet stream must not end: {waited:?}");
        drop(rx);
    }

    // A shutdown ends an open stream — even one whose subscriber is alive
    // and silent — so the graceful drain is not held open by it.
    #[tokio::test]
    async fn a_shutdown_ends_an_open_stream() {
        let (tx, rx) = mpsc::channel::<String>(1);
        let (end_tx, mut end_rx) = tokio::sync::watch::channel(false);
        let until = async move {
            let _ = end_rx.wait_for(|e| *e).await;
        };
        let res = sse_response_until(rx, until);
        let body = tokio::spawn(body_of(res));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !body.is_finished(),
            "an open stream must not end on its own"
        );
        end_tx.send_replace(true);
        let body = tokio::time::timeout(Duration::from_secs(2), body)
            .await
            .expect("the stream ends on shutdown")
            .unwrap();
        assert!(body.starts_with(": connected\nretry: 3000\n\n"), "{body:?}");
        drop(tx);
    }

    // ── The hub's registry ───────────────────────────────────────────────────

    // The first stream on a topic asks the driver to subscribe; the next ones
    // ride it; the last one out asks it to unsubscribe. Every stream gets
    // every message.
    #[tokio::test]
    async fn the_hub_counts_streams_per_topic_and_fans_out() {
        let (hub, mut wake) = Hub::new();
        let (mut a, _) = hub.register("user:u1").unwrap();
        let (mut b, _) = hub.register("user:u1").unwrap();
        assert_eq!(wake.try_recv().unwrap(), "user:u1");
        assert!(
            wake.try_recv().is_err(),
            "the second stream needs no SUBSCRIBE"
        );
        assert_eq!(hub.refs("user:u1"), 2);
        hub.deliver("user:u1", Arc::from("p1"));
        hub.deliver("board:b1", Arc::from("nobody")); // not wanted: dropped
        assert_eq!(&*a.recv().await.unwrap(), "p1");
        assert_eq!(&*b.recv().await.unwrap(), "p1");
        hub.release("user:u1");
        assert!(wake.try_recv().is_err(), "one stream is left");
        hub.release("user:u1");
        assert_eq!(wake.try_recv().unwrap(), "user:u1");
        assert_eq!(hub.refs("user:u1"), 0);
        assert!(hub.wanted().is_empty());
    }

    // A reconnect tells user streams (and only them) to resync; a close ends
    // every stream and refuses new ones.
    #[tokio::test]
    async fn a_reconnect_resyncs_user_streams_and_a_close_ends_them() {
        let (hub, _wake) = Hub::new();
        let (mut user, _) = hub.register("user:u1").unwrap();
        let (mut board, _) = hub.register("board:b1").unwrap();
        hub.announce_reconnect();
        assert_eq!(&*user.recv().await.unwrap(), HUB_RECONNECTED);
        assert!(board.try_recv().is_err());
        hub.close();
        assert!(matches!(
            user.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
        assert!(hub.register("user:u2").is_none());
    }

    // The subscribe edge: a forwarder per stream, which ends — releasing the
    // topic — when its client leaves, and ends its client's stream when it
    // falls behind.
    #[tokio::test]
    async fn a_forwarder_releases_its_topic_and_ends_a_lagging_stream() {
        let (hub, _wake) = Hub::new();
        hub.set_live("user:u1", true); // no entry yet: a no-op
        let rx = {
            // Live already, so `subscribe` does not wait for a driver.
            let (_m, _l) = hub.register("user:u1").unwrap();
            hub.set_live("user:u1", true);
            hub.clone().subscribe("user:u1".into()).await
        };
        assert_eq!(hub.refs("user:u1"), 2);
        hub.release("user:u1"); // the registration above
        drop(rx);
        // The forwarder notices its client left and releases the topic.
        for _ in 0..50 {
            if hub.refs("user:u1") == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(hub.refs("user:u1"), 0);

        let (_m, _l) = hub.register("user:slow").unwrap();
        hub.set_live("user:slow", true);
        let mut slow = hub.clone().subscribe("user:slow".into()).await;
        for i in 0..(TOPIC_BUFFER + 40) {
            hub.deliver("user:slow", Arc::from(format!("p{i}").as_str()));
        }
        // The forwarder hands over what fits, then finds it lagged and ends.
        let mut got = 0;
        while tokio::time::timeout(Duration::from_secs(2), slow.recv())
            .await
            .expect("the stream ends, it does not hang")
            .is_some()
        {
            got += 1;
        }
        assert!(got < TOPIC_BUFFER + 40, "a lagged stream ends: got {got}");
    }

    // A user stream whose topic Redis has not confirmed when the wait runs
    // out is told to resync the moment it is: what was published before the
    // SUBSCRIBE landed never reached it.
    #[tokio::test]
    async fn a_late_subscription_resyncs_a_user_stream() {
        let (hub, _wake) = Hub::new();
        let sub = tokio::spawn(hub.clone().subscribe("user:late".into()));
        // Past LIVE_WAIT nothing has confirmed it; the subscriber went ahead.
        let mut rx = tokio::time::timeout(LIVE_WAIT + Duration::from_secs(2), sub)
            .await
            .expect("the wait is bounded")
            .unwrap();
        assert!(rx.try_recv().is_err());
        hub.set_live("user:late", true);
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, HUB_RECONNECTED);
        hub.deliver("user:late", Arc::from("after"));
        assert_eq!(rx.recv().await.unwrap(), "after");
    }

    // A hub whose driver is gone (its runtime ended) is closed, and the next
    // subscribe gets a fresh hub rather than a dead one.
    #[tokio::test]
    async fn a_hub_that_lost_its_driver_is_replaced() {
        let url = "redis://127.0.0.1:1/";
        let first = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let hub = rt.block_on(async { Hub::for_url(url).unwrap() });
            drop(rt); // the driver task goes with its runtime
            hub
        })
        .join()
        .unwrap();
        assert!(
            first.closed.load(Ordering::SeqCst),
            "a driverless hub closes"
        );
        let second = Hub::for_url(url).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(!second.closed.load(Ordering::SeqCst));
    }

    // ── The user stream ──────────────────────────────────────────────────────

    fn frames_of(body: &str) -> Vec<String> {
        body.split("\n\n")
            .filter(|b| !b.is_empty())
            .map(str::to_string)
            .collect()
    }

    // The replay is the preamble, in order; live frames the replay covered
    // are dropped; a notification frame carries its seq as `id:`.
    #[tokio::test]
    async fn replay_then_live_frames_each_row_once_with_its_id() {
        let (tx, rx) = mpsc::channel::<String>(8);
        let mut s = UserStream::live(rx);
        s.preamble = vec![
            UserFrame::of(&UserEvent::Notification {
                notification_id: "n8".into(),
                seq: Some(8),
            }),
            UserFrame::of(&UserEvent::Resync {
                seq: 8,
                complete: true,
            }),
        ];
        s.covered_through = Some(8);
        let n = |id: &str, seq: i64| {
            serde_json::to_string(&UserEvent::Notification {
                notification_id: id.into(),
                seq: Some(seq),
            })
            .unwrap()
        };
        tx.send(n("n8", 8)).await.unwrap(); // the replay's row, live: dropped
        tx.send(n("n9", 9)).await.unwrap();
        tx.send(r#"{"type":"channel","channelId":"c1"}"#.into())
            .await
            .unwrap();
        drop(tx);
        let body = body_of(resumable_event_stream(s)).await;
        assert_eq!(
            frames_of(&body),
            [
                ": connected\nretry: 3000",
                "id: 8\ndata: {\"type\":\"notification\",\"notificationId\":\"n8\",\"seq\":8}",
                "data: {\"type\":\"resync\",\"seq\":8,\"complete\":true}",
                "id: 9\ndata: {\"type\":\"notification\",\"notificationId\":\"n9\",\"seq\":9}",
                "data: {\"type\":\"channel\",\"channelId\":\"c1\"}",
            ]
        );
    }

    // A frame is sent only to a live session: the first one after the
    // session is gone ends the stream with `session: ended` instead.
    #[tokio::test]
    async fn no_frame_reaches_a_session_that_ended() {
        let (tx, rx) = mpsc::channel::<String>(8);
        let alive = Arc::new(AtomicBool::new(true));
        let flag = alive.clone();
        let mut s = UserStream::live(rx);
        s.session = Some(session_check_fn(move || {
            let alive = flag.load(Ordering::SeqCst);
            async move {
                if alive {
                    SessionState::Alive
                } else {
                    SessionState::Ended
                }
            }
        }));
        let body = tokio::spawn(body_of(resumable_event_stream(s)));
        tx.send(r#"{"type":"notification","notificationId":"a","seq":1}"#.into())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        alive.store(false, Ordering::SeqCst);
        tx.send(r#"{"type":"notification","notificationId":"b","seq":2}"#.into())
            .await
            .unwrap();
        let body = tokio::time::timeout(Duration::from_secs(2), body)
            .await
            .expect("the stream ends with its session")
            .unwrap();
        assert!(body.contains("\"notificationId\":\"a\""), "{body}");
        assert!(!body.contains("\"notificationId\":\"b\""), "{body}");
        assert!(
            body.ends_with("data: {\"type\":\"session\",\"state\":\"ended\"}\n\n"),
            "{body}"
        );
        drop(tx);
    }

    // Logout's revocation ends exactly the streams of the revoked session.
    #[tokio::test]
    async fn a_revocation_ends_only_its_own_sessions_stream() {
        let (_tx, rx) = mpsc::channel::<String>(1);
        let (rtx, rrx) = mpsc::channel::<String>(4);
        let mut s = UserStream::live(rx);
        s.revoked = Some((rrx, "hash-mine".into()));
        let body = tokio::spawn(body_of(resumable_event_stream(s)));
        rtx.send("hash-other".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!body.is_finished(), "another session's logout is not ours");
        rtx.send("hash-mine".into()).await.unwrap();
        let body = tokio::time::timeout(Duration::from_secs(2), body)
            .await
            .expect("ours ends it")
            .unwrap();
        assert_eq!(
            frames_of(&body),
            [
                ": connected\nretry: 3000",
                "data: {\"type\":\"session\",\"state\":\"ended\"}",
            ]
        );
    }

    // A quiet stream finds out on the tick (the 7-day expiry, a deleted key).
    #[tokio::test]
    async fn a_quiet_stream_rereads_its_session_every_tick() {
        let (_tx, rx) = mpsc::channel::<String>(1);
        let mut s = UserStream::live(rx);
        s.session = Some(session_check_fn(|| async { SessionState::Ended }));
        s.session_every = Duration::from_millis(200);
        let body = tokio::spawn(body_of(resumable_event_stream(s)));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!body.is_finished(), "nothing is checked before the tick");
        let body = tokio::time::timeout(Duration::from_secs(2), body)
            .await
            .expect("the tick ends it")
            .unwrap();
        assert!(body.ends_with("data: {\"type\":\"session\",\"state\":\"ended\"}\n\n"));
        // An unreadable session is not an ended one.
        let (_tx, rx) = mpsc::channel::<String>(1);
        let mut s = UserStream::live(rx);
        s.session = Some(session_check_fn(|| async { SessionState::Unknown }));
        s.session_every = Duration::from_millis(20);
        let body = tokio::spawn(body_of(resumable_event_stream(s)));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!body.is_finished());
        body.abort();
    }

    // After the hub reconnects, the stream says so with the person's seq.
    #[tokio::test]
    async fn the_hubs_reconnect_becomes_a_resync_with_the_seq() {
        let (tx, rx) = mpsc::channel::<String>(4);
        let mut s = UserStream::live(rx);
        s.seq_now = Some(seq_now_fn(|| async { Some(42) }));
        tx.send(HUB_RECONNECTED.into()).await.unwrap();
        drop(tx);
        let body = body_of(resumable_event_stream(s)).await;
        assert_eq!(
            frames_of(&body)[1],
            "data: {\"type\":\"resync\",\"seq\":42,\"complete\":false}"
        );
        // And a topic stream never forwards the marker.
        let (tx, rx) = mpsc::channel::<String>(4);
        tx.send(HUB_RECONNECTED.into()).await.unwrap();
        tx.send("{\"type\":\"task\"}".into()).await.unwrap();
        drop(tx);
        let body = body_of(sse_response(rx)).await;
        assert_eq!(
            frames_of(&body),
            [": connected\nretry: 3000", "data: {\"type\":\"task\"}"]
        );
    }

    #[test]
    fn only_a_notification_with_a_seq_gets_an_id() {
        assert_eq!(
            notification_seq(r#"{"type":"notification","notificationId":"a","seq":5}"#),
            Some(5)
        );
        assert_eq!(
            notification_seq(r#"{"type":"notification","notificationId":"a"}"#),
            None
        );
        assert_eq!(
            notification_seq(r#"{"type":"brief","briefId":"b","seq":3}"#),
            None
        );
        assert_eq!(notification_seq("not json"), None);
    }

    // The read-sync event is a number and a tag; the v2 fields are absent
    // unless set, so a wave-1 publisher's frame is byte-identical.
    #[test]
    fn the_notifications_event_is_id_shaped() {
        let s = serde_json::to_string(&UserEvent::Notifications {
            unread: 3,
            unread_for_you: None,
            read_ids: None,
            all: None,
            resync: None,
        })
        .unwrap();
        assert_eq!(s, "{\"type\":\"notifications\",\"unread\":3}");
        let s = serde_json::to_string(&UserEvent::Notifications {
            unread: 1,
            unread_for_you: Some(0),
            read_ids: Some(vec!["a".into(), "b".into()]),
            all: Some(false),
            resync: None,
        })
        .unwrap();
        assert_eq!(
            s,
            "{\"type\":\"notifications\",\"unread\":1,\"unreadForYou\":0,\"readIds\":[\"a\",\"b\"],\"all\":false}"
        );
    }

    // The contract's wire (I-17, I-19): every new frame is tag + ids/flags.
    #[test]
    fn the_v2_frames_are_pinned() {
        let j = |e: &UserEvent| serde_json::to_string(e).unwrap();
        assert_eq!(
            j(&UserEvent::Notification {
                notification_id: "n1".into(),
                seq: None
            }),
            "{\"type\":\"notification\",\"notificationId\":\"n1\"}"
        );
        assert_eq!(
            j(&UserEvent::Notification {
                notification_id: "n1".into(),
                seq: Some(7)
            }),
            "{\"type\":\"notification\",\"notificationId\":\"n1\",\"seq\":7}"
        );
        assert_eq!(
            j(&UserEvent::Presence {
                looking: Some("c-a".into()),
                active: None,
                clients: 2
            }),
            "{\"type\":\"presence\",\"looking\":\"c-a\",\"active\":null,\"clients\":2}"
        );
        assert_eq!(
            j(&UserEvent::Resync {
                seq: 9,
                complete: false
            }),
            "{\"type\":\"resync\",\"seq\":9,\"complete\":false}"
        );
        assert_eq!(
            j(&UserEvent::Session {
                state: "ended".into()
            }),
            "{\"type\":\"session\",\"state\":\"ended\"}"
        );
    }
}
