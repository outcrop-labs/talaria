// The sweeper is exercised against an in-memory due query, an in-memory
// definition registry and a fake driver, so nothing here touches Postgres or
// Redis. Every edge is a field on `ReclaimDeps` — same pattern and same reason
// as runs/run.test.ts and runs/decide.test.ts. The port of
// ui/src/server/runs/reclaim.test.ts.
//
// WHAT THESE TESTS ARE ACTUALLY FOR. The sweeper is the piece that makes
// "survives a restart" true, and its two failure modes are opposites: it can
// fail to wake a run whose driver died (durability silently does not happen),
// or it can wake — or worse, FAIL — a run that is perfectly healthy. The second
// is the one server/research.ts:352 ships today, and the `awaiting` case is the
// sharpest version of it: a run parked on a person may sit for days, and a
// sweeper that read "old and not moving" as "broken" would auto-fail every
// paused run in the product.
//
// TWO DELIBERATE DIVERGENCES from the TS file, both structural:
//   · TS mocks the scheduler so importing the module does not register a real
//     job, then asserts RECLAIM_JOB_SPEC's numbers. Rust has no module-load
//     registration and no JobSpec yet (the scheduler crosses later in this
//     batch), so the numbers are pinned as the constants the spec will be
//     built from.
//   · The give-up log line is asserted by the unit tests inside reclaim.rs,
//     where the sentence-building is a pure function — TS spies on console
//     instead, and there is no subscriber installed here to spy on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use talaria_api::agent_auth::epoch_ms_to_iso;
use talaria_api::runs::define::{Authority, RunDefinition, RunRow, RunState, StepResult};
use talaria_api::runs::reclaim::{
    DriveFn, DueFn, RECLAIM_EVERY_MS, RECLAIM_FIRST_RUN_DELAY_MS, RECLAIM_JOB, RECLAIM_LIMIT,
    RECLAIM_MAX_RUN_MS, ReclaimDeps, ReclaimSweep, describe_sweep, run_reclaim_job,
    sweep_reclaimable_runs,
};
use talaria_api::runs::run::{DefinitionForFn, DriveResult, DriveStop};

const NOW: i64 = 1_700_000_000_000;

// ── The fake world ───────────────────────────────────────────────────────────

/// A definition that never pauses — the sweeper only reads `max_attempts` off
/// it, and the fake driver stands in for the actual stepping anyway.
fn def(kind: &str, max_attempts: u32) -> Arc<RunDefinition> {
    Arc::new(RunDefinition {
        kind: kind.into(),
        label: format!("{kind} kind"),
        step: Arc::new(|_| {
            Box::pin(async {
                Ok(StepResult::Done {
                    result: Value::Null,
                })
            })
        }),
        audience: Arc::new(|_| Authority::Admin { on_board: None }),
        max_step_ms: 30_000,
        max_attempts,
    })
}

fn default_defs() -> HashMap<String, Arc<RunDefinition>> {
    let mut m = HashMap::new();
    m.insert("test-kind".into(), def("test-kind", 3));
    m
}

/// A run row as the due query would hand it back. Defaults describe the
/// ordinary reclaim case: `running`, lease expired a minute ago, first entry.
fn row(id: &str) -> RunRow {
    RunRow {
        id: id.into(),
        kind: "test-kind".into(),
        owner_user_id: Some("user-1".into()),
        subject_type: None,
        subject_id: None,
        state: RunState::Running,
        phase: "reading the sources".into(),
        checkpoint: json!({ "page": 3 }),
        input: Value::Null,
        result: Value::Null,
        error: None,
        attempt: 0,
        lease_owner: Some("dead-driver-token".into()),
        lease_expires_at: Some(epoch_ms_to_iso(NOW - 60_000)),
        approval_key: None,
        decision: None,
        created_at: epoch_ms_to_iso(NOW - 600_000),
        updated_at: epoch_ms_to_iso(NOW - 120_000),
        started_at: Some(epoch_ms_to_iso(NOW - 600_000)),
        finished_at: None,
    }
}

/// The fake driver's canned result. TS's `stop(...)` helper.
fn dr(run_id: &str, stop: DriveStop, state: Option<RunState>, error: Option<&str>) -> DriveResult {
    DriveResult {
        run_id: run_id.into(),
        stop,
        steps: 0,
        state,
        error: error.map(str::to_string),
        retry_after_ms: None,
    }
}

/// What the harness may be told: a driver with opinions, and a registry other
/// than the default one.
#[derive(Default)]
struct Opts {
    drive: Option<DriveFn>,
    defs: Option<HashMap<String, Arc<RunDefinition>>>,
}

impl Opts {
    fn with_drive(drive: DriveFn) -> Self {
        Self {
            drive: Some(drive),
            defs: None,
        }
    }
}

struct World {
    deps: ReclaimDeps,
    /// Run ids handed to the driver, in order.
    driven: Arc<Mutex<Vec<String>>>,
    /// The limit the sweep asked the store for.
    asked_limit: Arc<Mutex<Option<i64>>>,
}

impl World {
    fn driven(&self) -> Vec<String> {
        self.driven.lock().unwrap().clone()
    }

    fn asked_limit(&self) -> Option<i64> {
        *self.asked_limit.lock().unwrap()
    }
}

fn harness(rows: Vec<RunRow>, opts: Opts) -> World {
    let driven: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let asked_limit: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
    let rows = Arc::new(rows);
    let defs = Arc::new(opts.defs.unwrap_or_else(default_defs));
    let drive_override = opts.drive;

    let due: DueFn = {
        let rows = rows.clone();
        let asked_limit = asked_limit.clone();
        Arc::new(move |limit| {
            let rows = rows.clone();
            let asked_limit = asked_limit.clone();
            Box::pin(async move {
                *asked_limit.lock().unwrap() = Some(limit);
                Ok(rows.iter().take(limit.max(0) as usize).cloned().collect())
            })
        })
    };
    let definition_for: DefinitionForFn = {
        let defs = defs.clone();
        Arc::new(move |kind| defs.get(kind).cloned())
    };
    let drive: DriveFn = {
        let driven = driven.clone();
        Arc::new(move |run_id| {
            let driven = driven.clone();
            let over = drive_override.clone();
            Box::pin(async move {
                driven.lock().unwrap().push(run_id.clone());
                match over {
                    Some(f) => f(run_id).await,
                    None => Ok(dr(&run_id, DriveStop::Done, Some(RunState::Done), None)),
                }
            })
        })
    };

    World {
        deps: ReclaimDeps {
            due,
            definition_for,
            drive,
            now: Arc::new(|| NOW),
        },
        driven,
        asked_limit,
    }
}

/// Detached drives are spawned, so an assertion about them has to come after
/// the runtime has polled the tasks. Nothing in the fake driver awaits, so a
/// few yields are enough — the current-thread test runtime only polls spawned
/// work at a yield point.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

/// A run whose driver says the attempts are spent — the canned result behind
/// every give-up test.
fn exhausted_drive() -> DriveFn {
    Arc::new(|run_id| {
        Box::pin(async move {
            Ok(dr(
                &run_id,
                DriveStop::Exhausted,
                Some(RunState::Error),
                Some(
                    "run gave up after 3 attempt(s): each driver that took it stopped without \
                     finishing or checkpointing",
                ),
            ))
        })
    })
}

// ── The reclaim itself ───────────────────────────────────────────────────────

#[tokio::test]
async fn re_queues_a_running_run_whose_lease_has_expired() {
    let h = harness(vec![row("run-1")], Opts::default());
    let r = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    settle().await;

    assert_eq!(r.scanned, 1);
    assert_eq!(r.driven, 1);
    // The number that says a PROCESS died, as opposed to a queued run nobody
    // had picked up yet. They are different operational facts.
    assert_eq!(r.reclaimed, 1);
    assert_eq!(r.given_up, 0);
    assert_eq!(h.driven(), vec!["run-1"]);
    // The lease had been expired for a minute; that is the queue-depth number.
    assert_eq!(r.stalest_ms, 60_000);
}

#[tokio::test]
async fn hands_over_a_queued_run_with_no_lease_and_does_not_call_it_a_reclaim() {
    // A run that was enqueued and never claimed — the process that inserted it
    // died before its detached drive got going, or never had one.
    let mut r = row("run-1");
    r.state = RunState::Queued;
    r.lease_owner = None;
    r.lease_expires_at = None;
    let h = harness(vec![r], Opts::default());
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    settle().await;

    assert_eq!(out.driven, 1);
    assert_eq!(out.reclaimed, 0);
    assert_eq!(h.driven(), vec!["run-1"]);
}

#[tokio::test]
async fn leaves_a_run_whose_lease_is_still_live_alone() {
    // The store's query already excludes these. The sweeper says it a second
    // time because it is the one thing in the system that can wake a run from
    // the outside, and two drivers in one run is how a side effect happens
    // twice.
    let mut r = row("run-1");
    r.lease_expires_at = Some(epoch_ms_to_iso(NOW + 30_000));
    let h = harness(vec![r], Opts::default());
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    settle().await;

    assert_eq!(out.live, 1);
    assert_eq!(out.driven, 0);
    assert_eq!(h.driven(), Vec::<String>::new());
}

#[tokio::test]
async fn leaves_a_run_for_another_instance_when_this_one_has_no_definition_for_the_kind() {
    // A row from a newer deploy, or a module not in this process's graph. NOT
    // an error on the row: failing it here would destroy work on the strength
    // of a local import graph.
    let mut r = row("run-1");
    r.kind = "kind-from-the-future".into();
    let h = harness(vec![r], Opts::default());
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    settle().await;

    assert_eq!(out.unknown_kinds, 1);
    assert_eq!(out.driven, 0);
    assert_eq!(h.driven(), Vec::<String>::new());
}

// ── The guard the whole file exists for ──────────────────────────────────────

#[tokio::test]
async fn never_drives_and_never_fails_a_run_parked_on_a_person_however_long_it_has_sat() {
    // Everything about this row screams "stale" to a sweeper that measures
    // staleness in wall-clock time: parked a week ago, no lease, and it has
    // already spent every attempt it had. It is still perfectly healthy — it
    // is waiting for somebody to answer a question — and this is the exact row
    // server/research.ts's sweep would mark
    // `error: 'run went stale (app restarted mid-research?)'`.
    let mut r = row("run-1");
    r.state = RunState::Awaiting;
    r.attempt = 99;
    r.lease_owner = None;
    r.lease_expires_at = None;
    r.approval_key = Some("run:test-kind:run-1:pick-a-branch".into());
    r.updated_at = epoch_ms_to_iso(NOW - 7 * 24 * 60 * 60_000);
    let h = harness(vec![r], Opts::default());
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    settle().await;

    assert_eq!(out.not_drivable, 1);
    assert_eq!(out.driven, 0);
    assert_eq!(out.given_up, 0);
    assert_eq!(out.reclaimed, 0);
    assert_eq!(h.driven(), Vec::<String>::new());
}

#[tokio::test]
async fn leaves_terminal_runs_alone_too() {
    let mut a = row("a");
    a.state = RunState::Done;
    let mut b = row("b");
    b.state = RunState::Error;
    let mut c = row("c");
    c.state = RunState::Cancelled;
    let h = harness(vec![a, b, c], Opts::default());
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    settle().await;

    assert_eq!(out.not_drivable, 3);
    assert_eq!(h.driven(), Vec::<String>::new());
}

// ── Giving up ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn awaits_the_hand_over_and_reports_the_give_up_with_what_actually_happened() {
    // attempt 2, and the claim will make it 3 — the default maximum. The
    // driver files the error; the sweep watches so the pass can SAY so. (The
    // sentence itself — the count, the driver's diagnosis, the phase it died
    // at, and never "went stale" — is pinned by the unit tests in reclaim.rs,
    // where it is built by a pure function.)
    let mut r = row("run-1");
    r.attempt = 2;
    r.phase = "summarizing 40 sources".into();
    let h = harness(vec![r], Opts::with_drive(exhausted_drive()));
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();

    assert_eq!(out.given_up, 1);
    assert_eq!(out.driven, 0);
    assert_eq!(h.driven(), vec!["run-1"]);
}

#[tokio::test]
async fn still_resumes_a_run_one_attempt_short_of_the_line() {
    // The boundary, pinned from the other side: attempt 1 becomes 2 on the
    // claim, which is under the default of 3, so this run gets another go.
    let mut r = row("run-1");
    r.attempt = 1;
    let h = harness(vec![r], Opts::default());
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    settle().await;

    assert_eq!(out.driven, 1);
    assert_eq!(out.given_up, 0);
}

#[tokio::test]
async fn counts_a_queued_run_without_adding_the_reclaim_increment() {
    // `store.claim` bumps `attempt` only when the previous state was `running`.
    // A queued run at attempt 2 of 3 therefore has one entry left, and must
    // not be treated as spent.
    let mut r = row("run-1");
    r.state = RunState::Queued;
    r.attempt = 2;
    r.lease_owner = None;
    r.lease_expires_at = None;
    let h = harness(vec![r], Opts::default());
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    settle().await;

    assert_eq!(out.driven, 1);
    assert_eq!(out.given_up, 0);
}

#[tokio::test]
async fn honors_a_definitions_own_max_attempts() {
    let once = def("no-second-chances", 1);
    let mut m = HashMap::new();
    m.insert("no-second-chances".into(), once);
    let mut r = row("run-1");
    r.kind = "no-second-chances".into();
    let h = harness(
        vec![r],
        Opts {
            drive: Some(exhausted_drive()),
            defs: Some(m),
        },
    );
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();

    assert_eq!(out.given_up, 1);
    assert_eq!(out.driven, 0);
}

#[tokio::test]
async fn counts_a_hand_over_the_driver_did_not_treat_as_exhausted_as_an_ordinary_hand_over() {
    // The give-up is the DRIVER's decision; the sweep only predicts it to
    // decide whether to await. A prediction that turns out wrong — another
    // instance claimed the run first — must not be reported as a give-up.
    let mut r = row("run-1");
    r.attempt = 2;
    let h = harness(
        vec![r],
        Opts::with_drive(Arc::new(|run_id| {
            Box::pin(async move { Ok(dr(&run_id, DriveStop::Busy, Some(RunState::Running), None)) })
        })),
    );
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();

    assert_eq!(out.given_up, 0);
    assert_eq!(out.driven, 1);
    assert_eq!(out.reclaimed, 1);
}

// ── The bound ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn asks_the_store_for_at_most_reclaim_limit_runs_by_default() {
    let h = harness(Vec::new(), Opts::default());
    sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    assert_eq!(h.asked_limit(), Some(RECLAIM_LIMIT));
}

#[tokio::test]
async fn never_starts_more_drives_than_the_bound_however_many_runs_are_due() {
    let many: Vec<RunRow> = (0..100).map(|i| row(&format!("run-{i}"))).collect();
    let h = harness(many, Opts::default());
    let out = sweep_reclaimable_runs(Some(3), &h.deps).await.unwrap();
    settle().await;

    assert_eq!(h.asked_limit(), Some(3));
    assert_eq!(out.scanned, 3);
    assert_eq!(h.driven(), vec!["run-0", "run-1", "run-2"]);
}

#[tokio::test]
async fn clamps_a_nonsensical_limit_rather_than_asking_for_zero_rows() {
    let h = harness(vec![row("run-1")], Opts::default());
    sweep_reclaimable_runs(Some(0), &h.deps).await.unwrap();
    assert_eq!(h.asked_limit(), Some(1));
}

// ── Being visible when it stops working ──────────────────────────────────────

#[test]
fn declares_timings_the_scheduler_can_call_hung() {
    // The JobSpec itself is assembled when the scheduler crosses (see the
    // reclaim.rs header); these are the numbers it will be built from, so a
    // change to any of them is a deliberate act that touches this test.
    // TS adds `> 0` guards on maxRunMs and firstRunDelayMs; the exact-value
    // pins below are those guards made stronger — the only way this test
    // still passes with the bound gone is if somebody edited it on purpose.
    assert_eq!(RECLAIM_JOB, "run-reclaim");
    assert_eq!(RECLAIM_EVERY_MS, 30_000);
    assert_eq!(RECLAIM_MAX_RUN_MS, 60_000);
    assert_eq!(RECLAIM_FIRST_RUN_DELAY_MS, 20_000);
    // Not perInstance: the input is a shared table, so the fleet does one
    // sweep per interval rather than one per instance. In TS that is an
    // absent field on the spec; here it will be an absent field on the Rust
    // spec too — nothing to assert until the type exists to omit it on.
    assert_eq!(RECLAIM_LIMIT, 25);
}

#[tokio::test]
async fn reports_nothing_to_do_as_nothing_to_do() {
    let h = harness(Vec::new(), Opts::default());
    let out = run_reclaim_job(&h.deps).await.unwrap();
    assert_eq!(out, None);
}

#[tokio::test]
async fn returns_a_sentence_naming_what_the_pass_did() {
    let mut parked = row("run-2");
    parked.state = RunState::Awaiting;
    parked.lease_expires_at = None;
    let h = harness(vec![row("run-1"), parked], Opts::default());
    let line = run_reclaim_job(&h.deps).await.unwrap().unwrap();
    settle().await;

    assert!(line.contains("2 run(s) due"), "{line}");
    assert!(
        line.contains("1 reclaimed from a driver that died"),
        "{line}"
    );
    assert!(line.contains("1 not drivable"), "{line}");
}

#[tokio::test]
async fn errs_when_a_hand_over_errs_so_the_failure_reaches_observability() {
    // `drive` is total for everything a run can do to itself. An error out of
    // it means the store or the lease is broken under the whole runtime, and
    // the scheduler's error state is the only place that fact reaches a
    // person.
    let mut r = row("run-1");
    r.attempt = 2;
    let h = harness(
        vec![r],
        Opts::with_drive(Arc::new(|_run_id| {
            Box::pin(async { Err("connection pool exhausted".to_string()) })
        })),
    );
    let err = run_reclaim_job(&h.deps).await.unwrap_err();
    assert!(err.contains("hand-over(s) threw"), "{err}");
}

#[tokio::test]
async fn lets_a_failing_due_query_err_rather_than_reporting_a_quiet_empty_pass() {
    // A sweeper that cannot see the queue reports zero runs due, which reads
    // exactly like a healthy idle fleet. That silence is the disease.
    let deps = ReclaimDeps {
        due: Arc::new(|_limit| {
            Box::pin(async {
                Err(sqlx::Error::Io(std::io::Error::other(
                    "relation \"runs\" does not exist",
                )))
            })
        }),
        definition_for: Arc::new(|_| None),
        drive: Arc::new(|run_id| {
            Box::pin(async move { Ok(dr(&run_id, DriveStop::Done, None, None)) })
        }),
        now: Arc::new(|| NOW),
    };
    let err = run_reclaim_job(&deps).await.unwrap_err();
    assert!(
        err.contains("relation \"runs\" does not exist"),
        "the query's own error must reach the scheduler verbatim: {err}"
    );
}

#[tokio::test]
async fn does_not_let_a_detached_drive_that_errs_take_the_process_down() {
    // attempt 0, so the hand-over is detached: the pass resolves cleanly and
    // the error is caught at the kick site rather than escaping the task.
    let h = harness(
        vec![row("run-1")],
        Opts::with_drive(Arc::new(|_run_id| {
            Box::pin(async { Err("redis is on fire".to_string()) })
        })),
    );
    let out = sweep_reclaimable_runs(None, &h.deps).await.unwrap();
    assert_eq!(out.driven, 1);
    assert_eq!(out.failed, 0);
    settle().await;
}

#[test]
fn describe_sweep_says_only_what_happened() {
    let line = describe_sweep(&ReclaimSweep {
        scanned: 4,
        driven: 2,
        reclaimed: 1,
        given_up: 1,
        unknown_kinds: 1,
        live: 0,
        not_drivable: 0,
        failed: 0,
        stalest_ms: 125_000,
    });
    assert_eq!(
        line,
        "4 run(s) due — 2 handed to a driver, 1 reclaimed from a driver that died, \
         1 given up on (attempts spent), 1 of a kind this instance cannot drive, \
         stalest lease expired 2 minutes ago"
    );
}
