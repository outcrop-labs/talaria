// The mirroring half of runs: topics, payload shape, and who may attach.
//
// Redis is faked at the edge boundary rather than mocked per call, because the
// thing under test IS the routing — "a run event reaches the subscriber on
// that run's topic and no other" is a statement about channel names, and a
// mock that records `publish(topic, payload)` without delivering anything
// cannot make it. The fake is a real (in-process) pub/sub: subscribers
// register topics, `publish` fans out to the ones that match.
//
// Two structural notes: "disconnect on abort" is observed as the hub's sender
// closing when the receiver drops (a stream ends by its response being
// dropped, not by an AbortSignal), and the SSE frames are asserted from the
// built response body — axum offers no Event getter, so the test reads the
// exact bytes the server would send. The unknown-subject warning is a tracing
// call, not spyable; the verdict is the pinned half (same call the reclaim
// suite makes).

use futures_util::FutureExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use talaria_api::realtime::{
    RealtimeDeps, RunWatchDeps, RunWatchRow, RunWatchVerdict, UserEvent, may_watch_run,
    publish_user, run_event_stream, run_publish, user_event_stream,
};
use talaria_api::runs::define::{DecisionOption, DecisionRequest, RunState};
use talaria_api::runs::run::RunEvent;
use tokio::sync::mpsc;

// ── The in-process hub ───────────────────────────────────────────────────────

type Topic = Arc<Mutex<HashMap<String, Vec<mpsc::Sender<String>>>>>;

/// The fake: `subscribe` registers a sender per topic and hands back the
/// receiver, `publish` fans out to every registered sender, and a sender whose
/// receiver has been dropped is gone — the observable half of "the dedicated
/// subscriber is disconnected when the request goes away".
#[derive(Clone)]
struct Hub {
    topics: Topic,
}

impl Hub {
    fn new() -> Self {
        Self {
            topics: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// deps whose publish fans out through the hub — the fake at the deps
    /// edge where the real redis client would be.
    fn deps(&self) -> RealtimeDeps {
        let publish_topics = self.topics.clone();
        let sub_hub = self.clone();
        RealtimeDeps {
            publish: Arc::new(move |topic: &str, payload: &str| {
                let senders = publish_topics
                    .lock()
                    .unwrap()
                    .get(topic)
                    .cloned()
                    .unwrap_or_default();
                for tx in senders {
                    // Only a closed receiver removes the sender; a full buffer
                    // (not reachable at test volumes) is not a disconnect.
                    let _ = tx.try_send(payload.to_string());
                }
            }),
            subscribe: Arc::new(move |topic| {
                let hub = sub_hub.clone();
                async move { hub.subscribe(&topic) }.boxed()
            }),
        }
    }

    fn subscribe(&self, topic: &str) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel::<String>(64);
        self.topics
            .lock()
            .unwrap()
            .entry(topic.to_string())
            .or_default()
            .push(tx);
        rx
    }

    /// Live subscribers on a topic — senders whose receivers have not dropped.
    fn subscriber_count(&self, topic: &str) -> usize {
        self.topics
            .lock()
            .unwrap()
            .get(topic)
            .map(|v| v.iter().filter(|tx| !tx.is_closed()).count())
            .unwrap_or(0)
    }

    /// End every subscription on a topic, so a stream built on it can finish
    /// and be read to its last byte.
    fn close_topic(&self, topic: &str) {
        if let Some(senders) = self.topics.lock().unwrap().get_mut(topic) {
            senders.clear();
        }
    }
}

// ── Stream helpers ───────────────────────────────────────────────────────────

/// Drain everything that landed on a subscription, in order. The caller
/// CLOSES the topic first (`publish` is a synchronous `try_send`, so what was
/// published is already buffered) — closing drops the hub's senders, `recv`
/// ends, and this returns instead of waiting on a subscription that never
/// ends.
async fn payloads(mut rx: mpsc::Receiver<String>) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(p) = rx.recv().await {
        out.push(p);
    }
    out
}

/// The JSON body of one delivered payload, as a parsed map.
fn frame(payload: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_str(payload).expect("payload is one JSON object")
}

/// The internal driver event — the WIDER object the wire event is built from,
/// question and all when a test wants one.
fn run_event(run_id: &str, state: RunState, phase: &str) -> RunEvent {
    RunEvent {
        kind_tag: "run",
        run_id: run_id.into(),
        kind: "demo".into(),
        state,
        phase: phase.into(),
        question: None,
        error: None,
    }
}

// ── Topics ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn run_topic_delivers_only_to_its_own_subscriber() {
    let hub = Hub::new();
    let deps = hub.deps();
    let a = hub.subscribe("run:run-a");
    let b = hub.subscribe("run:run-b");

    // The adapter publishes the wire event built field by field.
    let publish = run_publish(deps.clone());
    publish(run_event("run-a", RunState::Running, "for a"), None);
    // Then a DIFFERENT event on b's own topic. If the first publish had leaked
    // across topics, b's first payload would be that one — reading b and
    // finding only its own event is what proves the isolation, without racing
    // a timeout to assert an absence.
    publish(run_event("run-b", RunState::Running, "for b"), None);
    hub.close_topic("run:run-a");
    hub.close_topic("run:run-b");

    let a = payloads(a).await;
    let b = payloads(b).await;
    assert_eq!(a.len(), 1);
    assert_eq!(frame(&a[0])["runId"], "run-a");
    assert_eq!(frame(&a[0])["phase"], "for a");
    assert_eq!(b.len(), 1);
    assert_eq!(frame(&b[0])["runId"], "run-b");
    assert_eq!(frame(&b[0])["phase"], "for b");
}

#[tokio::test]
async fn run_topic_never_carries_the_decision_question() {
    let hub = Hub::new();
    let deps = hub.deps();
    let rx = hub.subscribe("run:run-q");

    // The driver's internal event holding a question — a field the caller can
    // set. The wire event is rebuilt field by field and the question must not
    // survive the rebuild.
    let mut event = run_event("run-q", RunState::Awaiting, "parked");
    event.question = Some(DecisionRequest {
        key: "k".into(),
        question: "Archive the Contoso thread?".into(),
        detail: Some("secret detail".into()),
        options: vec![DecisionOption {
            id: "yes".into(),
            label: "Archive it".into(),
            detail: None,
        }],
        href: None,
    });
    run_publish(deps)(event, None);
    hub.close_topic("run:run-q");

    let got = frame(&payloads(rx).await[0]);
    let mut keys: Vec<&str> = got.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["kind", "phase", "runId", "state", "type"]);
    assert!(
        serde_json::to_string(&got)
            .unwrap()
            .find("secret detail")
            .is_none()
    );
}

#[tokio::test]
async fn run_topic_carries_the_terminal_error_only_when_there_is_one() {
    let hub = Hub::new();
    let deps = hub.deps();
    let rx = hub.subscribe("run:run-e");

    let mut event = run_event("run-e", RunState::Error, "failed");
    event.error = Some("upstream refused".into());
    run_publish(deps)(event, None);

    hub.close_topic("run:run-e");
    let got = frame(&payloads(rx).await[0]);
    assert_eq!(got["state"], "error");
    assert_eq!(got["error"], "upstream refused");
    // And absent, not null, when there is not one — the plain transition above
    // in the isolation test carried no `error` key at all.
}

#[tokio::test]
async fn the_owner_firehose_hears_the_id_shaped_echo() {
    let hub = Hub::new();
    let deps = hub.deps();
    let rx = hub.subscribe("user:user-1");

    // The internal event carries phase (and could carry a question); the echo
    // on the person's own topic must be ids and closed enums only.
    let mut event = run_event("run-a", RunState::Awaiting, "about to archive #ACME-14");
    event.error = Some("nope".into());
    run_publish(deps)(event, Some("user-1"));

    hub.close_topic("user:user-1");
    let got = frame(&payloads(rx).await[0]);
    let mut keys: Vec<&str> = got.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["runId", "state", "type"]);
    assert!(
        serde_json::to_string(&got)
            .unwrap()
            .find("ACME-14")
            .is_none()
    );
}

#[tokio::test]
async fn a_run_without_an_owner_publishes_no_echo() {
    let hub = Hub::new();
    let deps = hub.deps();
    let mut nobody = hub.subscribe("user:");

    run_publish(deps.clone())(run_event("run-o", RunState::Done, "done"), None);
    // An EMPTY owner id is nobody's firehose, not a topic literally named
    // "user:".
    run_publish(deps)(run_event("run-o", RunState::Done, "done"), Some(""));

    assert!(nobody.try_recv().is_err());
}

#[tokio::test]
async fn one_persons_firehose_stays_off_another_persons() {
    let hub = Hub::new();
    let deps = hub.deps();
    let one = hub.subscribe("user:user-1");
    let two = hub.subscribe("user:user-2");

    publish_user(
        &deps,
        "user-1",
        &UserEvent::Run {
            run_id: "mine".into(),
            state: RunState::Awaiting,
        },
    );
    publish_user(
        &deps,
        "user-2",
        &UserEvent::Run {
            run_id: "theirs".into(),
            state: RunState::Queued,
        },
    );

    hub.close_topic("user:user-1");
    hub.close_topic("user:user-2");
    let one = payloads(one).await;
    let two = payloads(two).await;
    assert_eq!(frame(&one[0])["runId"], "mine");
    assert_eq!(frame(&two[0])["runId"], "theirs");
}

#[tokio::test]
async fn a_notification_event_is_id_shaped_too() {
    let hub = Hub::new();
    let deps = hub.deps();
    let rx = hub.subscribe("user:user-3");

    publish_user(
        &deps,
        "user-3",
        &UserEvent::Notification {
            notification_id: "n-1".into(),
        },
    );

    hub.close_topic("user:user-3");
    assert_eq!(
        payloads(rx).await[0],
        r#"{"type":"notification","notificationId":"n-1"}"#
    );
}

#[tokio::test]
async fn the_rails_two_signals_are_id_shaped_like_the_rest() {
    // The channel and conversation events move BADGES, and a badge client
    // refetches through the ordinary routes — so the payload has nothing to
    // disclose even if a future routing change points it at somebody new.
    let hub = Hub::new();
    let deps = hub.deps();
    let rx = hub.subscribe("user:user-5");

    publish_user(
        &deps,
        "user-5",
        &UserEvent::Channel {
            channel_id: "ch-1".into(),
        },
    );
    publish_user(
        &deps,
        "user-5",
        &UserEvent::Conversation {
            conversation_id: "cv-9".into(),
        },
    );

    hub.close_topic("user:user-5");
    let got = payloads(rx).await;
    assert_eq!(got[0], r#"{"type":"channel","channelId":"ch-1"}"#);
    assert_eq!(got[1], r#"{"type":"conversation","conversationId":"cv-9"}"#);
}

#[tokio::test]
async fn dropping_the_receiver_disconnects_its_dedicated_subscriber() {
    let hub = Hub::new();
    let rx = hub.subscribe("user:user-4");
    assert_eq!(hub.subscriber_count("user:user-4"), 1);
    drop(rx);
    assert_eq!(hub.subscriber_count("user:user-4"), 0);
}

// ── SSE framing ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_stream_opens_with_connected_then_frames_each_payload_as_sse_data() {
    let hub = Hub::new();
    let deps = hub.deps();

    let response = user_event_stream(&deps, "user-1").await;
    publish_user(
        &deps,
        "user-1",
        &UserEvent::Notification {
            notification_id: "n-1".into(),
        },
    );
    // End the subscription so the body can finish; the frames already
    // delivered are the assertion.
    hub.close_topic("user:user-1");

    let body = response.into_body();
    let bytes = axum::body::to_bytes(body, 64 * 1024)
        .await
        .expect("frames are bounded");
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        ": connected\n\ndata: {\"type\":\"notification\",\"notificationId\":\"n-1\"}\n\n"
    );
    let headers = {
        // rebuilt only to assert the header the stream sets
        let response = user_event_stream(&deps, "user-9").await;
        hub.close_topic("user:user-9");
        let (parts, _) = response.into_parts();
        parts.headers
    };
    assert_eq!(
        headers.get(axum::http::header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    assert_eq!(
        headers.get(axum::http::header::CACHE_CONTROL).unwrap(),
        "no-cache, no-transform"
    );
}

#[tokio::test]
async fn a_quiet_subscriber_still_gets_the_connected_preamble() {
    // The swallow-failures posture: a subscriber that never connects or never
    // hears anything still opens with `: connected`, so a client knows the
    // attach itself landed rather than seeing a dead stream. Ended by closing
    // the topic before anything is published.
    let hub = Hub::new();
    let deps = hub.deps();
    let response = run_event_stream(&deps, "run-quiet").await;
    hub.close_topic("run:run-quiet");
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("one frame");
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), ": connected\n\n");
}

// ── The gate ─────────────────────────────────────────────────────────────────

/// Per-test overrides for `watch_deps`, defaulting to the strict posture:
/// every predicate answers nothing, admin NO — so every case that does not opt
/// in asserts the ordinary-member answer, including the org-wide refusal.
#[derive(Default, Clone)]
struct Over {
    board_role: Option<String>,
    channel_role: Option<String>,
    task_board: Option<String>,
    conversation: bool,
    admin: bool,
    record_admin: Option<Arc<Mutex<Vec<String>>>>,
    record_board: Option<Arc<Mutex<Vec<String>>>>,
}

fn watch_deps(row: Option<RunWatchRow>, over: Over) -> RunWatchDeps {
    let row_for_get = row.clone();
    let Over {
        board_role,
        channel_role,
        task_board,
        conversation,
        admin,
        record_admin,
        record_board,
    } = over;
    RunWatchDeps {
        get_run: Arc::new(move |_| {
            let row = row_for_get.clone();
            async move { Ok(row) }.boxed()
        }),
        board_role: Arc::new(move |user_id, board_id| {
            let answer = board_role.clone();
            let seen = record_board.clone();
            async move {
                if let Some(seen) = seen {
                    seen.lock().unwrap().push(format!("{user_id}/{board_id}"));
                }
                Ok(answer)
            }
            .boxed()
        }),
        channel_role: Arc::new(move |_, _| {
            let answer = channel_role.clone();
            async move { Ok(answer) }.boxed()
        }),
        task_board_id: Arc::new(move |_| {
            let answer = task_board.clone();
            async move { Ok(answer) }.boxed()
        }),
        conversation_access: Arc::new(move |_, _| {
            let answer = conversation;
            async move { Ok(answer) }.boxed()
        }),
        is_admin: Arc::new(move |user_id| {
            let answer = admin;
            let asked = record_admin.clone();
            async move {
                if let Some(asked) = asked {
                    asked.lock().unwrap().push(user_id);
                }
                Ok(answer)
            }
            .boxed()
        }),
    }
}

fn row(
    owner: Option<&str>,
    subject_type: Option<&str>,
    subject_id: Option<&str>,
) -> Option<RunWatchRow> {
    Some(RunWatchRow {
        owner_user_id: owner.map(String::from),
        subject_type: subject_type.map(String::from),
        subject_id: subject_id.map(String::from),
    })
}

/// ownerless(None, None) spelled the way every org-wide test reads.
fn ownerless(subject_type: Option<&str>, subject_id: Option<&str>) -> Option<RunWatchRow> {
    row(None, subject_type, subject_id)
}

async fn verdict(row: Option<RunWatchRow>) -> RunWatchVerdict {
    may_watch_run("u2", "r1", &watch_deps(row, Over::default()))
        .await
        .expect("fake edges do not fail")
}

#[tokio::test]
async fn the_gate_allows_the_owner() {
    let v = may_watch_run(
        "u1",
        "r1",
        &watch_deps(row(Some("u1"), None, None), Over::default()),
    )
    .await
    .unwrap();
    assert_eq!(v, RunWatchVerdict::Ok);
}

#[tokio::test]
async fn the_gate_refuses_a_run_that_is_not_there_like_any_other_refusal() {
    // Same answer class as a refusal (the route collapses both to 403), but
    // the verdict stays distinguishable in a log — that is the point of the
    // three-valued reason.
    assert_eq!(verdict(None).await, RunWatchVerdict::Missing);
}

#[tokio::test]
async fn the_gate_refuses_a_stranger_on_somebody_elses_subjectless_run() {
    assert_eq!(
        verdict(row(Some("u1"), None, None)).await,
        RunWatchVerdict::NotAudience
    );
}

// ── Org-wide runs: the widening, both directions ─────────────────────────────
//
// A fitness sweep and a retrieval migration are ownerless AND subjectless, so
// the original rule refused everybody — the run had a live stream and no
// audience at all. The pair below is the whole rule: the shape decides who is
// asked, and admin-ness decides the answer.

#[tokio::test]
async fn an_admin_may_watch_an_org_wide_run() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let v = may_watch_run(
        "u9",
        "r1",
        &watch_deps(
            ownerless(None, None),
            Over {
                admin: true,
                record_admin: Some(asked.clone()),
                ..Over::default()
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(v, RunWatchVerdict::Ok);
    // Asked about the CALLER, not about the run's absent owner.
    assert_eq!(*asked.lock().unwrap(), ["u9"]);
}

#[tokio::test]
async fn a_non_admin_is_refused_on_an_org_wide_run() {
    assert_eq!(
        verdict(ownerless(None, None)).await,
        RunWatchVerdict::NotAudience
    );
}

#[tokio::test]
async fn an_admin_may_not_watch_somebody_elses_owned_run() {
    // The widening is about a SHAPE OF ROW, not about being an admin. A run
    // with an owner is that person's, and an admin gets the same refusal as
    // anyone.
    let v = may_watch_run(
        "u2",
        "r1",
        &watch_deps(
            row(Some("u1"), None, None),
            Over {
                admin: true,
                ..Over::default()
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(v, RunWatchVerdict::NotAudience);
}

#[tokio::test]
async fn an_admin_may_not_watch_a_run_whose_subject_refuses_them() {
    // A run about a board the admin is not a member of still resolves through
    // `boardRole`. Being an admin is not a read grant on somebody's board.
    let v = may_watch_run(
        "u2",
        "r1",
        &watch_deps(
            ownerless(Some("board"), Some("b1")),
            Over {
                admin: true,
                ..Over::default()
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(v, RunWatchVerdict::NotAudience);
}

#[tokio::test]
async fn an_admin_gets_the_unknown_subject_refusal_too() {
    let v = may_watch_run(
        "u2",
        "r1",
        &watch_deps(
            ownerless(Some("research"), Some("x1")),
            Over {
                admin: true,
                ..Over::default()
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(v, RunWatchVerdict::UnknownSubject);
}

#[tokio::test]
async fn a_board_subject_defers_to_board_role() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let allow = watch_deps(
        ownerless(Some("board"), Some("b1")),
        Over {
            board_role: Some("editor".into()),
            record_board: Some(seen.clone()),
            ..Over::default()
        },
    );
    assert_eq!(
        may_watch_run("u2", "r1", &allow).await.unwrap(),
        RunWatchVerdict::Ok
    );
    assert_eq!(*seen.lock().unwrap(), ["u2/b1"]);
    assert_eq!(
        verdict(ownerless(Some("board"), Some("b1"))).await,
        RunWatchVerdict::NotAudience
    );
}

#[tokio::test]
async fn a_task_subject_resolves_through_its_board() {
    let allow = watch_deps(
        ownerless(Some("task"), Some("t1")),
        Over {
            task_board: Some("b9".into()),
            board_role: Some("viewer".into()),
            ..Over::default()
        },
    );
    assert_eq!(
        may_watch_run("u2", "r1", &allow).await.unwrap(),
        RunWatchVerdict::Ok
    );

    // A task whose board cannot be resolved is a refusal, not a fall-through
    // to some other predicate — even one that says owner.
    let orphan = watch_deps(
        ownerless(Some("task"), Some("t1")),
        Over {
            board_role: Some("owner".into()),
            ..Over::default()
        },
    );
    assert_eq!(
        may_watch_run("u2", "r1", &orphan).await.unwrap(),
        RunWatchVerdict::NotAudience
    );
}

#[tokio::test]
async fn a_channel_subject_defers_to_channel_role() {
    let allow = watch_deps(
        ownerless(Some("channel"), Some("c1")),
        Over {
            channel_role: Some("member".into()),
            ..Over::default()
        },
    );
    assert_eq!(
        may_watch_run("u2", "r1", &allow).await.unwrap(),
        RunWatchVerdict::Ok
    );
    assert_eq!(
        verdict(ownerless(Some("channel"), Some("c1"))).await,
        RunWatchVerdict::NotAudience
    );
}

#[tokio::test]
async fn a_conversation_subject_defers_to_accessible_conversation() {
    let allow = watch_deps(
        ownerless(Some("conversation"), Some("k1")),
        Over {
            conversation: true,
            ..Over::default()
        },
    );
    assert_eq!(
        may_watch_run("u2", "r1", &allow).await.unwrap(),
        RunWatchVerdict::Ok
    );
    assert_eq!(
        verdict(ownerless(Some("conversation"), Some("k1"))).await,
        RunWatchVerdict::NotAudience
    );
}

#[tokio::test]
async fn a_subject_type_with_no_predicate_is_refused_not_allowed() {
    // Every predicate says yes; the verdict is still no, because the question
    // "who may read a `research` subject" has no answer in this file and a
    // default-allow would make every future subject type a silent widening.
    let permissive = watch_deps(
        ownerless(Some("research"), Some("x1")),
        Over {
            board_role: Some("owner".into()),
            channel_role: Some("owner".into()),
            conversation: true,
            task_board: Some("b1".into()),
            ..Over::default()
        },
    );
    assert_eq!(
        may_watch_run("u2", "r1", &permissive).await.unwrap(),
        RunWatchVerdict::UnknownSubject
    );
}

#[tokio::test]
async fn a_subject_type_it_knows_but_whose_id_is_missing_is_refused() {
    assert_eq!(
        verdict(ownerless(Some("board"), None)).await,
        RunWatchVerdict::NotAudience
    );
}

#[tokio::test]
async fn deciding_opens_no_subscriber_a_refusal_must_cost_nothing() {
    // The gate never subscribes: RunWatchDeps has no subscribe edge, so the
    // type system forbids it — the test stays to pin the intent against a
    // future "convenience".
    let hub = Hub::new();
    let before = hub.subscriber_count("user:anything");
    let _ = verdict(ownerless(Some("research"), Some("x1"))).await;
    let _ = verdict(None).await;
    assert_eq!(hub.subscriber_count("user:anything"), before);
}

#[tokio::test]
async fn a_failing_edge_is_an_error_not_a_refusal() {
    // A predicate that cannot answer (DB down) must not become a quiet 403 —
    // an error stays an error, the route 500s rather than refusing.
    let deps = RunWatchDeps {
        get_run: Arc::new(|_| {
            async { Err::<Option<RunWatchRow>, String>("relation \"runs\" does not exist".into()) }
                .boxed()
        }),
        board_role: Arc::new(|_, _| async { Ok::<Option<String>, String>(None) }.boxed()),
        channel_role: Arc::new(|_, _| async { Ok::<Option<String>, String>(None) }.boxed()),
        task_board_id: Arc::new(|_| async { Ok::<Option<String>, String>(None) }.boxed()),
        conversation_access: Arc::new(|_, _| async { Ok::<bool, String>(false) }.boxed()),
        is_admin: Arc::new(|_| async { Ok::<bool, String>(false) }.boxed()),
    };
    assert_eq!(
        may_watch_run("u2", "r1", &deps).await.unwrap_err(),
        "relation \"runs\" does not exist"
    );
}
