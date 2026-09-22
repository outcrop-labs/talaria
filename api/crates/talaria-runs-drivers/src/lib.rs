// THE DRIVER REGISTRY — the shutdown half of "a roll must never stall work".
//
// The api's graceful shutdown drains HTTP and the scheduler, but run drivers
// are detached tasks: a roll killed them mid-step holding their Redis leases,
// and the next instance's reclaim could not take the work until each lease
// TTL lapsed (eleven minutes for a work session) — after which the pickup was
// counted as a driver death (attempt+1) and the owed turn was retired. Ten
// rolls in a day burned sessions to exhaustion with zero model calls: the
// "tasks stuck in progress for hours" report, in full.
//
// THE CONTRACT: every live driver registers itself here for the life of its
// drive. On shutdown, `drain` fires each driver's abort (the same signal a
// lost lease uses — steps honor it before outward calls) and gives the
// cleanup its grace; the drivers' own exit path releases their leases. The
// next instance's sweep (30s) picks every run up immediately, re-entering
// from the last persisted checkpoint — which is exactly what reclaim is for,
// minus the eleven-minute wait and the false attempt.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::watch;

/// What a driver hands the registry: the channel that aborts its step.
pub type AbortHandle = Arc<watch::Sender<bool>>;

fn registry() -> &'static Mutex<HashMap<String, AbortHandle>> {
    static REG: OnceLock<Mutex<HashMap<String, AbortHandle>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register the driver for a run. The handle is the abort channel the drive
/// already owns; the registry only needs to reach it.
pub fn register(run_id: &str, abort: AbortHandle) {
    registry()
        .lock()
        .expect("driver registry")
        .insert(run_id.to_string(), abort);
}

/// Remove a driver that finished under its own power. Idempotent: a drain
/// may have cleared the map first.
pub fn unregister(run_id: &str) {
    registry().lock().expect("driver registry").remove(run_id);
}

/// Fire ONE driver's abort — the cancel half of the stop route. A cancel
/// that only flips the row leaves the local driver streaming inside a send
/// step that may legitimately run for hours; the abort is what ends the
/// in-flight call, and it is the same signal a lost lease and the shutdown
/// drain use, so the at-least-once machinery is the machinery that resumes.
/// False when no live driver holds the run — a cancel from another
/// instance, or a driver that already exited under its own power; the row
/// state is the truth in both cases.
pub fn fire(run_id: &str) -> bool {
    let reg = registry().lock().expect("driver registry");
    match reg.get(run_id) {
        Some(handle) => handle.send(true).is_ok(),
        None => false,
    }
}

/// How many drivers this process is holding right now.
pub fn live_count() -> usize {
    registry().lock().expect("driver registry").len()
}

/// Fire every live driver's abort and give the cleanup its grace. A driver
/// deep in a model call sees the abort at its next await; the step's drop is
/// the same event a lost lease produces, so the at-least-once machinery is
/// the machinery that resumes. Entries stay registered until each driver's
/// own exit unregisters, so the post-grace count is honest.
pub async fn drain(grace_ms: u64) {
    let handles: Vec<AbortHandle> = {
        let reg = registry().lock().expect("driver registry");
        reg.values().cloned().collect()
    };
    if handles.is_empty() {
        return;
    }
    tracing::info!(
        "[drivers] shutdown drain: aborting {} live run driver(s), grace {}ms",
        handles.len(),
        grace_ms
    );
    for abort in &handles {
        abort.send_if_modified(|v| {
            if !*v {
                *v = true;
                true
            } else {
                false
            }
        });
    }
    tokio::time::sleep(std::time::Duration::from_millis(grace_ms)).await;
    let remaining = live_count();
    if remaining > 0 {
        tracing::warn!(
            "[drivers] {remaining} driver(s) still in flight after the drain grace — their \
             leases expire on the TTL and the next reclaim takes them"
        );
    }
}
