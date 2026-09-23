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
// update-reconcile — the minute hand, below. reconcile_boot is the only
// writer that finishes a cutover, and before it the callers were the 6h
// check (whose interval lease a dead container's ghost can hold for the
// whole interval) and the admin read (which arrives through an edge that
// a stranded cutover has not raised — green publishes no port of its
// own). A cutover that outlives its armed helper must not wait hours for
// a reader: every minute, whichever replica holds the tick reconciles.
// Idle, that is one settings-row read, plus — when this container's image
// digest is one the fleet has not been rolled for — arming the fleet roll.
// The roll itself is detached: a fleet of agents outlives this tick.
//
// The auto half's consent story is exactly two switches, both off until
// a human: adoption (migrated — the engine refuses instances it never
// adopted) and the toggle (auto_update — default false). A registry that
// moved is never, by itself, a reason anything changed on a host. The
// fleet roll is not a third switch: it is the deploy finishing. The
// consent was the apply (or the orchestrator that replaced the image).

use std::sync::{Arc, OnceLock};

use futures_util::future::BoxFuture;
use talaria_scheduler::{JobName, JobSpec};
use talaria_secretbox::SecretBox;
use talaria_state::AppState;

use talaria_agent_auth::now_ms;
use talaria_runs_lease::{
    AcquireResult, RedisLeases, acquire_lease, keep_lease_alive, lease_key, release_lease,
};
use talaria_update_mode::{InstallMode, install_mode};
use talaria_update_registry::resolve_latest;
use talaria_update_roll::{reconcile_boot, roll, run_in_flight, self_image_digest, tidy};
use talaria_update_state::{Pin, RunBy, load, patch};

/// Wired from the api binary. This crate does not depend on fleet reconcile;
/// an unwired edge is a sentence, and the digest stays unrecorded so the
/// next tick retries rather than pretending the fleet moved.
pub static ROLL_FLEET: OnceLock<
    Arc<
        dyn Fn(
                sqlx::PgPool,
                SecretBox,
            ) -> BoxFuture<'static, Result<(Vec<String>, Vec<String>), String>>
            + Send
            + Sync,
    >,
> = OnceLock::new();

/// The digest the fleet was last rolled onto. Separate from the update row
/// so the panel's wire shape does not grow a bookkeeping field.
const FLEET_ROLLED_KEY: &str = "update.fleet_rolled_digest";

/// Own lease, not the app roll's. That lock is never released (the roller
/// stops its own container), so sharing it would hold the fleet roll for
/// the whole TTL after every cutover.
const FLEET_ROLL_LEASE: &str = "fleet-roll";
const FLEET_ROLL_TTL_MS: u64 = 20 * 60_000;

fn fleet_roll_lease_key() -> String {
    lease_key("update", FLEET_ROLL_LEASE)
}

pub const UPDATE_CHECK_EVERY_MS: u64 = 6 * 60 * 60_000;
pub const UPDATE_CHECK_FIRST_RUN_DELAY_MS: u64 = 5 * 60_000;
pub const UPDATE_CHECK_MAX_RUN_MS: u64 = 20 * 60_000;

/// The minute hand's cadence. One tick of slack for a lease held by a
/// container that died mid-interval; a first delay past green's boot
/// migrations so a crash-looping instance never reaches it.
pub const UPDATE_RECONCILE_EVERY_MS: u64 = 60_000;
pub const UPDATE_RECONCILE_FIRST_RUN_DELAY_MS: u64 = 30_000;
pub const UPDATE_RECONCILE_MAX_RUN_MS: u64 = 2 * 60_000;

/// The deps of both scheduled halves (the 6h check and the minute hand):
/// the whole AppState (a clone of lazy edges). Registration only CAPTURES
/// — the redis connection the roll lock needs is taken at run time, never
/// at boot, so a dead redis arms the schedule anyway and the job's error
/// is the honest sentence.
pub struct UpdateDeps {
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

/// The digest this process should roll the fleet onto, if any.
///
/// Image installs only. A run in flight is the app's own cutover — the
/// fleet waits until that lands. An adopted install rolls only from the
/// container that IS the pin (green); blue during its drain is not a second
/// deploy. An unadopted image install rolls when its own digest is not the
/// one the fleet was last rolled for — a restart of the same image is not
/// a deploy. Checkout and dev never reach this with `image_install`.
pub fn fleet_roll_digest<'a>(
    image_install: bool,
    run_in_flight: bool,
    migrated: bool,
    self_digest: Option<&'a str>,
    pinned_digest: Option<&str>,
    already_rolled: Option<&str>,
) -> Option<&'a str> {
    if !image_install || run_in_flight {
        return None;
    }
    let ours = self_digest.filter(|d| !d.is_empty())?;
    if migrated && pinned_digest != Some(ours) {
        return None;
    }
    if already_rolled == Some(ours) {
        return None;
    }
    Some(ours)
}

async fn recorded_fleet_digest(pg: &sqlx::PgPool) -> Option<String> {
    let v = talaria_settings::get_setting(pg, FLEET_ROLLED_KEY, serde_json::Value::Null).await;
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

async fn record_fleet_rolled(pg: &sqlx::PgPool, digest: &str) -> Result<(), String> {
    talaria_settings::set_setting(pg, FLEET_ROLLED_KEY, &serde_json::json!(digest))
        .await
        .map_err(|e| e.to_string())
}

/// Arm the fleet roll when [`fleet_roll_digest`] says this container is a
/// deploy the fleet has not caught up to. Detached: the reconcile tick must
/// not wait on a fleet. The digest is recorded only when the roll returns,
/// so a death mid-roll retries on the next tick instead of looking done.
/// A lease keeps two replicas from rolling the same fleet twice. Failures
/// are logged, never the caller's error — a fleet that could not roll must
/// not fail the cutover reconcile that just landed.
async fn note_fleet_roll(state: &AppState) {
    match arm_fleet_roll(state).await {
        Ok(Some(line)) => tracing::info!("[update] {line}"),
        Ok(None) => {}
        Err(e) => tracing::warn!("[update] fleet roll not armed: {e}"),
    }
}

async fn arm_fleet_roll(state: &AppState) -> Result<Option<String>, String> {
    if install_mode() != InstallMode::Image {
        return Ok(None);
    }
    let row = load(&state.pg).await;
    let in_flight = row.last_run.as_ref().is_some_and(run_in_flight);
    let self_digest = self_image_digest().await;
    let already = recorded_fleet_digest(&state.pg).await;
    let Some(digest) = fleet_roll_digest(
        true,
        in_flight,
        row.migrated,
        self_digest.as_deref(),
        row.pinned.as_ref().map(|p| p.digest.as_str()),
        already.as_deref(),
    ) else {
        return Ok(None);
    };
    let digest = digest.to_string();

    let roll = ROLL_FLEET
        .get()
        .cloned()
        .ok_or_else(|| "fleet roll not wired".to_string())?;
    let sb = state.secretbox().await?;

    let conn = state
        .redis()
        .await
        .map_err(|e| format!("redis unreachable for the fleet roll: {e}"))?;
    let mut backend = RedisLeases::new(conn.clone());
    let token = match acquire_lease(&mut backend, &fleet_roll_lease_key(), FLEET_ROLL_TTL_MS).await
    {
        AcquireResult::Acquired(t) => t,
        // Quiet: the minute tick will see this for as long as the roll
        // runs. The arm that took the lease already said so.
        AcquireResult::Held => return Ok(None),
        AcquireResult::Unavailable(e) => {
            return Err(format!("the fleet-roll lock could not be taken: {e}"));
        }
    };

    let pg = state.pg.clone();
    let release_conn = conn.clone();
    let beat_conn = conn;
    let beat_token = token.clone();
    let digest_for_roll = digest.clone();
    tokio::spawn(async move {
        let beat = keep_lease_alive(beat_conn, beat_token, FLEET_ROLL_TTL_MS, Default::default());
        match roll(pg.clone(), sb).await {
            Ok((rolled, warnings)) => {
                if let Err(e) = record_fleet_rolled(&pg, &digest_for_roll).await {
                    tracing::warn!(
                        "[update] the fleet roll landed but the digest did not record: {e}"
                    );
                } else {
                    tracing::info!(
                        "[update] rolled {} agent(s) onto the deployed image",
                        rolled.len()
                    );
                }
                for w in warnings {
                    tracing::warn!("[update] fleet roll: {w}");
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[update] the fleet roll after the deploy failed — it will retry: {e}"
                );
            }
        }
        drop(beat);
        let mut backend = RedisLeases::new(release_conn);
        let _ = release_lease(&mut backend, &token).await;
    });
    Ok(Some(format!("rolling the fleet onto {digest}")))
}

pub fn update_check_job_spec(deps: Arc<UpdateDeps>) -> JobSpec {
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
                note_fleet_roll(&deps.state).await;

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
                        let at = talaria_agent_auth::epoch_ms_to_iso(now_ms());
                        let available = pin.clone();
                        patch(&pg, |mut s| {
                            s.last_check = Some(talaria_update_state::CheckRecord {
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
                        let at = talaria_agent_auth::epoch_ms_to_iso(now_ms());
                        patch(&pg, |mut s| {
                            s.last_check = Some(talaria_update_state::CheckRecord {
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

/// The minute hand: reconcile, then arm the fleet roll when this container
/// is a deploy the fleet has not caught up to. reconcile_boot self-gates —
/// a settings row with no in-flight run is the idle cost — and its own
/// sentences (the holds, the landings, the heal) are the job's whole
/// output. The fleet roll is detached and does not replace that sentence.
/// NOT per_instance for the same reason as the check: one actor per tick
/// is enough, and any replica can be that actor.
pub fn update_reconcile_job_spec(deps: Arc<UpdateDeps>) -> JobSpec {
    JobSpec {
        name: JobName::UpdateReconcile,
        every_ms: UPDATE_RECONCILE_EVERY_MS,
        first_run_delay_ms: Some(UPDATE_RECONCILE_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(UPDATE_RECONCILE_MAX_RUN_MS),
        per_instance: false,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move {
                let line = reconcile_boot(&deps.state.pg).await?;
                note_fleet_roll(&deps.state).await;
                Ok(line)
            })
        }),
    }
}

pub fn register_update_check_job(deps: Arc<UpdateDeps>) {
    talaria_scheduler::register_job(update_check_job_spec(deps));
}

pub fn register_update_reconcile_job(deps: Arc<UpdateDeps>) {
    talaria_scheduler::register_job(update_reconcile_job_spec(deps));
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
    #[test]
    fn the_fleet_rolls_only_for_a_deploy_it_has_not_caught() {
        let deployed = "sha256:abc";
        let older = "sha256:old";
        // Adopted green, pin matches, fleet still on the previous digest.
        assert_eq!(
            fleet_roll_digest(
                true,
                false,
                true,
                Some(deployed),
                Some(deployed),
                Some(older)
            ),
            Some(deployed)
        );
        // Already rolled onto this digest: a restart is not a deploy.
        assert_eq!(
            fleet_roll_digest(
                true,
                false,
                true,
                Some(deployed),
                Some(deployed),
                Some(deployed)
            ),
            None
        );
        // Blue during the drain is not the pin. It must not roll the fleet
        // onto the image that is about to stop.
        assert_eq!(
            fleet_roll_digest(true, false, true, Some(older), Some(deployed), None),
            None
        );
        // The app's own cutover is still in flight.
        assert_eq!(
            fleet_roll_digest(true, true, true, Some(deployed), Some(deployed), None),
            None
        );
        // Checkout and dev are not image installs.
        assert_eq!(
            fleet_roll_digest(false, false, false, Some(deployed), None, None),
            None
        );
        // Unadopted image install (dokploy, compose): own digest moved.
        assert_eq!(
            fleet_roll_digest(true, false, false, Some(deployed), None, Some(older)),
            Some(deployed)
        );
        // Same image, restarted: not a deploy.
        assert_eq!(
            fleet_roll_digest(true, false, false, Some(deployed), None, Some(deployed)),
            None
        );
        // No digest at all (a dev box that somehow asked).
        assert_eq!(
            fleet_roll_digest(true, false, false, None, None, None),
            None
        );
        assert_eq!(
            fleet_roll_digest(true, false, false, Some(""), None, None),
            None
        );
    }
}
