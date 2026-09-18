// Live-DB proof of the assign-vs-dispatch split (cargo test -- --ignored).
// TALA-32: assigning an agent to a ticket sitting in intake must leave the
// ticket in intake — no column jump, no dispatch, no "moved to …" activity —
// while an explicit move into the pickup column still dispatches to every
// agent assignee, and a joiner on a ticket already in the queue gets only
// their own push. The guarantees this file pins live in the PATCH pipeline:
// the writer's status selection, the activity sentences, and the
// pickup-column gate that maybe_dispatch_ticket answers from the re-read
// row — none of which a pure unit test can vouch for.
//
// The dispatch edge is the work_dispatch suite's own trick, lifted here
// whole: a store-only RunDeps whose insert is the observable. No Redis, no
// definitions — the walk stops at NoDefinition after one store read, so an
// insert that lands IS the dispatch, and a silent store IS no dispatch.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test assign_not_dispatch_live -- --ignored

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use serde_json::Value;
use sqlx::postgres::PgPool;
use talaria_api::runs::define::{DecisionAnswer, RunDecision, RunRow, RunState};
use talaria_api::runs::defs::work_session::{WORK_SESSION_KIND, session_run_id};
use talaria_api::runs::run::{LeaseClaim, LeaseRenewal, PauseOutcome, RunDeps, RunLease};
use talaria_api::runs::store::{
    AnswerOutcome, CancelOutcome, ClaimOutcome, NewRun, RunStore, WriteFailure, WriteOutcome,
};
use talaria_api::tasks::{
    NewTask, TaskActor, TaskDeps, TaskPatch, create_task, get_task, list_activity, update_task,
};

// ── The fixture: real Postgres rows, fake dispatch ──────────────────────────

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// Every row hangs off one throwaway user; the cascade takes board → task →
/// activity with it. Tests run concurrently, so each carries its own tag.
async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where email = $1")
        .bind(format!("assign-dispatch-{tag}@link-test.invalid"))
        .execute(pg)
        .await
        .unwrap();
}

struct Fixture {
    task_id: String,
}

/// A fresh board with an unassigned ticket sitting in intake (`inbox`). The
/// default statuses serve through list_statuses's DEFAULTS fallback — no
/// board_statuses rows are written — so `assigned` is the pickup column and
/// `inbox` is intake, exactly the shape the bug report describes. The board
/// allows every agent: board POLICY is not what this file is about, and a
/// refusal there would stand dispatch down for the wrong reason.
async fn fixture(pg: &PgPool, tag: &str) -> Fixture {
    cleanup(pg, tag).await;
    let email = format!("assign-dispatch-{tag}@link-test.invalid");
    let (owner,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) \
         values ($1, $1, 'Assign Dispatch Live', 'member') returning id::text",
    )
    .bind(&email)
    .fetch_one(pg)
    .await
    .unwrap();
    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id, ticket_prefix, allow_all_agents) \
         values ('assign vs dispatch live test', $1::uuid, 'ADL', true) returning id::text",
    )
    .bind(&owner)
    .fetch_one(pg)
    .await
    .unwrap();

    let (run_deps, _fixture_recorder) = recorder_run_deps();
    let deps = task_deps(pg, run_deps);
    let task = create_task(
        &deps,
        &NewTask {
            board_id: &board,
            title: "the assign question",
            description: None,
            priority: None,
            effort: None,
            assignees: &[],
            due_date: None,
            start_date: None,
            color: None,
            estimated_hours: None,
            parent_id: None,
            tags: &[],
            created_by: &owner,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("intake ticket created: {}", e.message()));
    Fixture { task_id: task.id }
}

// ── The dispatch observable ─────────────────────────────────────────────────
// Same contract as work_dispatch's WalkStore: `get` answers empty (the walk
// always reaches insert), every other method is unreachable on purpose —
// louder than a wrong answer if the walk ever starts driving.

struct DispatchRecorder {
    inserted: Mutex<Vec<String>>,
}

impl RunStore for DispatchRecorder {
    fn insert<'a>(&'a self, row: NewRun) -> BoxFuture<'a, Result<RunRow, sqlx::Error>> {
        Box::pin(async move {
            self.inserted.lock().unwrap().push(row.id.clone());
            Ok(RunRow {
                id: row.id,
                kind: WORK_SESSION_KIND.into(),
                owner_user_id: None,
                subject_type: Some("task".into()),
                subject_id: None,
                state: RunState::Queued,
                phase: String::new(),
                checkpoint: Value::Null,
                input: Value::Null,
                result: Value::Null,
                error: None,
                attempt: 0,
                lease_owner: None,
                lease_expires_at: None,
                approval_key: None,
                decision: None,
                created_at: String::new(),
                updated_at: String::new(),
                started_at: None,
                finished_at: None,
            })
        })
    }
    fn get<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<Option<RunRow>, sqlx::Error>> {
        Box::pin(async { Ok(None) })
    }
    fn claim<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: i64,
    ) -> BoxFuture<'a, Result<ClaimOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn heartbeat<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: i64,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn checkpoint<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: Value,
        _: String,
        _: bool,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn phase<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn complete<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: Value,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn fail<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn park<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: RunDecision,
        _: String,
        _: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn defer<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: i64,
        _: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn release<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        unreachable_driven()
    }
    fn answer<'a>(
        &'a self,
        _: &'a str,
        _: DecisionAnswer,
    ) -> BoxFuture<'a, Result<AnswerOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn cancel<'a>(
        &'a self,
        _: &'a str,
        _: Option<String>,
    ) -> BoxFuture<'a, Result<CancelOutcome, sqlx::Error>> {
        unreachable_driven()
    }
    fn due<'a>(&'a self, _: i64) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
        unreachable_driven()
    }
    fn active_for<'a>(
        &'a self,
        _: &'a str,
        _: Option<i64>,
    ) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
        unreachable_driven()
    }
}

fn unreachable_driven<'a, T>() -> BoxFuture<'a, T> {
    Box::pin(async { unreachable!("the dispatch walk drives nothing") })
}

struct NoLease;
impl RunLease for NoLease {
    fn acquire<'a>(&'a self, _: &'a str, _: i64) -> BoxFuture<'a, LeaseClaim> {
        Box::pin(async { LeaseClaim::Busy })
    }
    fn renew<'a>(&'a self, _: &'a str, _: &'a str, _: i64) -> BoxFuture<'a, LeaseRenewal> {
        Box::pin(async { LeaseRenewal::Ok })
    }
    fn release<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// Store-only RunDeps: the recorder is the observable, the drive stops at
/// NoDefinition, the lease edge is never reached.
fn recorder_run_deps() -> (RunDeps, Arc<DispatchRecorder>) {
    let recorder = Arc::new(DispatchRecorder {
        inserted: Mutex::new(Vec::new()),
    });
    let deps = RunDeps {
        store: recorder.clone(),
        lease: Arc::new(NoLease),
        publish: Arc::new(|_, _| {}),
        pause: Arc::new(|_| {
            Box::pin(async {
                PauseOutcome::Refused {
                    reason: WriteFailure::Missing,
                    state: None,
                }
            })
        }),
        definition_for: Arc::new(|_| None),
        now: Arc::new(|| 0),
        new_id: Arc::new(|| "new".into()),
    };
    (deps, recorder)
}

fn task_deps(pg: &PgPool, run_deps: RunDeps) -> TaskDeps {
    TaskDeps {
        pg: pg.clone(),
        // No Redis anywhere: realtime publishes nothing, notifications still
        // land as rows, dispatch rides the store-only fake.
        realtime: talaria_api::realtime::RealtimeDeps::publish_only(None),
        notify: talaria_api::notify::NotifyDeps::publishing(pg.clone(), None),
        dispatch: Some(run_deps),
    }
}

/// The detached dispatch push needs a beat to land; the longest recorded
/// insert ever observed was well under this. No insert after the wait is the
/// assertion — a flaky-slow insert would rather fail loudly than pass
/// silently.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn assigning_an_agent_to_an_intake_ticket_moves_nothing_and_dispatches_nothing() {
    let pg = pool().await;
    let fx = fixture(&pg, "assign").await;

    // A PERSON assigns an agent — TaskActor::human, exactly what the
    // assignee picker's PATCH produces.
    let (run_deps, recorder) = recorder_run_deps();
    let deps = task_deps(&pg, run_deps);
    update_task(
        &deps,
        &fx.task_id,
        TaskPatch {
            assignees: Some(vec!["doug-engineering".to_string()]),
            ..TaskPatch::default()
        },
        &TaskActor::human("the-picker"),
    )
    .await
    .unwrap_or_else(|e| panic!("the assignment lands: {}", e.message()));
    settle().await;

    // The dispatch push DID fire (the patch gained an agent assignee) — and
    // the pickup-column gate inside maybe_dispatch_ticket stood it down,
    // because the ticket never left intake. No run, no session, no work.
    assert!(
        recorder.inserted.lock().unwrap().is_empty(),
        "an assignment alone started a session: {:?}",
        recorder.inserted.lock().unwrap()
    );

    // The ticket stayed in intake, with the name filed on the card.
    let task = get_task(&pg, &fx.task_id)
        .await
        .unwrap()
        .expect("task there");
    assert_eq!(task.status, "inbox", "assignment moved the ticket");
    assert_eq!(
        task.assignees,
        vec!["doug-engineering".to_string()],
        "the assignee must still be filed on the card"
    );

    // The activity record says assigned — and nothing else. No "moved to".
    let activity = list_activity(&pg, &fx.task_id).await.unwrap();
    let assigned: Vec<&str> = activity
        .iter()
        .filter(|a| a.kind_tag == "assigned")
        .map(|a| a.description.as_str())
        .collect();
    assert!(
        assigned.iter().any(|d| d.contains("doug-engineering")),
        "the assignment was not recorded: {assigned:?}"
    );
    let moves: Vec<&str> = activity
        .iter()
        .filter(|a| a.kind_tag == "status")
        .map(|a| a.description.as_str())
        .collect();
    assert!(
        moves.is_empty(),
        "an assignment wrote a column-move line: {moves:?}"
    );

    cleanup(&pg, "assign").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_explicit_move_into_the_pickup_column_still_dispatches_every_agent_assignee() {
    let pg = pool().await;
    let fx = fixture(&pg, "move").await;

    // Pre-assign TWO agents so the column move alone is what is under test.
    // The assignment itself must stay silent (the gate above it).
    let (quiet_deps, quiet) = recorder_run_deps();
    let deps = task_deps(&pg, quiet_deps);
    update_task(
        &deps,
        &fx.task_id,
        TaskPatch {
            assignees: Some(vec!["doug-engineering".into(), "some-other-agent".into()]),
            ..TaskPatch::default()
        },
        &TaskActor::human("the-picker"),
    )
    .await
    .unwrap_or_else(|e| panic!("the pre-assignment lands: {}", e.message()));
    settle().await;
    assert!(
        quiet.inserted.lock().unwrap().is_empty(),
        "pre-condition failed: assigning already dispatched"
    );

    // THE HUMAN MOVE: the same patch the drag produces, status named.
    let (run_deps, recorder) = recorder_run_deps();
    let deps = task_deps(&pg, run_deps);
    update_task(
        &deps,
        &fx.task_id,
        TaskPatch {
            status: Some("assigned".to_string()),
            ..TaskPatch::default()
        },
        &TaskActor::human("the-drag"),
    )
    .await
    .unwrap_or_else(|e| panic!("the column move lands: {}", e.message()));
    settle().await;

    // Entering the pickup queue is approval for EVERY agent assignee: one
    // session per agent, generation zero each.
    let mut inserted = recorder.inserted.lock().unwrap().clone();
    inserted.sort();
    let mut expected = vec![
        session_run_id(&fx.task_id, "doug-engineering", 0),
        session_run_id(&fx.task_id, "some-other-agent", 0),
    ];
    expected.sort();
    assert_eq!(
        inserted, expected,
        "the pickup move dispatched the wrong sessions"
    );

    // The move itself is on the record, once, as a status line.
    let activity = list_activity(&pg, &fx.task_id).await.unwrap();
    let moves: Vec<&str> = activity
        .iter()
        .filter(|a| a.kind_tag == "status")
        .map(|a| a.description.as_str())
        .collect();
    assert_eq!(
        moves,
        vec!["moved to assigned"],
        "the move wrote: {moves:?}"
    );

    cleanup(&pg, "move").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_new_agent_assignee_on_a_ticket_already_in_pickup_gets_only_their_own_push() {
    let pg = pool().await;
    let fx = fixture(&pg, "add").await;

    // Phase 1: assign the first agent — the intake gate stands it down.
    let (first_deps, first) = recorder_run_deps();
    let deps = task_deps(&pg, first_deps);
    update_task(
        &deps,
        &fx.task_id,
        TaskPatch {
            assignees: Some(vec!["doug-engineering".into()]),
            ..TaskPatch::default()
        },
        &TaskActor::human("the-picker"),
    )
    .await
    .unwrap_or_else(|e| panic!("the first assignment lands: {}", e.message()));
    settle().await;
    assert!(
        first.inserted.lock().unwrap().is_empty(),
        "pre-condition failed: the first assignment dispatched"
    );

    // Phase 2: the human move into the pickup column — the first agent's
    // session starts (this is the previous test's territory, asserted here
    // as this test's own pre-condition).
    let (move_deps, moved) = recorder_run_deps();
    let deps = task_deps(&pg, move_deps);
    update_task(
        &deps,
        &fx.task_id,
        TaskPatch {
            status: Some("assigned".to_string()),
            ..TaskPatch::default()
        },
        &TaskActor::human("the-drag"),
    )
    .await
    .unwrap_or_else(|e| panic!("the move lands: {}", e.message()));
    settle().await;
    assert_eq!(
        moved.inserted.lock().unwrap().clone(),
        vec![session_run_id(&fx.task_id, "doug-engineering", 0)],
        "pre-condition failed: the move did not start the first agent's session"
    );

    // Phase 3: a SECOND agent joins a ticket already sitting in the queue.
    // Entering is approval for everyone; GAINING an assignee is approval for
    // that one only — the first agent's live session must not be restarted.
    let (join_deps, joined) = recorder_run_deps();
    let deps = task_deps(&pg, join_deps);
    update_task(
        &deps,
        &fx.task_id,
        TaskPatch {
            assignees: Some(vec!["doug-engineering".into(), "late-arrival".into()]),
            ..TaskPatch::default()
        },
        &TaskActor::human("the-picker"),
    )
    .await
    .unwrap_or_else(|e| panic!("the second assignment lands: {}", e.message()));
    settle().await;
    assert_eq!(
        joined.inserted.lock().unwrap().clone(),
        vec![session_run_id(&fx.task_id, "late-arrival", 0)],
        "the joiner pushed the wrong sessions"
    );

    cleanup(&pg, "add").await;
}
