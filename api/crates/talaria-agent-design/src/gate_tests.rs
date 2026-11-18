// The hire route's order, pinned: claim → enqueue → (on failure) release.
// fleet_create.rs goes through `gate_hire`; consuming the design after the
// enqueue (the order before the review) cannot be written through it, and
// each case below fails if the order changes.

use std::sync::{Arc, Mutex};

use super::{DesignError, Refusal, gate_hire};

type Log = Arc<Mutex<Vec<&'static str>>>;

fn step(log: &Log, name: &'static str) {
    log.lock().unwrap().push(name);
}

async fn run(
    claim: Option<Result<(), DesignError>>,
    enqueue: Result<u8, &'static str>,
) -> (
    Vec<&'static str>,
    Result<Result<u8, &'static str>, DesignError>,
) {
    let log: Log = Arc::default();
    let (lc, le, lr) = (log.clone(), log.clone(), log.clone());
    let claim = claim.map(|c| {
        Box::pin(async move {
            step(&lc, "claim");
            c
        }) as futures_util::future::BoxFuture<'static, Result<(), DesignError>>
    });
    let enqueue = Box::pin(async move {
        step(&le, "enqueue");
        enqueue
    });
    let out = gate_hire(claim, enqueue, move || {
        Box::pin(async move { step(&lr, "release") })
    })
    .await;
    let steps = log.lock().unwrap().clone();
    (steps, out)
}

#[tokio::test]
async fn the_design_is_claimed_before_the_hire_is_enqueued() {
    let (steps, out) = run(Some(Ok(())), Ok(1)).await;
    assert_eq!(steps, ["claim", "enqueue"]);
    assert!(matches!(out, Ok(Ok(1))));
}

#[tokio::test]
async fn a_refused_claim_enqueues_nothing() {
    let (steps, out) = run(Some(Err(DesignError::Refused(Refusal::Hiring))), Ok(1)).await;
    assert_eq!(
        steps,
        ["claim"],
        "a second tab's hire never reaches the queue"
    );
    assert!(matches!(out, Err(DesignError::Refused(Refusal::Hiring))));
}

#[tokio::test]
async fn a_failed_enqueue_hands_the_design_back() {
    let (steps, out) = run(Some(Ok(())), Err("insert failed")).await;
    assert_eq!(steps, ["claim", "enqueue", "release"]);
    assert!(matches!(out, Ok(Err("insert failed"))));
}

#[tokio::test]
async fn a_hire_without_a_design_only_enqueues() {
    let (steps, _) = run(None, Err("insert failed")).await;
    assert_eq!(steps, ["enqueue"], "nothing to hand back");
}
