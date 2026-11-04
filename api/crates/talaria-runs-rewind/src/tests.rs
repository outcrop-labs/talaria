// The stage machine against a fake world: every stage in order, a crash after
// each one, every refusal, and the mismatches that fail a rewind.

use std::sync::{Arc, Mutex};

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use talaria_runs_define::{RunRow, RunState, RunStepContext, StepActivity, StepResult, StepSignal};
use talaria_runs_store::{CheckpointKind, RunCheckpoint, RunLineage};
use talaria_runs_work_session::WorkSessionInput;

use super::*;

struct Fake {
    calls: Mutex<Vec<String>>,
    failed: Mutex<Option<(Option<Stage>, bool)>>,
    source: Mutex<Result<Source, String>>,
    reach: Result<String, String>,
    reserve: Result<Reservation, String>,
    quiet: Quiet,
    fork: Result<Fork, String>,
    cut: Result<Cut, String>,
    restore: Result<String, String>,
    tail: Result<Option<String>, String>,
    checkout: Result<Checkout, String>,
    start: Result<(), String>,
}

impl Fake {
    fn new() -> Self {
        Fake {
            calls: Mutex::new(Vec::new()),
            failed: Mutex::new(None),
            source: Mutex::new(Ok(source())),
            reach: Ok("/opt/data/workbench/jobs/job-1".into()),
            reserve: Ok(reservation()),
            quiet: Quiet::Confirmed,
            fork: Ok(Fork::Created),
            cut: Ok(Cut {
                was: 12,
                now: 7,
                last_tool_call_id: Some("call_7".into()),
            }),
            restore: Ok("tree-7".into()),
            tail: Ok(Some("call_7".into())),
            checkout: Ok(Checkout {
                tree: "tree-7".into(),
                branch: "tala-1-work-a2".into(),
                clean: true,
            }),
            start: Ok(()),
        }
    }

    fn with_source(self, source: Result<Source, String>) -> Self {
        *self.source.lock().unwrap() = source;
        self
    }

    fn log(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_string());
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn failed_at(&self) -> Option<(Option<Stage>, bool)> {
        *self.failed.lock().unwrap()
    }
}

impl RewindWorld for Fake {
    fn source<'a>(&'a self, _input: &'a RewindInput) -> BoxFuture<'a, Result<Source, String>> {
        self.log("source");
        let answer = self.source.lock().unwrap().clone();
        async move { answer }.boxed()
    }
    fn reach<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<String, String>> {
        self.log("reach");
        async move { self.reach.clone() }.boxed()
    }
    fn reserve<'a>(
        &'a self,
        _: &'a str,
        _: &'a RewindInput,
        _: &'a Source,
        _: &'a Validated,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Reservation, String>> {
        self.log("reserve");
        async move { self.reserve.clone() }.boxed()
    }
    fn cancel_source<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        self.log("cancel");
        async move { Ok(()) }.boxed()
    }
    fn quiet<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Quiet> {
        self.log("quiet");
        async move { self.quiet.clone() }.boxed()
    }
    fn fork<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Fork, String>> {
        self.log("fork");
        async move { self.fork.clone() }.boxed()
    }
    fn cut<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<Cut, String>> {
        self.log("cut");
        async move { self.cut.clone() }.boxed()
    }
    fn restore<'a>(
        &'a self,
        _: &'a str,
        _: &'a RewindPlan,
    ) -> BoxFuture<'a, Result<String, String>> {
        self.log("restore");
        async move { self.restore.clone() }.boxed()
    }
    fn session_tail<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        self.log("tail");
        async move { self.tail.clone() }.boxed()
    }
    fn checkout<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<Checkout, String>> {
        self.log("checkout");
        async move { self.checkout.clone() }.boxed()
    }
    fn start<'a>(
        &'a self,
        _: &'a str,
        _: &'a RewindInput,
        _: &'a RewindPlan,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.log("start");
        async move { self.start.clone() }.boxed()
    }
    fn fail<'a>(
        &'a self,
        _: &'a str,
        _: &'a RewindInput,
        plan: Option<&'a RewindPlan>,
        stage: Option<Stage>,
    ) -> BoxFuture<'a, ()> {
        self.log("fail");
        *self.failed.lock().unwrap() = Some((stage, plan.is_some()));
        async move {}.boxed()
    }
}

fn step_row(seq: i64) -> RunCheckpoint {
    RunCheckpoint {
        seq,
        kind: CheckpointKind::Step,
        turn: Some(3),
        agent_turn_id: Some("t-3".into()),
        step_no: Some(2),
        tool_call_id: Some("call_7".into()),
        tool: Some("terminal".into()),
        title: Some("python3 test_calc.py".into()),
        status: Some("ok".into()),
        job_id: Some("job-1".into()),
        workspace_commit: Some("c0ffee".into()),
        workspace_tree: Some("tree-7".into()),
        restorable: true,
        unrestorable_reason: None,
        created_at: "2026-11-04T10:00:00.000Z".into(),
    }
}

fn ticket(column: TicketColumn) -> Ticket {
    Ticket {
        ticket_ref: Some("TALA-1".into()),
        title: "Add subtract".into(),
        status: "in_progress".into(),
        column,
        active_key: Some("in_progress".into()),
    }
}

fn source() -> Source {
    Source {
        run_id: "run-1".into(),
        seq: 7,
        input: WorkSessionInput {
            task_id: "t-1".into(),
            agent_model: "atlas".into(),
            board_id: "b-1".into(),
            board_name: Some("Rewind fixture".into()),
            generation: 0,
            rewind: None,
        },
        lineage: RunLineage {
            lineage_id: "run-1".into(),
            attempt_no: 1,
            rewound_from_run_id: None,
            rewound_from_seq: None,
        },
        row: Some(step_row(7)),
        ticket: Some(ticket(TicketColumn::Working)),
        job: Some(SourceJob {
            id: "job-1".into(),
            repo: "fixture/rewind-fixture".into(),
            host_id: "forgejo".into(),
            agent_id: "a-1".into(),
            effort: "light".into(),
            plan: String::new(),
        }),
    }
}

fn reservation() -> Reservation {
    Reservation {
        attempt_run_id: "run-2".into(),
        generation: 1,
        attempt_no: 2,
        job_id: "job-2".into(),
        workdir: "/opt/data/workbench/jobs/job-2".into(),
        root: "/opt/data/workbench/jobs/job-2".into(),
        branch: "tala-1-work-a2".into(),
        origin_url: "https://forgejo/fixture/rewind-fixture.git".into(),
        department: "operations".into(),
    }
}

fn input() -> Value {
    json!({
        "sourceRunId": "run-1", "seq": 7, "requestedBy": "u-1",
        "taskId": "t-1", "agentModel": "atlas", "boardId": "b-1",
    })
}

fn ctx(checkpoint: Value) -> RunStepContext {
    let (tx, signal) = StepSignal::channel();
    drop(tx);
    RunStepContext {
        activity: StepActivity::new(),
        run: RunRow {
            id: "rw-1".into(),
            kind: REWIND_KIND.into(),
            owner_user_id: Some("u-1".into()),
            subject_type: Some("task".into()),
            subject_id: Some("t-1".into()),
            state: RunState::Running,
            phase: "queued".into(),
            checkpoint: Value::Null,
            input: input(),
            result: Value::Null,
            error: None,
            attempt: 0,
            crashes: 0,
            lease_owner: None,
            lease_expires_at: None,
            approval_key: None,
            decision: None,
            created_at: "2026-11-04T10:00:00.000Z".into(),
            updated_at: "2026-11-04T10:00:00.000Z".into(),
            started_at: None,
            finished_at: None,
        },
        input: input(),
        checkpoint,
        decision: None,
        signal,
        log: Arc::new(|_| {}),
        attempt: 0,
    }
}

fn stage_of(result: &StepResult) -> Option<Stage> {
    match result {
        StepResult::Next { checkpoint, .. } => {
            serde_json::from_value::<RewindCheckpoint>(checkpoint.clone())
                .ok()
                .map(|c| c.stage)
        }
        _ => None,
    }
}

async fn drive_to_end(world: &Fake) -> StepResult {
    let mut checkpoint = Value::Null;
    loop {
        let result = rewind_step(ctx(checkpoint.clone()), world).await.unwrap();
        match result {
            StepResult::Next {
                checkpoint: next, ..
            } => checkpoint = next,
            other => return other,
        }
    }
}

#[tokio::test]
async fn the_stages_run_in_order_and_end_in_the_attempt() {
    let world = Fake::new();
    let result = drive_to_end(&world).await;
    assert_eq!(
        world.calls(),
        [
            "source", "reach", "reserve", "cancel", "quiet", "fork", "cut", "restore", "tail",
            "checkout", "start"
        ]
    );
    let StepResult::Done { result } = result else {
        panic!("{result:?}");
    };
    assert_eq!(result["attemptRunId"], "run-2");
    assert_eq!(result["attemptNo"], 2);
    assert_eq!(result["jobId"], "job-2");
    assert_eq!(result["turn"], 3);
    assert_eq!(world.failed_at(), None);
}

#[tokio::test]
async fn validate_fixes_the_plan_every_later_stage_reads() {
    let world = Fake::new();
    let result = rewind_step(ctx(Value::Null), &world).await.unwrap();
    let StepResult::Next { checkpoint, phase } = result else {
        panic!("{result:?}");
    };
    assert_eq!(phase.as_deref(), Some("stop"));
    let cp: RewindCheckpoint = serde_json::from_value(checkpoint).unwrap();
    assert_eq!(cp.stage, Stage::Stop);
    assert_eq!(cp.plan.attempt_run_id, "run-2");
    assert_eq!(cp.plan.tool_call_id, "call_7");
    assert_eq!(cp.plan.commit, "c0ffee");
    assert_eq!(cp.plan.tree, "tree-7");
    assert_eq!(cp.plan.turn, 3);
    assert_eq!(cp.plan.source_root, "/opt/data/workbench/jobs/job-1");
    assert_eq!(cp.plan.move_to, None);
}

#[tokio::test]
async fn a_crash_after_each_stage_resumes_at_that_stage() {
    let order = [
        (Stage::Stop, "cancel", Some(Stage::Quiet)),
        (Stage::Quiet, "quiet", Some(Stage::Fork)),
        (Stage::Fork, "fork", Some(Stage::Cut)),
        (Stage::Cut, "cut", Some(Stage::Restore)),
        (Stage::Restore, "restore", Some(Stage::Verify)),
        (Stage::Verify, "tail", Some(Stage::Start)),
        (Stage::Start, "start", None),
    ];
    let world = Fake::new();
    let plan: RewindPlan = match rewind_step(ctx(Value::Null), &world).await.unwrap() {
        StepResult::Next { checkpoint, .. } => {
            serde_json::from_value::<RewindCheckpoint>(checkpoint)
                .unwrap()
                .plan
        }
        other => panic!("{other:?}"),
    };
    for (stage, first_call, next_stage) in order {
        let world = Fake::new();
        let result = rewind_step(
            ctx(json!(RewindCheckpoint {
                stage,
                plan: plan.clone()
            })),
            &world,
        )
        .await
        .unwrap();
        assert_eq!(
            world.calls().first().map(String::as_str),
            Some(first_call),
            "{stage:?}"
        );
        assert!(
            !world
                .calls()
                .iter()
                .any(|c| c == "source" || c == "reserve"),
            "{stage:?} re-validated"
        );
        assert_eq!(stage_of(&result), next_stage, "{stage:?}");
        if next_stage.is_none() {
            assert!(matches!(result, StepResult::Done { .. }));
        }
    }
}

async fn refused(world: &Fake) -> String {
    let err = rewind_step(ctx(Value::Null), world).await.unwrap_err();
    assert_eq!(
        world.failed_at(),
        Some((None, false)),
        "fail runs with no plan"
    );
    assert!(
        !world.calls().iter().any(|c| c == "cancel"),
        "nothing destructive ran"
    );
    err
}

#[tokio::test]
async fn an_unrestorable_step_is_refused_before_anything_runs() {
    let mut src = source();
    src.row = Some(RunCheckpoint {
        restorable: false,
        unrestorable_reason: Some("the job's checkout was removed".into()),
        workspace_commit: None,
        ..step_row(7)
    });
    let world = Fake::new().with_source(Ok(src));
    let err = refused(&world).await;
    assert_eq!(
        err,
        "step 7 cannot be restored: the job's checkout was removed"
    );
    assert_eq!(world.calls(), ["source", "fail"]);
}

#[tokio::test]
async fn a_turn_row_and_a_missing_row_are_refused() {
    let mut src = source();
    src.row = Some(RunCheckpoint {
        kind: CheckpointKind::Turn,
        ..step_row(7)
    });
    let err = refused(&Fake::new().with_source(Ok(src))).await;
    assert!(err.contains("turn boundary"), "{err}");
    let mut src = source();
    src.row = None;
    let err = refused(&Fake::new().with_source(Ok(src))).await;
    assert_eq!(err, "run run-1 has no ledger row 7");
}

#[tokio::test]
async fn a_gone_job_is_refused() {
    let mut src = source();
    src.job = None;
    let err = refused(&Fake::new().with_source(Ok(src))).await;
    assert_eq!(err, "step 7's job job-1 is gone");
}

#[tokio::test]
async fn a_done_ticket_and_an_unworkable_column_are_refused_and_review_moves_back() {
    let mut src = source();
    src.ticket = Some(Ticket {
        status: "done".into(),
        ..ticket(TicketColumn::Done)
    });
    let err = refused(&Fake::new().with_source(Ok(src))).await;
    assert!(err.starts_with("the ticket is done"), "{err}");

    let mut src = source();
    src.ticket = Some(Ticket {
        status: "backlog".into(),
        ..ticket(TicketColumn::Other)
    });
    let err = refused(&Fake::new().with_source(Ok(src))).await;
    assert!(err.contains("\"backlog\""), "{err}");

    let mut src = source();
    src.ticket = Some(Ticket {
        status: "review".into(),
        ..ticket(TicketColumn::Review)
    });
    let validated = validate(&src).unwrap();
    assert_eq!(validated.move_to.as_deref(), Some("in_progress"));

    let mut src = source();
    src.ticket = Some(Ticket {
        status: "review".into(),
        active_key: None,
        ..ticket(TicketColumn::Review)
    });
    let err = refused(&Fake::new().with_source(Ok(src))).await;
    assert!(err.contains("no working column"), "{err}");
}

#[tokio::test]
async fn an_unreachable_agent_or_missing_commit_refuses_before_the_hold() {
    let mut world = Fake::new();
    world.reach = Err("snapshot commit c0ffee is not in /opt/data/workbench/jobs/job-1".into());
    let err = rewind_step(ctx(Value::Null), &world).await.unwrap_err();
    assert!(err.contains("c0ffee"), "{err}");
    assert_eq!(world.calls(), ["source", "reach", "fail"]);
}

#[tokio::test]
async fn no_free_slot_refuses() {
    let mut world = Fake::new();
    world.reserve = Err("another rewind is already in flight on this ticket and agent".into());
    let err = rewind_step(ctx(Value::Null), &world).await.unwrap_err();
    assert!(err.contains("already in flight"), "{err}");
    assert_eq!(world.failed_at(), Some((None, false)));
}

async fn at(stage: Stage) -> Value {
    let plan = serde_json::from_value::<RewindCheckpoint>(
        match rewind_step(ctx(Value::Null), &Fake::new()).await.unwrap() {
            StepResult::Next { checkpoint, .. } => checkpoint,
            other => panic!("{other:?}"),
        },
    )
    .unwrap()
    .plan;
    json!(RewindCheckpoint { stage, plan })
}

#[tokio::test]
async fn an_agent_that_stays_loud_fails_the_rewind_and_leaves_the_source_stopped() {
    let mut world = Fake::new();
    world.quiet = Quiet::Unconfirmed("still running".into());
    let err = rewind_step(ctx(at(Stage::Quiet).await), &world)
        .await
        .unwrap_err();
    assert!(
        err.contains("still running") && err.contains("stays stopped"),
        "{err}"
    );
    assert_eq!(world.failed_at(), Some((Some(Stage::Quiet), true)));
}

#[tokio::test]
async fn a_retried_fork_is_accepted_and_a_foreign_session_is_not() {
    let mut world = Fake::new();
    world.fork = Ok(Fork::Exists {
        parent_session_id: Some("run-1".into()),
    });
    let result = rewind_step(ctx(at(Stage::Fork).await), &world)
        .await
        .unwrap();
    assert_eq!(stage_of(&result), Some(Stage::Cut));

    let mut world = Fake::new();
    world.fork = Ok(Fork::Exists {
        parent_session_id: Some("some-chat".into()),
    });
    let err = rewind_step(ctx(at(Stage::Fork).await), &world)
        .await
        .unwrap_err();
    assert!(err.contains("not a fork of this run"), "{err}");
}

#[tokio::test]
async fn a_cut_that_lands_elsewhere_fails() {
    let mut world = Fake::new();
    world.cut = Ok(Cut {
        was: 12,
        now: 12,
        last_tool_call_id: Some("call_9".into()),
    });
    let err = rewind_step(ctx(at(Stage::Cut).await), &world)
        .await
        .unwrap_err();
    assert!(err.contains("call_9") && err.contains("call_7"), "{err}");
    assert_eq!(world.failed_at(), Some((Some(Stage::Cut), true)));
}

#[tokio::test]
async fn a_tree_mismatch_fails_at_restore_and_at_verify() {
    let mut world = Fake::new();
    world.restore = Ok("tree-other".into());
    let err = rewind_step(ctx(at(Stage::Restore).await), &world)
        .await
        .unwrap_err();
    assert!(
        err.contains("tree-other") && err.contains("tree-7"),
        "{err}"
    );

    let mut world = Fake::new();
    world.checkout = Ok(Checkout {
        tree: "tree-7".into(),
        branch: "tala-1-work-a2".into(),
        clean: false,
    });
    let err = rewind_step(ctx(at(Stage::Verify).await), &world)
        .await
        .unwrap_err();
    assert!(err.contains("uncommitted"), "{err}");

    let mut world = Fake::new();
    world.tail = Ok(Some("call_3".into()));
    let err = rewind_step(ctx(at(Stage::Verify).await), &world)
        .await
        .unwrap_err();
    assert!(err.contains("call_3"), "{err}");
    assert_eq!(world.calls(), ["tail", "fail"]);
}

#[tokio::test]
async fn a_start_that_cannot_land_fails_with_the_plan() {
    let mut world = Fake::new();
    world.start = Err("session run-2 was claimed while the rewind ran".into());
    let err = rewind_step(ctx(at(Stage::Start).await), &world)
        .await
        .unwrap_err();
    assert!(err.contains("claimed"), "{err}");
    assert_eq!(world.failed_at(), Some((Some(Stage::Start), true)));
}

#[tokio::test]
async fn the_attempt_input_carries_its_binding() {
    let plan: RewindPlan = serde_json::from_value::<RewindCheckpoint>(at(Stage::Stop).await)
        .unwrap()
        .plan;
    let input = attempt_input("run-1", &source().input, &plan);
    assert_eq!(input.generation, 1);
    let binding = input.rewind.unwrap();
    assert_eq!(binding.from_run_id, "run-1");
    assert_eq!(binding.turn, 3);
    assert_eq!(binding.attempt_no, 2);
    assert_eq!(binding.job_id, "job-2");
    assert_eq!(binding.branch, "tala-1-work-a2");
    assert_eq!(binding.workdir, "/opt/data/workbench/jobs/job-2");
}

#[test]
fn a_source_is_settled_only_when_terminal_and_past_the_window() {
    assert!(settled(RunState::Done, Some(1_000), 200_000, 120_000));
    assert!(!settled(RunState::Done, Some(100_000), 200_000, 120_000));
    assert!(!settled(RunState::Running, Some(1_000), 200_000, 120_000));
    assert!(!settled(RunState::Error, None, 200_000, 120_000));
}

#[tokio::test]
async fn the_checkpoint_spelling_is_pinned() {
    let cp = at(Stage::Cut).await;
    assert_eq!(cp["stage"], "cut");
    assert_eq!(cp["plan"]["attemptRunId"], "run-2");
    assert_eq!(cp["plan"]["toolCallId"], "call_7");
}

#[test]
fn the_tail_is_the_last_tool_result() {
    use crate::world::tail_tool_call;
    let rows = vec![
        json!({"role": "assistant", "tool_calls": [{"id": "call_7"}]}),
        json!({"role": "tool", "tool_call_id": "call_7"}),
    ];
    assert_eq!(tail_tool_call(&rows).as_deref(), Some("call_7"));
    let rows = vec![
        json!({"role": "tool", "tool_call_id": "call_7"}),
        json!({"role": "assistant"}),
    ];
    assert_eq!(tail_tool_call(&rows), None);
    assert_eq!(tail_tool_call(&[]), None);
}

#[test]
fn the_attempt_root_keeps_the_source_roots_place() {
    use crate::world::attempt_root;
    assert_eq!(
        attempt_root("/jobs/a", "/jobs/a/rewind-fixture", "/jobs/b"),
        "/jobs/b/rewind-fixture"
    );
    assert_eq!(attempt_root("/jobs/a", "/jobs/a", "/jobs/b"), "/jobs/b");
}
