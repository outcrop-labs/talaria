// Live-DB proof of runs/store.rs (cargo test -- --ignored). Every write in
// the store is a compare-and-set whose correctness is a WHERE clause — a
// typo'd predicate does not fail a unit test, it silently lets two drivers
// finish one run. These tests walk the whole CAS lifecycle against the real
// table: the claim race, the reclaim attempt-increment, the lease-loss
// refusal, the park/answer/stale-key path, the deferral wait, and
// cancel-from-anywhere. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it runs_store:: -- --ignored

use crate::support::pg;
use serde_json::json;
use sqlx::postgres::PgPool;
use talaria_api::runs::define::{
    DecisionAnswer, DecisionOption, DecisionRequest, RunDecision, RunState,
};
use talaria_api::runs::store::{
    AnswerOutcome, CancelOutcome, CheckpointKind, ClaimOutcome, NewRun, PgRunStore, RunStore,
    StepSnapshot, WriteFailure, record_step, retire_compacted_steps, retire_job_steps,
    run_checkpoints,
};
/// Every test passes its own kind: the suite runs in parallel, and this deletes
/// all of a kind's runs.
async fn cleanup(pg: &PgPool, kind: &str) {
    sqlx::query("delete from runs where kind = $1")
        .bind(kind)
        .execute(pg)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_cas_lifecycle_refuses_every_wrong_writer() {
    let pg = pg().await;
    let kind = "rust-store-test-cas";
    cleanup(&pg, kind).await;
    let store = PgRunStore::new(pg.clone());

    let id = uuid::Uuid::new_v4().to_string();
    let run = store
        .insert(NewRun {
            id: id.clone(),
            kind: kind.into(),
            owner_user_id: None,
            subject_type: None,
            subject_id: None,
            input: json!({"n": 1}),
            phase: "starting".into(),
        })
        .await
        .unwrap();
    assert_eq!(run.state, RunState::Queued);
    assert_eq!(run.attempt, 0);
    assert_eq!(run.phase, "starting");

    // First claim wins; `started_at` is stamped once.
    let a = format!("driver-a:{}", uuid::Uuid::new_v4());
    match store.claim(&id, &a, 60_000).await.unwrap() {
        ClaimOutcome::Claimed { run, reclaimed } => {
            assert!(!reclaimed);
            assert_eq!(run.attempt, 0);
            assert!(run.started_at.is_some());
            assert_eq!(run.lease_owner.as_deref(), Some(a.as_str()));
        }
        other => panic!("first claim must win, got {other:?}"),
    }

    // A live lease: the second driver is refused without touching the row.
    let b = format!("driver-b:{}", uuid::Uuid::new_v4());
    match store.claim(&id, &b, 60_000).await.unwrap() {
        ClaimOutcome::Taken { state, until } => {
            assert_eq!(state, RunState::Running);
            assert!(until.is_some(), "the live lease's expiry is the wait");
        }
        other => panic!("second claim must be taken, got {other:?}"),
    }

    // Only the leaseholder writes. Driver B's checkpoint is refused with the
    // reason that means "stop, cleanly".
    store
        .checkpoint(&id, &a, json!({"step": 1}), "step 1".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .checkpoint(&id, &b, json!({"step": 999}), "hijack".into(), false)
            .await
            .unwrap(),
        Err(WriteFailure::LeaseLost {
            state: RunState::Running
        })
    );
    // …and the hijack wrote nothing: the row still holds driver A's step.
    assert_eq!(
        store.get(&id).await.unwrap().unwrap().checkpoint,
        json!({"step": 1})
    );

    // THE RECLAIM: expire the lease out from under driver A (a crash, a
    // deploy, a container paused past its TTL), and the next claim both wins
    // and COUNTS — `attempt` moves on reclaim, and only on reclaim.
    sqlx::query(
        "update runs set lease_expires_at = now() - interval '1 second' where id = $1::uuid",
    )
    .bind(&id)
    .execute(&pg)
    .await
    .unwrap();
    match store.claim(&id, &b, 60_000).await.unwrap() {
        ClaimOutcome::Claimed { run, reclaimed } => {
            assert!(
                reclaimed,
                "a claim on a running row with an expired lease is a reclaim"
            );
            assert_eq!(run.attempt, 1);
            assert_eq!(
                run.checkpoint,
                json!({"step": 1}),
                "reclaim resumes from the last persisted checkpoint"
            );
            assert_eq!(
                run.started_at,
                store.get(&id).await.unwrap().unwrap().started_at
            );
        }
        other => panic!("expired-lease claim must reclaim, got {other:?}"),
    }

    // Park: lease released (the row is nobody's while it waits for a human),
    // decision + approval key written.
    let question = RunDecision {
        request: DecisionRequest {
            key: "which".into(),
            question: "Which source?".into(),
            detail: None,
            options: vec![
                DecisionOption {
                    id: "a".into(),
                    label: "A".into(),
                    detail: None,
                },
                DecisionOption {
                    id: "b".into(),
                    label: "B".into(),
                    detail: None,
                },
            ],
            href: None,
            free_text: false,
        },
        answer: None,
    };
    store
        .park(
            &id,
            &b,
            question.clone(),
            format!("approval:{id}"),
            "asking".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let parked = store.get(&id).await.unwrap().unwrap();
    assert_eq!(parked.state, RunState::Awaiting);
    assert_eq!(parked.lease_owner, None);
    assert_eq!(
        parked.approval_key.as_deref(),
        Some(format!("approval:{id}").as_str())
    );

    // An answer naming a different question is refused — the stale-tab case.
    let stale = DecisionAnswer {
        key: "some-old-question".into(),
        option_id: "a".into(),
        note: None,
        answered_by: None,
        answered_at: "2026-08-29T00:00:00.000Z".into(),
    };
    assert!(matches!(
        store.answer(&id, stale).await.unwrap(),
        AnswerOutcome::StaleKey { .. }
    ));

    // The right answer re-queues the run with the answer IN the row.
    let right = DecisionAnswer {
        key: "which".into(),
        option_id: "b".into(),
        note: Some("b is the live one".into()),
        answered_by: Some("some-user".into()),
        answered_at: "2026-08-29T00:01:00.000Z".into(),
    };
    match store.answer(&id, right).await.unwrap() {
        AnswerOutcome::Answered(run) => {
            assert_eq!(run.state, RunState::Queued);
            let d = run.decision.expect("the question stays with the answer");
            assert_eq!(d.answer.as_ref().unwrap().option_id, "b");
        }
        other => panic!("the right answer must land, got {other:?}"),
    }

    // Deferral: back to queued but not takeable until `until` — the lease
    // stamp IS the wait.
    let c = format!("driver-c:{}", uuid::Uuid::new_v4());
    store.claim(&id, &c, 60_000).await.unwrap().unwrap_claimed();
    store
        .defer(&id, &c, now_ms() + 300_000, "rate limited".into())
        .await
        .unwrap()
        .unwrap();
    match store.claim(&id, &a, 60_000).await.unwrap() {
        ClaimOutcome::Taken { until, .. } => assert!(until.is_some()),
        other => panic!("a deferred run must not be takeable, got {other:?}"),
    }

    // Cancel from ANYWHERE — no lease predicate, and the deferred row (leased
    // to C, in the future) is still cancellable from A.
    match store
        .cancel(&id, Some("changed my mind".into()))
        .await
        .unwrap()
    {
        CancelOutcome::Cancelled { state } => assert_eq!(state, RunState::Cancelled),
        other => panic!("cancel must work from anywhere, got {other:?}"),
    }
    // THE PREDICATE IS THE CANCELLATION CHECK: driver C's terminal write is
    // refused, so a cancelled run cannot be finished by the process that was
    // driving it when the button was pressed.
    assert_eq!(
        store.complete(&id, &c, json!("too late")).await.unwrap(),
        Err(WriteFailure::Cancelled)
    );
    // And cancelling again says terminal, not missing.
    assert!(matches!(
        store.cancel(&id, None).await.unwrap(),
        CancelOutcome::Terminal { .. }
    ));

    cleanup(&pg, kind).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn due_finds_the_unleased_and_kind_views_read_the_latest() {
    use talaria_api::runs::store::{active_run_of_kind, latest_run_of_kind};

    let pg = pg().await;
    let kind = "rust-store-test-due";
    cleanup(&pg, kind).await;
    let store = PgRunStore::new(pg.clone());

    let id = uuid::Uuid::new_v4().to_string();
    store
        .insert(NewRun {
            id: id.clone(),
            kind: kind.into(),
            owner_user_id: None,
            subject_type: None,
            subject_id: None,
            input: json!({}),
            phase: "queued".into(),
        })
        .await
        .unwrap();

    // Fresh + unleased = due (the sweep would drive it).
    let due = store.due(25).await.unwrap();
    assert!(
        due.iter().any(|r| r.id == id),
        "a queued unleased run is due"
    );

    // Claimed with a live lease = not due.
    store
        .claim(&id, "solo-driver", 60_000)
        .await
        .unwrap()
        .unwrap_claimed();
    let due = store.due(25).await.unwrap();
    assert!(!due.iter().any(|r| r.id == id), "a leased run is not due");

    // Kind views: latest of the kind, and active while not terminal.
    let latest = latest_run_of_kind(&pg, kind).await.unwrap().unwrap();
    assert_eq!(latest.id, id);
    assert_eq!(latest.state, RunState::Running);
    assert!(active_run_of_kind(&pg, kind).await.unwrap().is_some());

    store.cancel(&id, None).await.unwrap();
    let latest = latest_run_of_kind(&pg, kind).await.unwrap().unwrap();
    assert_eq!(latest.state, RunState::Cancelled);
    assert!(
        active_run_of_kind(&pg, kind).await.unwrap().is_none(),
        "a cancelled run is not active"
    );

    sqlx::query("delete from runs where kind = $1")
        .bind(kind)
        .execute(&pg)
        .await
        .unwrap();
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Test-only: ClaimOutcome::Claimed carries a Box<RunRow>; assert it landed
/// and drop the row.
trait ClaimAssert {
    fn unwrap_claimed(self);
}

impl ClaimAssert for ClaimOutcome {
    fn unwrap_claimed(self) {
        assert!(
            matches!(self, ClaimOutcome::Claimed { .. }),
            "expected Claimed, got {self:?}"
        );
    }
}

async fn claimed_run(store: &PgRunStore, kind: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    store
        .insert(NewRun {
            id: id.clone(),
            kind: kind.into(),
            owner_user_id: None,
            subject_type: None,
            subject_id: None,
            input: json!({}),
            phase: "starting".into(),
        })
        .await
        .unwrap();
    let token = format!("driver:{}", uuid::Uuid::new_v4());
    match store.claim(&id, &token, 60_000).await.unwrap() {
        ClaimOutcome::Claimed { .. } => (id, token),
        other => panic!("a fresh run must be claimable, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn n_checkpoints_yield_n_ledger_rows_and_the_run_holds_the_last() {
    let pg = pg().await;
    let kind = "rust-store-test-ledger";
    cleanup(&pg, kind).await;
    let store = PgRunStore::new(pg.clone());
    let (id, token) = claimed_run(&store, kind).await;

    for turn in 1..=3 {
        let checkpoint = json!({"stage": "send", "turn": turn, "stageAttempt": 0});
        store
            .checkpoint(&id, &token, checkpoint, format!("turn {turn}"), false)
            .await
            .unwrap()
            .unwrap();
    }
    store
        .checkpoint(&id, &token, json!({"cursor": 9}), "swept".into(), false)
        .await
        .unwrap()
        .unwrap();

    let rows = run_checkpoints(&pg, &id).await.unwrap();
    assert_eq!(
        rows.iter().map(|r| r.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4],
        "one gapless row per checkpoint, oldest first"
    );
    assert!(rows.iter().all(|r| r.kind == CheckpointKind::Turn));
    assert_eq!(rows[1].turn, Some(2));
    assert_eq!(rows[1].status.as_deref(), Some("send"));
    assert_eq!(rows[1].title.as_deref(), Some("turn 2"));
    assert_eq!(rows[3].turn, None);
    assert_eq!(rows[3].status, None);
    assert!(!rows[0].restorable, "a turn boundary restores nothing");
    assert!(rows[0].unrestorable_reason.is_some());

    let run = store.get(&id).await.unwrap().unwrap();
    assert_eq!(run.checkpoint, json!({"cursor": 9}));
    assert_eq!(run.phase, "swept");

    cleanup(&pg, kind).await;
    assert!(
        run_checkpoints(&pg, &id).await.unwrap().is_empty(),
        "the ledger goes with its run"
    );
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_lost_lease_write_leaves_no_ledger_row() {
    let pg = pg().await;
    let kind = "rust-store-test-lost-lease";
    cleanup(&pg, kind).await;
    let store = PgRunStore::new(pg.clone());
    let (id, token) = claimed_run(&store, kind).await;
    store
        .checkpoint(&id, &token, json!({"turn": 1}), "turn 1".into(), false)
        .await
        .unwrap()
        .unwrap();

    let refused = store
        .checkpoint(
            &id,
            "someone-else",
            json!({"turn": 2}),
            "hijack".into(),
            false,
        )
        .await
        .unwrap();
    assert!(matches!(refused, Err(WriteFailure::LeaseLost { .. })));

    store.cancel(&id, None).await.unwrap();
    let cancelled = store
        .checkpoint(&id, &token, json!({"turn": 3}), "late".into(), false)
        .await
        .unwrap();
    assert_eq!(cancelled, Err(WriteFailure::Cancelled));

    let rows = run_checkpoints(&pg, &id).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "only the leaseholder's write is in the ledger"
    );
    assert_eq!(rows[0].turn, Some(1));
    let seq: i64 = sqlx::query_scalar("select checkpoint_seq from runs where id = $1::uuid")
        .bind(&id)
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(seq, 1, "a refused write allocates no seq");

    cleanup(&pg, kind).await;
}

fn step(call: &str, tool: &str, at_ms: i64, job_id: Option<&str>) -> StepSnapshot {
    step_in("hermes-turn", call, tool, at_ms, job_id)
}

fn step_in(
    turn_id: &str,
    call: &str,
    tool: &str,
    at_ms: i64,
    job_id: Option<&str>,
) -> StepSnapshot {
    StepSnapshot {
        agent_turn_id: Some(turn_id.into()),
        tool_call_id: call.into(),
        tool: tool.into(),
        title: None,
        status: "completed".into(),
        job_id: job_id.map(str::to_string),
        workspace_commit: Some("a".repeat(40)),
        workspace_tree: Some("b".repeat(40)),
        unrestorable_reason: None,
        at_ms,
    }
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn step_rows_share_the_seq_allocator_dedupe_by_call_and_keep_their_order_across_a_compaction()
{
    let pg = pg().await;
    let kind = "rust-store-test-steps";
    cleanup(&pg, kind).await;
    let store = PgRunStore::new(pg.clone());
    let (id, token) = claimed_run(&store, kind).await;
    let job = uuid::Uuid::new_v4().to_string();
    // The hook's clock runs well behind the driver's: the test proves that
    // rank and step number order a turn, not the wall clock.
    let past = 1_700_000_000_000i64;
    let place = |rows: &[talaria_api::runs::store::RunCheckpoint], call: &str| {
        let row = rows
            .iter()
            .find(|r| r.tool_call_id.as_deref() == Some(call))
            .unwrap();
        (row.turn, row.step_no)
    };

    let turn = |n: i64, stage: &str| json!({"stage": stage, "turn": n, "stageAttempt": 0});
    store
        .checkpoint(&id, &token, turn(1, "send"), "pushed".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record_step(&pg, &id, &step("c1", "terminal", past + 1_000, None))
            .await
            .unwrap(),
        Some(2),
        "a step takes the next seq after the turn row"
    );
    assert_eq!(
        record_step(
            &pg,
            &id,
            &step("c2", "write_file", past + 2_000, Some(&job))
        )
        .await
        .unwrap(),
        Some(3)
    );
    assert_eq!(
        record_step(
            &pg,
            &id,
            &step("c2", "write_file", past + 2_000, Some(&job))
        )
        .await
        .unwrap(),
        None,
        "the frame the ledger already holds adds no row"
    );
    store
        .checkpoint(
            &id,
            &token,
            turn(1, "record"),
            "turn 1 answered".into(),
            false,
        )
        .await
        .unwrap()
        .unwrap();
    store
        .checkpoint(
            &id,
            &token,
            turn(2, "send"),
            "turn 1 recorded".into(),
            false,
        )
        .await
        .unwrap()
        .unwrap();
    // A frame of turn 1 that lands after turn 2 opened still joins turn 1:
    // its Hermes turn id is the one turn 1's rows carry.
    assert_eq!(
        record_step(&pg, &id, &step("c0", "read_file", past + 500, None))
            .await
            .unwrap(),
        Some(6)
    );
    // Turn 2's first step carries a new Hermes turn id and takes the turn
    // the run is on.
    assert_eq!(
        record_step(
            &pg,
            &id,
            &step_in("hermes-turn-2", "d1", "terminal", past + 3_000, Some(&job))
        )
        .await
        .unwrap(),
        Some(7)
    );
    let (allocated,): (i64,) =
        sqlx::query_as("select checkpoint_seq from runs where id = $1::uuid")
            .bind(&id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(allocated, 7, "a duplicate does not spend a seq");

    let rows = run_checkpoints(&pg, &id).await.unwrap();
    assert_eq!(place(&rows, "c1"), (Some(1), Some(1)));
    assert_eq!(place(&rows, "c2"), (Some(1), Some(2)));
    assert_eq!(
        place(&rows, "c0"),
        (Some(1), Some(3)),
        "numbered on arrival, in its own turn"
    );
    assert_eq!(place(&rows, "d1"), (Some(2), Some(1)));
    assert_eq!(
        rows.iter().map(|r| r.seq).collect::<Vec<_>>(),
        [1, 2, 3, 6, 4, 5, 7],
        "a turn's send row, its steps by step number, its record row; then the next turn"
    );
    assert_eq!(rows[1].kind, CheckpointKind::Step);
    assert_eq!(rows[1].tool.as_deref(), Some("terminal"));
    assert!(
        !rows[1].restorable,
        "a step with no job of the agent restores nothing"
    );
    assert_eq!(
        rows[1].unrestorable_reason.as_deref(),
        Some("the step named no job of this agent")
    );
    assert!(rows[2].restorable);
    assert_eq!(rows[2].job_id.as_deref(), Some(job.as_str()));
    assert_eq!(
        rows[1].created_at, "2023-11-14T22:13:21.000Z",
        "the hook's clock is the row's"
    );

    // A compaction took c2's conversation row out of the live transcript:
    // the step says so. c1 is still there; d1 landed after the read and is
    // not judged by it. A second pass changes nothing.
    let through = talaria_api::runs::store::checkpoint_seq(&pg, &id)
        .await
        .unwrap();
    let present = vec!["c1".to_string(), "c0".to_string(), "d1".to_string()];
    assert_eq!(
        retire_compacted_steps(&pg, &id, through - 1, &present)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        retire_compacted_steps(&pg, &id, through - 1, &present)
            .await
            .unwrap(),
        0
    );
    let rows = run_checkpoints(&pg, &id).await.unwrap();
    let c2 = rows
        .iter()
        .find(|r| r.tool_call_id.as_deref() == Some("c2"))
        .unwrap();
    assert!(!c2.restorable);
    assert!(
        c2.unrestorable_reason
            .as_deref()
            .unwrap()
            .starts_with("compacted:"),
        "{c2:?}"
    );
    let d1 = rows
        .iter()
        .find(|r| r.tool_call_id.as_deref() == Some("d1"))
        .unwrap();
    assert!(d1.restorable, "a frame past the bound is not judged");
    // Steps after the compaction keep counting up within their turn.
    assert_eq!(
        record_step(&pg, &id, &step("c3", "terminal", past + 4_000, Some(&job)))
            .await
            .unwrap(),
        Some(8)
    );
    let rows = run_checkpoints(&pg, &id).await.unwrap();
    assert_eq!(place(&rows, "c3"), (Some(1), Some(4)));

    assert_eq!(
        retire_job_steps(&pg, &job, "the job's checkout was removed")
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        retire_job_steps(&pg, &job, "again").await.unwrap(),
        0,
        "a retired row keeps its reason"
    );

    // A clock the server cannot hold, or one ahead of it, is the server's
    // present rather than a dropped frame.
    let before: i64 = sqlx::query_scalar("select (extract(epoch from now()) * 1000)::bigint")
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(
        record_step(&pg, &id, &step("c-far", "terminal", i64::MAX, None))
            .await
            .unwrap(),
        Some(9)
    );
    assert_eq!(
        record_step(&pg, &id, &step("c-neg", "terminal", -5, None))
            .await
            .unwrap(),
        Some(10)
    );
    let (far, neg): (i64, i64) = sqlx::query_as(
        "select (extract(epoch from max(created_at) filter (where tool_call_id = 'c-far')) * 1000)::bigint, \
                (extract(epoch from max(created_at) filter (where tool_call_id = 'c-neg')) * 1000)::bigint \
         from run_checkpoints where run_id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(far >= before && far <= before + 60_000, "{far} vs {before}");
    assert!(neg >= before && neg <= before + 60_000, "{neg} vs {before}");

    // The step after the run's last tool call lands after the run ended, in
    // a turn no row has seen: it takes the last closed turn, after that
    // turn's last step.
    store
        .checkpoint(
            &id,
            &token,
            turn(2, "record"),
            "turn 2 answered".into(),
            false,
        )
        .await
        .unwrap()
        .unwrap();
    store
        .complete(&id, &token, json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record_step(
            &pg,
            &id,
            &step_in("hermes-turn-3", "c-last", "terminal", past + 9_000, None)
        )
        .await
        .unwrap(),
        Some(12)
    );
    let rows = run_checkpoints(&pg, &id).await.unwrap();
    assert_eq!(place(&rows, "c-last"), (Some(2), Some(2)));
    assert_eq!(
        rows.iter().map(|r| r.seq).collect::<Vec<_>>(),
        [1, 2, 3, 6, 8, 9, 10, 4, 5, 7, 12, 11],
        "turn 1 in step order, then turn 2's send row, its steps, the late step, its record row"
    );

    cleanup(&pg, kind).await;
}
