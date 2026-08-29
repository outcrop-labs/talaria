// Real-time fan-out over Redis pub/sub → SSE — the port of
// ui/src/server/realtime.ts. Mutations publish a small event to a topic
// (`board:<id>`, `channel:<id>`, `run:<id>`, `user:<id>`); each connected
// client holds an SSE stream fed by a dedicated Redis subscriber. Multiplayer
// without websockets.
//
// THE RULE THAT MAKES THIS SAFE, and it is worth stating once at the top
// because two of the topics below are the first ones carrying a thing a person
// might not be allowed to read: AN EVENT SAYS WHAT CHANGED, NEVER WHAT IT SAYS.
// The client re-fetches through the ordinary route, which applies the ordinary
// read ACL. A payload that carried the content would be a second read path with
// no gate on it — the fan-out would become the disclosure.
//
// Same Redis as the TS side for as long as both runtimes serve routes: a
// publish from either runtime's publishers reaches either runtime's
// subscribers, which is what lets a topic cross before its writers do.

use crate::runs::define::RunState;
use crate::runs::run::RunEvent;
use crate::runs::store::RunStore;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use futures_util::stream::{StreamExt, once, unfold};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const LOG: &str = "[realtime]";

// ── The two edges to Redis ───────────────────────────────────────────────────

/// The publish edge: fire-and-forget, errors logged not propagated — TS's
/// `void redis.publish(...).catch(log)`. A publish that BLOCKED the caller
/// would make every mutation wait on Redis; a publish that ERRORED the caller
/// would fail the write it is announcing, when the write already landed.
pub type PublishFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The subscribe edge: open a DEDICATED subscriber connection for one client
/// and hand back the receiving end of its forwarder. NEVER FAILS — TS swallows
/// connect/subscribe failures (an unhandled rejection there would take down
/// the whole server process); the Rust edge logs and returns a receiver
/// nobody feeds, so the client sees its `: connected` preamble and pings and
/// no events — exactly the quiet stream a dead subscriber left in TS.
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
fn quiet_subscribe() -> SubscribeFn {
    Arc::new(|_| async { mpsc::channel::<String>(1).1 }.boxed())
}

impl RealtimeDeps {
    /// Both halves over real Redis: publishes ride the shared
    /// ConnectionManager, each stream opens its own subscriber from a fresh
    /// `redis::Client` (parsing the URL only — the connection happens inside
    /// the stream's own task, where its failure is survivable by design).
    pub fn new(conn: redis::aio::ConnectionManager, redis_url: &str) -> Self {
        // The caller holds a manager built from this URL; parse cannot fail
        // now if it did not then.
        let client = redis::Client::open(redis_url)
            .expect("a redis url that already produced a ConnectionManager");
        Self {
            publish: redis_publish(conn),
            subscribe: redis_subscribe(client),
        }
    }

    /// The stream half alone — what an SSE route needs. The publish edge is a
    /// documented no-op: streams only ever subscribe, and forcing the shared
    /// manager's first-connect handshake on them would make opening a watch
    /// depend on the publisher's connection, a dependency TS never had.
    pub fn streams_only(redis_url: &str) -> Self {
        let subscribe = match redis::Client::open(redis_url) {
            Ok(client) => redis_subscribe(client),
            Err(e) => {
                tracing::error!("{LOG} cannot parse REDIS_URL — every stream will open quiet: {e}");
                quiet_subscribe()
            }
        };
        Self {
            publish: Arc::new(|_, _| {}),
            subscribe,
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

fn redis_subscribe(client: redis::Client) -> SubscribeFn {
    Arc::new(move |topic: String| {
        let client = client.clone();
        async move {
            let (tx, rx) = mpsc::channel::<String>(16);
            tokio::spawn(async move {
                let mut pubsub = match client.get_async_pubsub().await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("{LOG} subscriber for {topic} could not connect: {e}");
                        return;
                    }
                };
                if let Err(e) = pubsub.subscribe(&topic).await {
                    tracing::warn!("{LOG} subscriber for {topic} could not subscribe: {e}");
                    return;
                }
                // Bind the message stream ONCE: `on_message` borrows the pubsub
                // connection, and the loop below is that connection's whole
                // remaining life.
                let mut messages = pubsub.on_message();
                while let Some(msg) = messages.next().await {
                    let Ok(payload) = msg.get_payload::<String>() else {
                        continue;
                    };
                    // A send error is the receiver gone — the client hung up —
                    // and the only correct move is to drop the subscriber with
                    // this task. Nothing downstream disconnects a stream
                    // nobody was handed.
                    if tx.send(payload).await.is_err() {
                        break;
                    }
                }
            });
            rx
        }
        .boxed()
    })
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

/// An SSE response of a board's events. The boards event routes cross with the
/// boards family; the stream itself is realtime's and ports now.
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
// Notifications publish here too (`addNotification` → `{type:'notification'}`,
// the bell's signal) alongside runs and the brief — which is what the topic was
// NAMED for: it is the person's, not any one feature's. That port (notify's
// write half) is a later slice in this batch.

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

impl RunWireEvent {
    /// THE WIRE EVENT IS BUILT FIELD BY FIELD from the driver's internal
    /// event, never serialized from it. TS builds a fresh object because
    /// TypeScript cannot stop a caller passing a WIDER object than the
    /// parameter type; Rust's type system makes the extra fields unnameable,
    /// and this constructor keeps the loudest one — `question` — unnamed on
    /// the way out too, so the day someone "simplifies" this to
    /// `serde_json::to_string(&event)` the missing field is the diff.
    fn from_internal(event: &RunEvent) -> Self {
        Self {
            kind_tag: "run",
            run_id: event.run_id.clone(),
            kind: event.kind.clone(),
            state: event.state,
            phase: event.phase.clone(),
            error: event.error.clone(),
        }
    }
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
/// The enum IS the wire shape — TS re-builds the event field by field before
/// serializing because its union type cannot stop a wider object; a Rust enum
/// has no wider object to pass.
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
    /// The bell's signal: a notification row landed for this person.
    Notification {
        notification_id: String,
    },
    /// Something was appended to the person's daily brief.
    ///
    /// ID-SHAPED LIKE EVERY OTHER EVENT HERE, and on this topic that is not a
    /// formality: the brief is the densest private thing in the product — one
    /// person's approvals, blocked work and unread DMs on a single page — so
    /// an event carrying so much as a title would make the fan-out a second
    /// read path with no ACL on it. `seq` is the append cursor, which is all
    /// a client needs to decide whether the page it is holding is behind.
    Brief {
        brief_id: String,
        seq: i64,
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

/// run.ts publishRunEvent — the real `RunDeps.publish` assembly, the piece the
/// driver's deps point at once the handoff slice arms it. The run topic gets
/// the wire event; the owner's firehose gets the id-shaped echo, because the
/// phone watching "what do I have in flight" is not watching this run.
pub fn run_publish(deps: RealtimeDeps) -> crate::runs::run::PublishFn {
    Arc::new(move |event: RunEvent, owner_user_id: Option<&str>| {
        publish_run(&deps, &event.run_id, &RunWireEvent::from_internal(&event));
        // TS's `if (ownerUserId)` is a truthy check — an empty owner id is
        // nobody's firehose, not a topic named "user:".
        if let Some(owner) = owner_user_id.filter(|o| !o.is_empty()) {
            publish_user(
                &deps,
                owner,
                &UserEvent::Run {
                    run_id: event.run_id.clone(),
                    state: event.state,
                },
            );
        }
    })
}

/// An SSE response of one run's events. Gate the caller with `may_watch_run`
/// FIRST and to completion — see routes/runs_events.rs for why the order is
/// load-bearing.
pub async fn run_event_stream(deps: &RealtimeDeps, run_id: &str) -> Response {
    sse_response((deps.subscribe)(format!("run:{run_id}")).await)
}

/// An SSE response of one person's own firehose. The only legitimate argument
/// is the authenticated caller's own id.
pub async fn user_event_stream(deps: &RealtimeDeps, user_id: &str) -> Response {
    sse_response((deps.subscribe)(format!("user:{user_id}")).await)
}

// ── SSE framing ──────────────────────────────────────────────────────────────

/// topicEventStream's frames: the `: connected` preamble, then one `data:`
/// frame per pub/sub message. Payloads are one line by construction —
/// `serde_json::to_string` never emits a literal newline, and axum asserts on
/// one rather than corrupting the stream.
fn topic_frames(
    rx: mpsc::Receiver<String>,
) -> impl futures_util::Stream<Item = Result<Event, Infallible>> {
    once(async { Ok::<Event, Infallible>(Event::default().comment("connected")) }).chain(unfold(
        rx,
        |mut rx| async move {
            // `recv` ending (every sender dropped) is the stream ending — the
            // subscriber is gone and the response finishes, which is how the
            // client learns to reconnect with a fresh one.
            rx.recv()
                .await
                .map(|payload| (Ok::<Event, Infallible>(Event::default().data(payload)), rx))
        },
    ))
}

/// The SSE response TS's routes build: `text/event-stream` (set by `Sse`
/// itself), `Cache-Control: no-cache, no-transform` (TS's header verbatim), and
/// a 25s `: ping` keep-alive. Three documented divergences, all in the keep:
/// axum pings only when the stream has been IDLE for the interval — TS pinged
/// unconditionally — and any data frame resets the timer, which is the same
/// job a ping does for an intermediary; TS's `Connection: keep-alive` header
/// is not set, because HTTP/1.1 defaults to keep-alive and hyper owns that
/// header on this side; and a stream whose subscriber dies ENDS (the
/// forwarder's sender drops, the receiver closes) rather than pinging into the
/// void until the client gives up — an EventSource reconnects on stream end
/// and gets a fresh subscriber, the same recovery TS left to the client's
/// timeout but from the server's own initiative.
fn sse_response(rx: mpsc::Receiver<String>) -> Response {
    let mut res = Sse::new(topic_frames(rx))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_millis(25_000))
                .text("ping"),
        )
        .into_response();
    res.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    res
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
// it, because a `subject_type` is free text by design and a future port can
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
    /// The run is not there. The ROUTE answers this exactly like `NotAudience`
    /// (403, no body difference) — 404-vs-403 on a guessable id is an
    /// existence oracle, and the board and channel event routes already
    /// collapse the two.
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

/// May this person attach to `run:<id>`? An `Err` is an edge failing — in TS
/// the same failure propagates out of `mayWatchRun` and the route answers 500;
/// the port keeps it an error rather than converting it to a refusal, because
/// a broken predicate and an answered refusal are different facts.
pub async fn may_watch_run(
    user_id: &str,
    run_id: &str,
    deps: &RunWatchDeps,
) -> Result<RunWatchVerdict, String> {
    let run = match (deps.get_run)(run_id.to_string()).await? {
        Some(run) => run,
        None => return Ok(RunWatchVerdict::Missing),
    };
    // TS's truthiness, encoded once: in all three of these columns an empty
    // string is as absent as a null (`if (run.ownerUserId)`, `!subjectType`,
    // `!subjectId`), and every branch below reads the filtered Options.
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
            // `accessibleConversation`, not "owned conversation": a PLAN
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
            // caller. `subject_type` is free text so a port can add one
            // without a migration; the cost of that freedom is that a new
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

/// The real watch edges: the same predicates the TS module defers its imports
/// to (there, deferral breaks a load-time cycle; here, the modules are just
/// modules). Each returns the sqlx error text in the `Err` string — the route
/// logs it and answers 500, the answer a throwing predicate produced in TS.
pub fn real_watch_deps(pg: sqlx::PgPool) -> RunWatchDeps {
    // One clone per edge: every closure owns its pool handle outright.
    let get_run_pg = pg.clone();
    let board_pg = pg.clone();
    let channel_pg = pg.clone();
    let task_pg = pg.clone();
    let conversation_pg = pg.clone();
    let admin_pg = pg;
    RunWatchDeps {
        get_run: Arc::new(move |id| {
            let pg = get_run_pg.clone();
            async move {
                let store = crate::runs::store::PgRunStore::new(pg);
                let row = store
                    .get(&id)
                    .await
                    .map_err(|e| e.to_string())?
                    .map(|r| RunWatchRow {
                        owner_user_id: r.owner_user_id,
                        subject_type: r.subject_type,
                        subject_id: r.subject_id,
                    });
                Ok(row)
            }
            .boxed()
        }),
        board_role: Arc::new(move |user_id, board_id| {
            let pg = board_pg.clone();
            async move {
                crate::boards::board_role(&pg, &user_id, &board_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        channel_role: Arc::new(move |user_id, channel_id| {
            let pg = channel_pg.clone();
            async move {
                crate::channels::channel_role(&pg, &user_id, &channel_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        task_board_id: Arc::new(move |task_id| {
            let pg = task_pg.clone();
            async move {
                crate::tasks::task_board_id(&pg, &task_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        conversation_access: Arc::new(move |user_id, conversation_id| {
            let pg = conversation_pg.clone();
            async move {
                crate::conversations::conversation_accessible(&pg, &user_id, &conversation_id)
                    .await
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
        is_admin: Arc::new(move |user_id| {
            let pg = admin_pg.clone();
            async move {
                crate::users::get_user_role(&pg, &user_id)
                    .await
                    .map(|role| role == "admin")
                    .map_err(|e| e.to_string())
            }
            .boxed()
        }),
    }
}
