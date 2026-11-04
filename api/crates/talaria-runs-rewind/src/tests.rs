// The stage machine against one stateful world: every stage in order, a crash
// right after each stage with the retry resuming from the persisted
// checkpoint against the same world, every refusal, and the retries that must
// touch nothing that happened since.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use talaria_runs_define::{RunRow, RunState, RunStepContext, StepActivity, StepResult, StepSignal};
use talaria_runs_store::{CheckpointKind, RunCheckpoint, RunLineage};
use talaria_runs_work_session::{AgentStop, WorkSessionInput};

use super::*;
use crate::world::{
    AttemptRow, Slot, column_move, contained, quiet_end, quiet_from, slot_of, tail_tool_call,
};
use talaria_fleet_docker::rewind::SessionLease;
use talaria_gateway::fleet_chat::SessionAt;

const SOURCE: &str = "run-1";
const ATTEMPT: &str = "run-2";
const CALL: &str = "call_7";
const ROOT: &str = "/opt/data/workbench/jobs/job-2";

struct Session {
    parent: Option<String>,
    rows: Vec<(String, Option<String>)>,
}

/// What the world holds between steps. Each method changes it the way the
/// real one changes Postgres and the container, so a retry sees what the
/// crashed step left.
#[derive(Default)]
struct State {
    calls: Vec<String>,
    ticket: Option<Ticket>,
    hold: Option<String>,
    sessions: HashMap<String, Session>,
    roots: HashSet<String>,
    attempts: Vec<AttemptRow>,
    supersede_calls: usize,
    driven: Vec<String>,
    moved: Vec<String>,
    discards: usize,
    quiet: Option<Quiet>,
    reach_err: Option<String>,
    reserve_err: Option<String>,
    attempt_read_fails: bool,
    cut_lands_at: Option<String>,
    restore_tree: String,
    checkout_clean: bool,
    start_err: Option<String>,
    fork_answer_lost: bool,
}

struct World(Mutex<State>);

fn source_rows() -> Vec<(String, Option<String>)> {
    let mut rows = vec![("user".to_string(), None)];
    for n in 1..=9 {
        rows.push(("assistant".into(), None));
        rows.push(("tool".into(), Some(format!("call_{n}"))));
    }
    rows.push(("assistant".into(), None));
    rows
}

impl World {
    fn new() -> Self {
        let mut sessions = HashMap::new();
        sessions.insert(
            SOURCE.to_string(),
            Session {
                parent: None,
                rows: source_rows(),
            },
        );
        World(Mutex::new(State {
            ticket: Some(ticket(TicketColumn::Working)),
            sessions,
            restore_tree: "tree-7".into(),
            checkout_clean: true,
            ..State::default()
        }))
    }

    fn with(self, f: impl FnOnce(&mut State)) -> Self {
        f(&mut self.0.lock().unwrap());
        self
    }

    fn set(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.0.lock().unwrap());
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap()
    }

    fn log(&self, call: &str) {
        self.state().calls.push(call.to_string());
    }

    fn calls(&self) -> Vec<String> {
        self.state().calls.clone()
    }
}

impl RewindWorld for World {
    fn source<'a>(&'a self, _input: &'a RewindInput) -> BoxFuture<'a, Result<Source, String>> {
        self.log("source");
        let ticket = self.state().ticket.clone();
        async move { Ok(Source { ticket, ..source() }) }.boxed()
    }
    fn ticket<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Option<Ticket>, String>> {
        self.log("ticket");
        let ticket = self.state().ticket.clone();
        async move { Ok(ticket) }.boxed()
    }
    fn reach<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<String, String>> {
        self.log("reach");
        let answer = match self.state().reach_err.clone() {
            Some(e) => Err(e),
            None => Ok("/opt/data/workbench/jobs/job-1".to_string()),
        };
        async move { answer }.boxed()
    }
    fn reserve<'a>(
        &'a self,
        rewind_id: &'a str,
        _: &'a RewindInput,
        _: &'a Source,
        _: &'a Validated,
        _: &'a str,
    ) -> BoxFuture<'a, Result<Reservation, String>> {
        self.log("reserve");
        let mut s = self.state();
        let answer = match (&s.reserve_err, &s.hold) {
            (Some(e), _) => Err(e.clone()),
            (None, Some(other)) if other != rewind_id => {
                Err("another rewind is already in flight on this ticket and agent".into())
            }
            _ => {
                s.hold = Some(rewind_id.to_string());
                Ok(reservation())
            }
        };
        async move { answer }.boxed()
    }
    fn cancel_source<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        self.log("cancel");
        async move { Ok(()) }.boxed()
    }
    fn quiet<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Quiet> {
        self.log("quiet");
        let answer = self.state().quiet.clone().unwrap_or(Quiet::Confirmed);
        async move { answer }.boxed()
    }
    fn fork<'a>(
        &'a self,
        _: &'a str,
        src: &'a str,
        attempt: &'a str,
    ) -> BoxFuture<'a, Result<Fork, String>> {
        self.log("fork");
        let mut s = self.state();
        let answer = match s.sessions.get(attempt) {
            Some(existing) => Ok(Fork::Exists {
                parent_session_id: existing.parent.clone(),
            }),
            None => {
                let rows = s
                    .sessions
                    .get(src)
                    .map(|x| x.rows.clone())
                    .unwrap_or_default();
                s.sessions.insert(
                    attempt.to_string(),
                    Session {
                        parent: Some(src.to_string()),
                        rows,
                    },
                );
                if s.fork_answer_lost {
                    Err("no answer within 30s".to_string())
                } else {
                    Ok(Fork::Created)
                }
            }
        };
        async move { answer }.boxed()
    }
    fn cut<'a>(
        &'a self,
        _: &'a str,
        session: &'a str,
        call: &'a str,
    ) -> BoxFuture<'a, Result<Cut, String>> {
        self.log("cut");
        let mut s = self.state();
        let target = s.cut_lands_at.clone().unwrap_or_else(|| call.to_string());
        let answer = match s.sessions.get_mut(session) {
            None => Err(format!("no session {session}")),
            Some(session) => {
                let was = session.rows.len() as u64;
                match session
                    .rows
                    .iter()
                    .position(|(role, id)| role == "tool" && id.as_deref() == Some(&target))
                {
                    None => Err(format!(
                        "the session has no result row for tool call {target}"
                    )),
                    Some(at) => {
                        session.rows.truncate(at + 1);
                        Ok(Cut {
                            was,
                            now: session.rows.len() as u64,
                            last_tool_call_id: session.rows.last().and_then(|(_, id)| id.clone()),
                        })
                    }
                }
            }
        };
        async move { answer }.boxed()
    }
    fn restore<'a>(
        &'a self,
        _: &'a str,
        plan: &'a RewindPlan,
    ) -> BoxFuture<'a, Result<String, String>> {
        self.log("restore");
        let mut s = self.state();
        s.roots.insert(plan.root.clone());
        let tree = s.restore_tree.clone();
        async move { Ok(tree) }.boxed()
    }
    fn session_tail<'a>(
        &'a self,
        _: &'a str,
        session: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        self.log("tail");
        let s = self.state();
        let answer = match s.sessions.get(session) {
            None => Err(format!("no session {session}")),
            Some(session) => Ok(session
                .rows
                .last()
                .and_then(|(role, id)| if role == "tool" { id.clone() } else { None })),
        };
        async move { answer }.boxed()
    }
    fn checkout<'a>(
        &'a self,
        _: &'a str,
        root: &'a str,
    ) -> BoxFuture<'a, Result<Checkout, String>> {
        self.log("checkout");
        let s = self.state();
        let answer = if s.roots.contains(root) {
            Ok(Checkout {
                tree: s.restore_tree.clone(),
                branch: "tala-1-work-a2".into(),
                clean: s.checkout_clean,
            })
        } else {
            Err(format!("no checkout at {root}"))
        };
        async move { answer }.boxed()
    }
    fn attempt<'a>(&'a self, run_id: &'a str) -> BoxFuture<'a, Result<Option<AttemptRow>, String>> {
        self.log("attempt");
        let s = self.state();
        let answer = if s.attempt_read_fails {
            Err("run read: the pool timed out".into())
        } else {
            Ok(s.attempts.iter().find(|a| a.run_id == run_id).cloned())
        };
        async move { answer }.boxed()
    }
    fn start<'a>(
        &'a self,
        rewind_id: &'a str,
        _: &'a RewindInput,
        plan: &'a RewindPlan,
        move_to: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.log("start");
        let mut s = self.state();
        let answer = match &s.start_err {
            Some(e) => Err(e.clone()),
            None => {
                assert_eq!(
                    s.hold.as_deref(),
                    Some(rewind_id),
                    "start runs under this rewind's hold"
                );
                if let Some(to) = move_to {
                    s.moved.push(to.to_string());
                }
                s.attempts.push(AttemptRow {
                    run_id: plan.attempt_run_id.clone(),
                    job_id: plan.job_id.clone(),
                });
                s.hold = None;
                s.supersede_calls += 1;
                s.driven.push(plan.attempt_run_id.clone());
                Ok(())
            }
        };
        async move { answer }.boxed()
    }
    fn lift_hold<'a>(&'a self, rewind_id: &'a str) -> BoxFuture<'a, ()> {
        self.log("lift_hold");
        let mut s = self.state();
        if s.hold.as_deref() == Some(rewind_id) {
            s.hold = None;
        }
        async move {}.boxed()
    }
    fn discard<'a>(
        &'a self,
        input: &'a RewindInput,
        plan: &'a RewindPlan,
        forked: bool,
    ) -> BoxFuture<'a, ()> {
        self.log("discard");
        let mut s = self.state();
        s.discards += 1;
        s.roots.remove(&plan.root);
        let ours = s
            .sessions
            .get(&plan.attempt_run_id)
            .is_some_and(|session| session.parent.as_deref() == Some(&input.source_run_id));
        if forked && ours {
            s.sessions.remove(&plan.attempt_run_id);
        }
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
        tool_call_id: Some(CALL.into()),
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
        status: match column {
            TicketColumn::Working => "in_progress",
            TicketColumn::Review => "review",
            TicketColumn::Done => "done",
            TicketColumn::Other => "blocked",
        }
        .into(),
        column,
        active_key: Some("in_progress".into()),
    }
}

fn source() -> Source {
    Source {
        run_id: SOURCE.into(),
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
            lineage_id: SOURCE.into(),
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
        attempt_run_id: ATTEMPT.into(),
        generation: 1,
        attempt_no: 2,
        job_id: "job-2".into(),
        workdir: ROOT.into(),
        root: ROOT.into(),
        branch: "tala-1-work-a2".into(),
        origin_url: "https://forgejo/fixture/rewind-fixture.git".into(),
        department: "operations".into(),
    }
}

fn input() -> Value {
    json!({
        "sourceRunId": SOURCE, "seq": 7, "requestedBy": "u-1",
        "taskId": "t-1", "agentModel": "atlas", "boardId": "b-1",
    })
}

fn ctx(checkpoint: Value) -> RunStepContext {
    ctx_of("rw-1", checkpoint)
}

fn ctx_of(id: &str, checkpoint: Value) -> RunStepContext {
    let (tx, signal) = StepSignal::channel();
    drop(tx);
    RunStepContext {
        activity: StepActivity::new(),
        run: RunRow {
            id: id.into(),
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

/// Drive the run from `checkpoint` to its end. A step number in `crash_after`
/// has its checkpoint dropped instead of persisted, so the next step re-runs
/// from the one before it, exactly as a driver that died before its write.
async fn drive(
    world: &World,
    mut checkpoint: Value,
    crash_after: Option<usize>,
) -> Result<StepResult, String> {
    let mut step = 0usize;
    loop {
        let result = rewind_step(ctx(checkpoint.clone()), world).await?;
        match result {
            StepResult::Next {
                checkpoint: next, ..
            } => {
                if crash_after != Some(step) {
                    checkpoint = next;
                }
            }
            other => return Ok(other),
        }
        step += 1;
        assert!(step < 40, "the run did not end");
    }
}

async fn drive_as(world: &World, id: &str, mut checkpoint: Value) {
    loop {
        match rewind_step(ctx_of(id, checkpoint.clone()), world)
            .await
            .unwrap()
        {
            StepResult::Next {
                checkpoint: next, ..
            } => checkpoint = next,
            _ => return,
        }
    }
}

fn assert_converged(world: &World, label: &str) {
    let s = world.state();
    assert_eq!(s.attempts.len(), 1, "{label}: one attempt");
    assert_eq!(s.hold, None, "{label}: no hold left");
    assert!(
        s.roots.contains(ROOT),
        "{label}: the attempt's checkout stands"
    );
    let fork = s.sessions.get(ATTEMPT).expect("the fork exists");
    assert_eq!(
        fork.rows.last().and_then(|(_, id)| id.clone()).as_deref(),
        Some(CALL),
        "{label}: the fork ends at the step"
    );
    assert_eq!(
        s.supersede_calls, 1,
        "{label}: the source's jobs were retired once"
    );
    assert_eq!(
        s.driven,
        vec![ATTEMPT.to_string()],
        "{label}: the attempt was driven once"
    );
    assert_eq!(s.discards, 0, "{label}: nothing was discarded");
}

#[tokio::test]
async fn the_stages_run_in_order_and_end_in_the_attempt() {
    let world = World::new();
    let result = drive(&world, Value::Null, None).await.unwrap();
    assert_eq!(
        world.calls(),
        [
            "source", "reach", "reserve", "cancel", "quiet", "fork", "cut", "restore", "tail",
            "checkout", "attempt", "ticket", "start"
        ]
    );
    let StepResult::Done { result } = result else {
        panic!("{result:?}");
    };
    assert_eq!(result["attemptRunId"], ATTEMPT);
    assert_eq!(result["attemptNo"], 2);
    assert_eq!(result["jobId"], "job-2");
    assert_eq!(result["turn"], 3);
    assert_converged(&world, "plain run");
}

#[tokio::test]
async fn a_crash_right_after_each_stage_converges_on_the_same_end_state() {
    for crash_after in 0..8 {
        let world = World::new();
        let result = drive(&world, Value::Null, Some(crash_after)).await.unwrap();
        assert!(
            matches!(result, StepResult::Done { .. }),
            "crash after step {crash_after}: {result:?}"
        );
        assert_converged(&world, &format!("crash after step {crash_after}"));
    }
}

#[tokio::test]
async fn a_retried_start_leaves_a_newer_attempt_alone() {
    let world = World::new();
    let start_cp = {
        let mut cp = Value::Null;
        loop {
            let result = rewind_step(ctx(cp.clone()), &world).await.unwrap();
            match result {
                StepResult::Next { checkpoint, .. } => {
                    cp = checkpoint;
                    if stage_of(&StepResult::Next {
                        checkpoint: cp.clone(),
                        phase: None,
                    }) == Some(Stage::Start)
                    {
                        break cp;
                    }
                }
                other => panic!("{other:?}"),
            }
        }
    };
    let result = rewind_step(ctx(start_cp.clone()), &world).await.unwrap();
    assert!(matches!(result, StepResult::Done { .. }));
    world.set(|s| {
        s.attempts.push(AttemptRow {
            run_id: "run-3".into(),
            job_id: "job-3".into(),
        });
    });
    let result = rewind_step(ctx(start_cp), &world).await.unwrap();
    assert!(matches!(result, StepResult::Done { .. }), "{result:?}");
    let s = world.state();
    assert_eq!(
        s.supersede_calls, 1,
        "a retry past its own commit retires nothing"
    );
    assert_eq!(s.driven.len(), 1, "a retry drives nothing");
    assert_eq!(s.moved.len(), 0, "a retry moves nothing");
    assert_eq!(s.attempts.len(), 2, "attempt 3 is untouched");
    assert_eq!(s.discards, 0);
}

#[tokio::test]
async fn a_retried_start_that_cannot_read_its_attempt_destroys_nothing() {
    let world = World::new();
    let result = drive(&world, Value::Null, None).await.unwrap();
    assert!(matches!(result, StepResult::Done { .. }));
    let start_cp = at(Stage::Start).await;
    world.set(|s| s.attempt_read_fails = true);
    let err = rewind_step(ctx(start_cp), &world).await.unwrap_err();
    assert!(err.contains("run read"), "{err}");
    let s = world.state();
    assert_eq!(
        s.discards, 0,
        "a failure that cannot tell whether the attempt committed discards nothing"
    );
    assert!(s.roots.contains(ROOT));
    assert!(s.sessions.contains_key(ATTEMPT), "the fork stands");
    assert_eq!(s.hold, None);
}

#[tokio::test]
async fn a_ticket_closed_or_blocked_during_the_rewind_fails_start() {
    for (column, word) in [
        (TicketColumn::Done, "done"),
        (TicketColumn::Other, "blocked"),
    ] {
        let world = World::new().with(|s| s.ticket = Some(ticket(TicketColumn::Review)));
        let start_cp = at_in(&world, Stage::Start).await;
        world.set(|s| s.ticket = Some(ticket(column.clone())));
        let err = rewind_step(ctx(start_cp), &world).await.unwrap_err();
        assert!(err.contains(word), "{column:?}: {err}");
        let s = world.state();
        assert!(s.attempts.is_empty(), "{column:?}: no attempt");
        assert!(s.moved.is_empty(), "{column:?}: the person's move stands");
        assert_eq!(s.hold, None, "{column:?}: the hold is lifted");
        assert_eq!(
            s.discards, 1,
            "{column:?}: the prepared checkout and fork are taken back"
        );
    }
}

#[tokio::test]
async fn a_ticket_still_in_review_at_start_moves_back_as_the_person() {
    let world = World::new().with(|s| s.ticket = Some(ticket(TicketColumn::Review)));
    let result = drive(&world, Value::Null, None).await.unwrap();
    assert!(matches!(result, StepResult::Done { .. }));
    assert_eq!(world.state().moved, vec!["in_progress".to_string()]);
    let world = World::new().with(|s| s.ticket = Some(ticket(TicketColumn::Review)));
    let start_cp = at_in(&world, Stage::Start).await;
    world.set(|s| s.ticket = Some(ticket(TicketColumn::Working)));
    rewind_step(ctx(start_cp), &world).await.unwrap();
    assert!(
        world.state().moved.is_empty(),
        "a ticket a person already moved is not moved again"
    );
}

#[tokio::test]
async fn a_failure_before_start_takes_back_what_the_rewind_placed() {
    let world = World::new().with(|s| s.cut_lands_at = Some("call_9".into()));
    let err = drive(&world, Value::Null, None).await.unwrap_err();
    assert!(err.contains("call_9") && err.contains(CALL), "{err}");
    let s = world.state();
    assert_eq!(s.discards, 1);
    assert_eq!(s.hold, None);
    assert!(s.attempts.is_empty());
    assert!(!s.sessions.contains_key(ATTEMPT), "no session is left");
}

#[tokio::test]
async fn a_fork_whose_answer_was_lost_is_taken_back_too() {
    let world = World::new().with(|s| s.fork_answer_lost = true);
    let err = drive(&world, Value::Null, None).await.unwrap_err();
    assert!(err.contains("no answer"), "{err}");
    assert!(
        !world.state().sessions.contains_key(ATTEMPT),
        "the fork Hermes made after the answer was lost is gone"
    );
}

#[tokio::test]
async fn a_session_that_is_not_this_rewinds_fork_survives_its_failure() {
    let world = World::new().with(|s| {
        s.sessions.insert(
            ATTEMPT.to_string(),
            Session {
                parent: Some("someone-else".into()),
                rows: Vec::new(),
            },
        );
    });
    let err = drive(&world, Value::Null, None).await.unwrap_err();
    assert!(err.contains("not a fork of this run"), "{err}");
    assert!(world.state().sessions.contains_key(ATTEMPT));
}

#[test]
fn a_slot_a_failed_rewind_left_is_taken_back_and_nothing_else_is() {
    let pair = vec!["gen-0".to_string(), "gen-1".to_string()];
    assert_eq!(slot_of(&SessionAt::Absent, &pair), Slot::Free);
    assert_eq!(
        slot_of(
            &SessionAt::Held {
                parent: Some("gen-0".into())
            },
            &pair
        ),
        Slot::Leftover {
            parent: "gen-0".into()
        }
    );
    assert_eq!(
        slot_of(&SessionAt::Held { parent: None }, &pair),
        Slot::Spent,
        "a session of its own is a conversation, not a leftover fork"
    );
    assert_eq!(
        slot_of(
            &SessionAt::Held {
                parent: Some("another-ticket".into())
            },
            &pair
        ),
        Slot::Spent
    );
}

#[tokio::test]
async fn a_lost_turn_says_what_happened_and_when_to_rewind_again() {
    let world = World::new().with(|s| {
        s.quiet = Some(Quiet::TurnLost {
            lease_ends_in_secs: 95,
        })
    });
    let err = drive(&world, Value::Null, None).await.unwrap_err();
    assert!(err.contains("lost"), "names the lost turn: {err}");
    assert!(err.contains("restarted"), "says what lost it: {err}");
    assert!(err.contains("95 seconds"), "says when to retry: {err}");
    assert!(err.contains("stays stopped"), "{err}");
    assert!(!err.contains("once its turn has ended"), "{err}");
}

#[test]
fn a_run_the_agent_forgot_with_its_lease_still_held_is_a_lost_turn() {
    let held = Some(SessionLease::Held { ends_in_secs: 95 });
    assert_eq!(
        quiet_end("404 run_not_found".into(), held),
        Quiet::TurnLost {
            lease_ends_in_secs: 95
        }
    );
    assert_eq!(
        quiet_end("still running".into(), held),
        Quiet::Unconfirmed("still running; the agent still holds a turn on the session".into())
    );
    assert_eq!(
        quiet_end("404 run_not_found".into(), None),
        Quiet::Unconfirmed("404 run_not_found".into()),
        "an unread lease proves nothing"
    );
}

#[tokio::test]
async fn two_rewinds_past_the_routes_check_get_one_attempt_and_the_loser_touches_nothing() {
    let world = World::new();
    let (first, second) = tokio::join!(
        rewind_step(ctx_of("rewind-a", Value::Null), &world),
        rewind_step(ctx_of("rewind-b", Value::Null), &world)
    );
    let loser = second.unwrap_err();
    assert!(loser.contains("already in flight"), "{loser}");
    assert_eq!(
        world.state().hold.as_deref(),
        Some("rewind-a"),
        "the loser's failure lifts no hold but its own"
    );
    assert_eq!(world.state().discards, 0);
    let StepResult::Next { checkpoint, .. } = first.unwrap() else {
        panic!("the winner goes on");
    };
    drive_as(&world, "rewind-a", checkpoint).await;
    assert_eq!(world.state().attempts.len(), 1);
}

async fn at_in(world: &World, stage: Stage) -> Value {
    let mut cp = Value::Null;
    loop {
        let result = rewind_step(ctx(cp.clone()), world).await.unwrap();
        let StepResult::Next { checkpoint, .. } = result else {
            panic!("{result:?}");
        };
        cp = checkpoint;
        if serde_json::from_value::<RewindCheckpoint>(cp.clone())
            .unwrap()
            .stage
            == stage
        {
            return cp;
        }
    }
}

async fn at(stage: Stage) -> Value {
    at_in(&World::new(), stage).await
}

async fn refused(world: &World) -> String {
    let err = rewind_step(ctx(Value::Null), world).await.unwrap_err();
    let s = world.state();
    assert!(
        !s.calls.iter().any(|c| c == "cancel"),
        "nothing destructive ran"
    );
    assert_eq!(s.discards, 0, "nothing was placed, nothing is discarded");
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
    assert_eq!(
        validate(&src).unwrap_err(),
        "step 7 cannot be restored: the job's checkout was removed"
    );
    let mut src = source();
    src.row = Some(RunCheckpoint {
        kind: CheckpointKind::Turn,
        ..step_row(7)
    });
    assert!(validate(&src).unwrap_err().contains("turn boundary"));
    let mut src = source();
    src.row = None;
    assert_eq!(validate(&src).unwrap_err(), "run run-1 has no ledger row 7");
    let mut src = source();
    src.job = None;
    assert_eq!(validate(&src).unwrap_err(), "step 7's job job-1 is gone");
}

#[tokio::test]
async fn a_done_ticket_and_an_unworkable_column_are_refused_and_review_moves_back() {
    let world = World::new().with(|s| s.ticket = Some(ticket(TicketColumn::Done)));
    let err = refused(&world).await;
    assert!(err.starts_with("the ticket is done"), "{err}");
    assert_eq!(world.calls(), ["source", "lift_hold"]);

    let world = World::new().with(|s| s.ticket = Some(ticket(TicketColumn::Other)));
    let err = refused(&world).await;
    assert!(err.contains("\"blocked\""), "{err}");

    let mut src = source();
    src.ticket = Some(ticket(TicketColumn::Review));
    assert_eq!(
        validate(&src).unwrap().move_to.as_deref(),
        Some("in_progress")
    );

    let mut src = source();
    src.ticket = Some(Ticket {
        active_key: None,
        ..ticket(TicketColumn::Review)
    });
    assert!(validate(&src).unwrap_err().contains("no working column"));
}

#[tokio::test]
async fn an_unreachable_agent_or_missing_commit_refuses_before_the_hold() {
    let world = World::new().with(|s| {
        s.reach_err = Some("snapshot commit c0ffee is not in /opt/data/workbench/jobs/job-1".into())
    });
    let err = refused(&world).await;
    assert!(err.contains("c0ffee"), "{err}");
    assert_eq!(world.calls(), ["source", "reach", "lift_hold"]);
    assert_eq!(world.state().hold, None);
}

#[tokio::test]
async fn no_free_slot_refuses() {
    let world = World::new().with(|s| {
        s.reserve_err = Some("another rewind is already in flight on this ticket and agent".into())
    });
    let err = refused(&world).await;
    assert!(err.contains("already in flight"), "{err}");
}

#[tokio::test]
async fn an_agent_that_stays_loud_fails_the_rewind_and_leaves_the_source_stopped() {
    let world = World::new().with(|s| s.quiet = Some(Quiet::Unconfirmed("still running".into())));
    let err = drive(&world, Value::Null, None).await.unwrap_err();
    assert!(
        err.contains("still running") && err.contains("stays stopped"),
        "{err}"
    );
    let s = world.state();
    assert_eq!(s.hold, None);
    assert!(!s.sessions.contains_key(ATTEMPT), "no fork was made");
}

#[tokio::test]
async fn a_retried_fork_is_accepted_and_a_foreign_session_is_not() {
    let world = World::new();
    let result = drive(&world, Value::Null, Some(2)).await.unwrap();
    assert!(matches!(result, StepResult::Done { .. }));
    assert_converged(&world, "fork retried");

    let world = World::new().with(|s| {
        s.sessions.insert(
            ATTEMPT.into(),
            Session {
                parent: Some("some-chat".into()),
                rows: vec![],
            },
        );
    });
    let err = drive(&world, Value::Null, None).await.unwrap_err();
    assert!(err.contains("not a fork of this run"), "{err}");
}

#[tokio::test]
async fn a_tree_mismatch_fails_at_restore_and_at_verify() {
    let world = World::new().with(|s| s.restore_tree = "tree-other".into());
    let err = drive(&world, Value::Null, None).await.unwrap_err();
    assert!(
        err.contains("tree-other") && err.contains("tree-7"),
        "{err}"
    );

    let world = World::new();
    let verify_cp = at_in(&world, Stage::Verify).await;
    world.set(|s| s.checkout_clean = false);
    let err = rewind_step(ctx(verify_cp), &world).await.unwrap_err();
    assert!(err.contains("uncommitted"), "{err}");
}

#[tokio::test]
async fn a_start_that_cannot_land_fails_with_the_plan() {
    let world = World::new()
        .with(|s| s.start_err = Some("session run-2 was claimed while the rewind ran".into()));
    let err = drive(&world, Value::Null, None).await.unwrap_err();
    assert!(err.contains("claimed"), "{err}");
    assert_eq!(world.state().discards, 1);
}

#[tokio::test]
async fn the_attempt_input_carries_its_binding() {
    let plan: RewindPlan = serde_json::from_value::<RewindCheckpoint>(at(Stage::Stop).await)
        .unwrap()
        .plan;
    let input = attempt_input(SOURCE, &source().input, &plan);
    assert_eq!(input.generation, 1);
    let binding = input.rewind.unwrap();
    assert_eq!(binding.from_run_id, SOURCE);
    assert_eq!(binding.turn, 3);
    assert_eq!(binding.attempt_no, 2);
    assert_eq!(binding.job_id, "job-2");
    assert_eq!(binding.branch, "tala-1-work-a2");
    assert_eq!(binding.workdir, ROOT);
}

#[test]
fn a_terminal_row_alone_never_confirms_quiet() {
    let loud = AgentStop::Unconfirmed("still running".into());
    assert_eq!(
        quiet_from(&loud, Some(false)),
        None,
        "a busy session keeps polling"
    );
    assert_eq!(
        quiet_from(&loud, None),
        None,
        "an unanswered idle check keeps polling"
    );
    assert_eq!(
        quiet_from(&loud, Some(true)),
        Some(Quiet::Confirmed),
        "an idle session is the proof"
    );
    assert_eq!(
        quiet_from(&AgentStop::Ended("cancelled".into()), Some(false)),
        Some(Quiet::Confirmed)
    );
    assert_eq!(quiet_from(&AgentStop::NoTurn, None), Some(Quiet::Confirmed));
}

#[test]
fn a_root_outside_the_workdir_is_refused() {
    let w = "/opt/data/workbench/jobs/job-1";
    assert!(contained(w, w));
    assert!(contained(w, "/opt/data/workbench/jobs/job-1/repo"));
    assert!(!contained(w, "/opt/data/workbench/jobs/job-10"));
    assert!(!contained(w, "/opt/data/workbench/jobs/job-1/../job-2"));
    assert!(!contained(w, "/etc"));
    assert!(!contained(w, "/opt/data/workbench/jobs/job-1/x\ny"));
    assert!(!contained(w, ""));
}

#[test]
fn the_column_rule_is_one_function() {
    assert_eq!(column_move(&ticket(TicketColumn::Working)).unwrap(), None);
    assert_eq!(
        column_move(&ticket(TicketColumn::Review))
            .unwrap()
            .as_deref(),
        Some("in_progress")
    );
    assert!(
        column_move(&ticket(TicketColumn::Done))
            .unwrap_err()
            .contains("done")
    );
    assert!(
        column_move(&ticket(TicketColumn::Other))
            .unwrap_err()
            .contains("blocked")
    );
}

#[tokio::test]
async fn the_checkpoint_spelling_is_pinned() {
    let cp = at(Stage::Cut).await;
    assert_eq!(cp["stage"], "cut");
    assert_eq!(cp["plan"]["attemptRunId"], ATTEMPT);
    assert_eq!(cp["plan"]["toolCallId"], CALL);
}

#[test]
fn the_tail_is_the_last_tool_result() {
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
