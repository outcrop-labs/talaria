// update-check — the scheduled half of the engine. Every 6h: reconcile
// any run this container owes (green's boot reconcile rides the first
// tick after a cutover, 5min in), resolve the tracked tag to a digest,
// record what was available, and — ONLY behind the default-off
// auto-update toggle — roll to it. Manual apply on the panel needs none
// of this; the job is the "hands off" mode an admin opts into.
//
// NOT per_instance: the scheduler's redis lease picks one replica per
// tick (two replicas during a rolling overlap is a supported topology —
// the lease covers it), and the ROLL lock inside covers the manual apply
// that lands mid-check.
//
// The auto half's consent story is exactly two switches, both off until
// a human: adoption (migrated — the engine refuses instances it never
// adopted) and the toggle (auto_update — default false). A registry that
// moved is never, by itself, a reason anything changed on a host.

use std::sync::Arc;

use crate::scheduler::{JobName, JobSpec};
use crate::state::AppState;

use super::mode::{InstallMode, install_mode};
use super::registry::resolve_latest;
use super::roll::{reconcile_boot, roll, run_in_flight, tidy};
use super::state::{Pin, RunBy, load, patch};

pub const UPDATE_CHECK_EVERY_MS: u64 = 6 * 60 * 60_000;
pub const UPDATE_CHECK_FIRST_RUN_DELAY_MS: u64 = 5 * 60_000;
pub const UPDATE_CHECK_MAX_RUN_MS: u64 = 20 * 60_000;

/// The deps are the whole AppState (a clone of lazy edges): registration
/// only CAPTURES — the redis connection the roll lock needs is taken at
/// run time, never at boot, so a dead redis arms the schedule anyway and
/// the job's error is the honest sentence.
pub struct UpdateCheckDeps {
    pub state: AppState,
}

/// The auto half's decision, pure so the panel and the tests can read the
/// same policy the job applies: the toggle, the adoption, a digest that
/// actually moved, and no run in flight. A migrated install with no pin is
/// corrupt state (adoption pins), and an engine that replaces containers
/// does not act on corrupt state unattended — `pinned.is_some_and` is that
/// refusal.
pub fn should_auto_apply(
    auto_update: bool,
    migrated: bool,
    run_in_flight: bool,
    pinned: Option<&Pin>,
    available: &Pin,
) -> bool {
    auto_update && migrated && !run_in_flight && pinned.is_some_and(|p| p != available)
}

pub fn update_check_job_spec(deps: Arc<UpdateCheckDeps>) -> JobSpec {
    JobSpec {
        name: JobName::UpdateCheck,
        every_ms: UPDATE_CHECK_EVERY_MS,
        // Green's boot reconcile rides the first tick: soon enough that a
        // cutover's run lands done minutes after green boots, late enough
        // that a crash-looping instance never reaches it.
        first_run_delay_ms: Some(UPDATE_CHECK_FIRST_RUN_DELAY_MS),
        // The roll can legitimately spend its pull inside one run; the
        // scheduler's lease and the roll lock both expire by TTL when the
        // roller's container stops itself mid-run — that is the designed
        // end of an auto roll, not a hang.
        max_run_ms: Some(UPDATE_CHECK_MAX_RUN_MS),
        per_instance: false,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move {
                let pg = deps.state.pg.clone();

                // Green's half of any in-flight run, first: the check is a
                // reader of the state row and must not read past an
                // un-reconciled cutover.
                if let Some(line) = reconcile_boot(&pg).await? {
                    tracing::info!("[update] {line}");
                }

                if install_mode() != InstallMode::Image {
                    // The dormant installs (checkout, dev, off): the honest
                    // quiet, not an error — there is nothing to check.
                    return Ok(None);
                }

                let state = load(&pg).await;
                let in_flight = state.last_run.as_ref().is_some_and(run_in_flight);

                // Resolve and record, whatever the answer was — the panel's
                // "last checked" is this row.
                let pin = match resolve_latest().await {
                    Ok(pin) => {
                        let at = crate::agent_auth::epoch_ms_to_iso(now_ms());
                        let available = pin.clone();
                        patch(&pg, |mut s| {
                            s.last_check = Some(super::state::CheckRecord {
                                at,
                                available: Some(available),
                                error: None,
                            });
                            s
                        })
                        .await
                        .map_err(|e| format!("the check record did not write: {e}"))?;
                        pin
                    }
                    Err(e) => {
                        // Failed checks are said out loud (the scheduler's
                        // Err contract: never swallowed) AND recorded — the
                        // panel shows the sentence beside its button.
                        let at = crate::agent_auth::epoch_ms_to_iso(now_ms());
                        patch(&pg, |mut s| {
                            s.last_check = Some(super::state::CheckRecord {
                                at,
                                available: None,
                                error: Some(e.clone()),
                            });
                            s
                        })
                        .await
                        .map_err(|e2| format!("the failed check did not record: {e2}"))?;
                        return Err(format!("the registry check failed: {e}"));
                    }
                };

                if should_auto_apply(
                    state.auto_update,
                    state.migrated,
                    in_flight,
                    state.pinned.as_ref(),
                    &pin,
                ) {
                    let conn = deps
                        .state
                        .redis()
                        .await
                        .map_err(|e| format!("redis unreachable for the auto roll: {e}"))?;
                    tracing::info!(
                        "[update] auto-applying {} ({} → {})",
                        pin.version,
                        state
                            .pinned
                            .as_ref()
                            .map(|p| p.version.as_str())
                            .unwrap_or("unpinned"),
                        pin.version
                    );
                    // The roll ends by stopping this container; nothing
                    // after it is promised to run.
                    roll(&pg, conn, &pin, RunBy::Auto).await?;
                    return Ok(Some(format!("auto-rolled to {}", pin.version)));
                }

                match tidy(&pg).await {
                    Ok(_) => Ok(Some(format!("checked: {}", pin.version))),
                    // Tidy is janitorial; a tidy failure is logged, never
                    // allowed to mask a green check.
                    Err(e) => Ok(Some(format!("checked: {} (tidy: {e})", pin.version))),
                }
            })
        }),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn register_update_check_job(deps: Arc<UpdateCheckDeps>) {
    crate::scheduler::register_job(update_check_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(v: &str) -> Pin {
        Pin {
            digest: format!("sha256:{:0>64}", v),
            version: v.into(),
        }
    }

    #[test]
    fn the_auto_half_needs_both_switches_and_a_moved_digest() {
        let a = pin("a");
        let a2 = pin("a");
        let b = pin("b");
        // Both switches on, digest moved: the only yes.
        assert!(should_auto_apply(true, true, false, Some(&a), &b));
        // Each switch off, alone:
        assert!(!should_auto_apply(false, true, false, Some(&a), &b));
        assert!(!should_auto_apply(true, false, false, Some(&a), &b));
        // A run in flight:
        assert!(!should_auto_apply(true, true, true, Some(&a), &b));
        // The same digest (an unpinned-but-current or a re-check): no.
        assert!(!should_auto_apply(true, true, false, Some(&a), &a2));
        assert!(!should_auto_apply(true, true, false, None, &a));
    }
}
