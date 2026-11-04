// Live-DB proof of runs/store.rs (cargo test -- --ignored). Every write in
// the store is a compare-and-set whose correctness is a WHERE clause — a
// typo'd predicate does not fail a unit test, it silently lets two drivers
// finish one run. These tests walk the whole CAS lifecycle against the real
// table: the claim race, the reclaim attempt-increment, the lease-loss
// refusal, the park/answer/stale-key path, the deferral wait, and
// cancel-from-anywhere. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test it runs_store:: -- --ignored

use crate::support::pg;
use serde_json::json;
use sqlx::postgres::PgPool;
use talaria_api::runs::define::{
    DecisionAnswer, DecisionOption, DecisionRequest, RunDecision, RunState,
};
use talaria_api::runs::store::{
    AnswerOutcome, CancelOutcome, CheckpointKind, ClaimOutcome, NewRun, PgRunStore, RunStore,
    WriteFailure, agent_turn_of, run_checkpoints, set_agent_run_id,
};
/// One run of the test kind; earlier crashed runs of this suite are swept so
/// the unique approval_key index never bites the next attempt.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from runs where kind = 'rust-store-test'")
        .execute(pg)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_cas_lifecycle_refuses_every_wrong_writer() {
    let pg = pg().await;
    cleanup(&pg).await;
    let store = PgRunStore::new(pg.clone());

    let id = uuid::Uuid::new_v4().to_string();
    let run = store
        .insert(NewRun {
            id: id.clone(),
            kind: "rust-store-test".into(),
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

    cleanup(&pg).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_late_turn_cannot_overwrite_a_newer_turns_agent_run_id() {
    let pg = pg().await;
    let store = PgRunStore::new(pg.clone());
    let id = uuid::Uuid::new_v4().to_string();
    store
        .insert(NewRun {
            id: id.clone(),
            kind: "rust-store-agent-run-test".into(),
            owner_user_id: None,
            subject_type: None,
            subject_id: None,
            input: json!({"agentModel": "atlas"}),
            phase: "starting".into(),
        })
        .await
        .unwrap();
    let token = "owner-a";
    store.claim(&id, token, 60_000).await.unwrap();
    store
        .checkpoint(
            &id,
            token,
            json!({"stage": "send", "turn": 2}),
            "t".into(),
            false,
        )
        .await
        .unwrap()
        .unwrap();

    let state = set_agent_run_id(&pg, &id, 2, Some("run_new"))
        .await
        .unwrap();
    assert_eq!(state, Some(RunState::Running));
    set_agent_run_id(&pg, &id, 1, Some("run_old"))
        .await
        .unwrap();

    let (_, named) = agent_turn_of(&pg, &id).await.unwrap().unwrap();
    assert_eq!(named.as_deref(), Some("run_new"));

    store.cancel(&id, None).await.unwrap();
    let state = set_agent_run_id(&pg, &id, 1, Some("run_old"))
        .await
        .unwrap();
    assert_eq!(
        state,
        Some(RunState::Cancelled),
        "a refused write still reports the cancel, so its turn gets stopped"
    );
    sqlx::query("delete from runs where id = $1::uuid")
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn due_finds_the_unleased_and_kind_views_read_the_latest() {
    use talaria_api::runs::store::{active_run_of_kind, latest_run_of_kind};

    let pg = pg().await;
    let kind = "rust-store-test-due"; // own kind: this suite runs its tests in parallel
    sqlx::query("delete from runs where kind = $1")
        .bind(kind)
        .execute(&pg)
        .await
        .unwrap();
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

async fn claimed_run(store: &PgRunStore) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    store
        .insert(NewRun {
            id: id.clone(),
            kind: "rust-store-test".into(),
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
    cleanup(&pg).await;
    let store = PgRunStore::new(pg.clone());
    let (id, token) = claimed_run(&store).await;

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
    assert_eq!(
        rows.last().unwrap().checkpoint.as_ref(),
        Some(&run.checkpoint)
    );
    assert_eq!(run.phase, "swept");

    cleanup(&pg).await;
    assert!(
        run_checkpoints(&pg, &id).await.unwrap().is_empty(),
        "the ledger goes with its run"
    );
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_lost_lease_write_leaves_no_ledger_row() {
    let pg = pg().await;
    cleanup(&pg).await;
    let store = PgRunStore::new(pg.clone());
    let (id, token) = claimed_run(&store).await;
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

    cleanup(&pg).await;
}
