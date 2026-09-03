// The roll engine — one update, start to finish, split across two
// processes by design (mod.rs's "self" law):
//
//   BLUE (the process that starts the roll — the live container):
//     gates → lock → pull the digest → render + env → green up →
//     health gate → alias onto green → record cutting-over → stop ITSELF.
//   GREEN (the container blue brought up):
//     boot reconcile — verify the edge routes to us, keep blue stopped for
//     the keep-window, mark the run done.
//
// Blue's last act stops the very process performing it, so nothing after
// `stop_container(self)` in `roll` can be relied on to run — and nothing
// needs to: the state row said `cutting-over` BEFORE the stop (crash-safe:
// any reader sees an in-flight run, not a false done), and green's
// reconcile is the only writer that moves it to `done`.
//
// THE LOCK IS NEVER RELEASED, deliberately. The holder's last act is to
// kill its own container; releasing on the way down would mean trusting a
// SIGTERM drain to run cleanup code, and the NEXT roll (green's) is gated
// by reconcile marking done, not by the lock anyway. The TTL is the whole
// story: it bounds how long a roller that died WITHOUT stopping itself (a
// crash mid-pull) blocks the next attempt, and a heartbeat keeps a
// legitimately slow pull (900s of patience in the pull verb) from losing
// the lock mid-roll.
//
// EVERY VERB GATES THE SAME TWO WAYS: install mode (mode.rs — the honest
// refusal for checkout/dev/off) and ADOPTION (state.migrated — the engine
// refuses to touch an instance that never handed over the keys; this is
// the dormancy that lets every existing install keep deploying exactly as
// it always has).

use redis::aio::ConnectionManager;
use sqlx::PgPool;

use super::docker::{
    attach_fleet_alias, edge_healthy, inspect_self, pull_image, remove_container, slot_up,
    start_container, stop_container, wait_healthy_slot,
};
use super::layout::{Slot, default_image_ref, roll_drain_ms, slot_container, update_project};
use super::mode::{InstallMode, install_mode};
use super::registry::is_digest;
use super::render::{
    SlotSpec, digest_ref, render_update_compose, repo_of, slot_spec_from_inspect, write_slot_env,
};
use super::state::{Pin, RunBy, RunRecord, RunState, UpdateState, load, patch, record_run};
use crate::agent_auth::epoch_ms_to_iso;
use crate::fleet::docker::docker;
use crate::runs::lease::{AcquireResult, RedisLeases, keep_lease_alive, lease_key};

const LOG: &str = "[update]";

/// The redis namespace the roll lock lives under ('update', beside 'sched'
/// and 'run' — see lease_key).
const ROLL_LEASE_NS: &str = "update";

/// One lock for the whole install, not per-run: a roll and a rollback and
/// an adoption contend for the same key, because they all move the same
/// two containers.
pub fn roll_lease_key() -> String {
    lease_key(ROLL_LEASE_NS, "roll")
}

/// How long a dead roller's lock blocks the next attempt. Generous against
/// the legitimate worst case (a 900s pull plus a 180s gate plus a drain)
/// because a heartbeat covers the slow-but-alive case and this TTL only
/// matters when nobody is renewing it.
const ROLL_LOCK_TTL_MS: u64 = 20 * 60_000;

/// How long the incoming slot has to reach `healthy`. The compose
/// healthcheck's own start_period is 90s of boot migrations; this is the
/// engine's patience on top, matching the fleet's roll gate.
const HEALTH_GATE_MS: u64 = 180_000;

/// How long a stopped old slot stays as rollback material before tidy
/// removes it. A day is long enough to notice a bad roll and short enough
/// that the worker's disk does not fill with gigabyte images.
pub const KEEP_WINDOW_MS: u64 = 24 * 60 * 60_000;

/// A run older than this that is still in-flight AND we are not its green
/// container is a roller that died mid-roll (a crash between two records):
/// no reconcile will ever finish it, the auto gate would block forever,
/// and the panel would show a spinning wheel that never lands. Generous —
/// past every legitimate phase timeout combined.
const STALE_RUN_MS: i64 = 60 * 60_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn now_iso() -> String {
    epoch_ms_to_iso(now_ms())
}

/// Is a run in one of its three in-flight states?
pub fn run_in_flight(run: &RunRecord) -> bool {
    matches!(
        run.state,
        RunState::Pulling | RunState::Starting | RunState::CuttingOver
    )
}

/// The two gates every acting verb shares, as one sentence-bearing check:
/// the mode refusal first (it is the truer answer about the install), the
/// adoption gate second.
pub fn acting_gate(state: &UpdateState, mode: InstallMode) -> Result<(), String> {
    if let Some(sentence) = mode.refusal() {
        return Err(sentence.to_string());
    }
    if !state.migrated {
        return Err(
            "This instance has not handed update control to the app — adopt it from the admin panel first.".into(),
        );
    }
    Ok(())
}

/// This container's docker name (docker prefixes inspect's Name with a
/// slash; the verbs take it without).
async fn self_name() -> Result<String, String> {
    let doc = inspect_self().await?;
    doc.get("Name")
        .and_then(|n| n.as_str())
        .map(|n| n.trim_start_matches('/').to_string())
        .ok_or_else(|| "the self inspect document carries no name".into())
}

/// Which slot THIS process is, by container name. None outside the
/// updater-owned project (pre-adoption, a dev box, the dokploy container
/// during adoption) — the callers sentence that themselves.
pub async fn self_slot() -> Result<Slot, String> {
    let name = self_name().await?;
    let a = slot_container(Slot::A);
    let b = slot_container(Slot::B);
    if name == a {
        Ok(Slot::A)
    } else if name == b {
        Ok(Slot::B)
    } else {
        Err(format!(
            "this process is not one of the updater-owned slots ({name}) — adoption has not moved this install to them"
        ))
    }
}

fn other(slot: Slot) -> Slot {
    match slot {
        Slot::A => Slot::B,
        Slot::B => Slot::A,
    }
}

/// The digest-pinned image reference a slot container runs
/// (`Config.Image`), if the container exists.
async fn slot_image_ref(slot: Slot) -> Option<String> {
    let (out, _) = docker(
        &["inspect", "-f", "{{.Config.Image}}", &slot_container(slot)],
        std::time::Duration::from_secs(10),
    )
    .await
    .ok()?;
    let r = out.trim().to_string();
    (!r.is_empty()).then_some(r)
}

/// The digest half of a `repo@sha256:…` reference, if it carries one.
pub fn digest_suffix(reference: &str) -> Option<&str> {
    reference
        .split_once('@')
        .map(|(_, d)| d)
        .filter(|d| is_digest(d))
}

/// Write the compose file (both slots, flip-rendered) and both slot env
/// files from a spec read off the LIVE container. Called by every roll —
/// drift between rolls cannot survive it.
async fn write_project(
    spec: &SlotSpec,
    active: Slot,
    active_digest: &str,
    incoming: Slot,
    incoming_digest: &str,
) -> Result<(), String> {
    let repo = repo_of(&default_image_ref());
    let tree = render_update_compose(spec, &repo, active, active_digest, incoming_digest);
    let yaml = crate::fleet::render::yaml11_emit(&tree);
    let path = super::layout::compose_file();
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("update dir unwritable ({}): {e}", dir.display()))?;
    }
    // 0600 beside the env files: the compose carries no secrets itself, but
    // the same directory does, and the discipline is one rule, not per-file.
    let write = || std::fs::write(&path, yaml);
    write().map_err(|e| format!("compose write failed ({}): {e}", path.display()))?;
    if let Ok(meta) = std::fs::metadata(&path) {
        use std::os::unix::fs::PermissionsExt;
        let mut p = meta.permissions();
        p.set_mode(0o600);
        let _ = std::fs::set_permissions(&path, p);
    }
    // Both env files from the same spec: the active slot's re-write heals
    // drift (a container env changed since its last roll), the incoming
    // slot's is the one this roll rides on.
    write_slot_env(Slot::A, &spec.env).await?;
    write_slot_env(Slot::B, &spec.env).await?;
    let _ = incoming; // both written; the flip lives in the compose above
    Ok(())
}

/// Record a state transition on the current run (or a fresh one, when
/// `run` is None). Errors are returned, not swallowed — a state row that
/// silently failed to write is a run the panel cannot see.
async fn transition<F>(pg: &PgPool, run: Option<RunRecord>, f: F) -> Result<(), String>
where
    F: FnOnce(&mut RunRecord),
{
    patch(pg, |mut s| {
        let mut r = run.unwrap_or_else(|| {
            s.last_run.clone().unwrap_or(RunRecord {
                state: RunState::Pulling,
                from: s.pinned.clone().unwrap_or(Pin {
                    digest: String::new(),
                    version: String::new(),
                }),
                to: Pin {
                    digest: String::new(),
                    version: String::new(),
                },
                by: RunBy::Manual,
                started_at: now_iso(),
                finished_at: None,
                error: None,
            })
        });
        f(&mut r);
        record_run(&mut s, r);
        s
    })
    .await
    .map(|_| ())
    .map_err(|e| format!("the update state row did not write: {e}"))
}

/// Roll this install to a digest. The caller has already resolved and
/// validated the pin (routes: the admin pressed the button; job: the check
/// found it); everything here is choreography.
pub async fn roll(pg: &PgPool, conn: ConnectionManager, to: &Pin, by: RunBy) -> Result<(), String> {
    let state = load(pg).await;
    acting_gate(&state, install_mode())?;
    if !is_digest(&to.digest) {
        return Err(format!("refusing to pull {} — not a digest", to.digest));
    }
    if let Some(run) = &state.last_run
        && run_in_flight(run)
    {
        return Err("an update is already in flight".into());
    }

    // The lock: single roller across every replica (the scheduler's own
    // job lease covers the CHECK; this covers the roll itself, which a
    // manual apply can also start).
    let mut backend = RedisLeases::new(conn.clone());
    let token =
        match crate::runs::lease::acquire_lease(&mut backend, &roll_lease_key(), ROLL_LOCK_TTL_MS)
            .await
        {
            AcquireResult::Acquired(t) => t,
            AcquireResult::Held => {
                return Err("another update holds the roll lock".into());
            }
            AcquireResult::Unavailable(e) => {
                return Err(format!("the roll lock could not be taken (redis): {e}"));
            }
        };
    // Renewed while this process lives; see the header for why it is never
    // released.
    let _beat = keep_lease_alive(conn, token, ROLL_LOCK_TTL_MS, Default::default());

    let me = self_slot().await?;
    let incoming = other(me);
    let from = state.pinned.clone().unwrap_or_else(|| Pin {
        digest: String::new(),
        version: String::new(),
    });
    let fresh = || RunRecord {
        state: RunState::Pulling,
        from: from.clone(),
        to: to.clone(),
        by,
        started_at: now_iso(),
        finished_at: None,
        error: None,
    };

    // The spec reads the LIVE container before anything moves.
    let spec = slot_spec_from_inspect(&inspect_self().await?)?;

    // Pull, recorded, before any container exists for the new digest.
    transition(pg, Some(fresh()), |r| r.state = RunState::Pulling).await?;
    let reference = digest_ref(&repo_of(&default_image_ref()), &to.digest);
    if let Err(e) = pull_image(&reference).await {
        transition(pg, Some(fresh()), |r| {
            r.state = RunState::Failed;
            r.finished_at = Some(now_iso());
            r.error = Some(e.clone());
        })
        .await
        .ok();
        return Err(format!("the pull failed: {e}"));
    }

    // Render + bring the incoming slot up on the pulled digest.
    write_project(&spec, me, &from.digest, incoming, &to.digest).await?;
    transition(pg, Some(fresh()), |r| r.state = RunState::Starting).await?;
    if let Err(e) = slot_up(incoming).await {
        transition(pg, Some(fresh()), |r| {
            r.state = RunState::Failed;
            r.finished_at = Some(now_iso());
            r.error = Some(e.clone());
        })
        .await
        .ok();
        return Err(format!("the replacement slot did not start: {e}"));
    }

    // The gate: the compose healthcheck, verbatim. Unhealthy means the new
    // image booted and could not answer /api/healthz — migrations failed,
    // the port never listened, whatever it was, the old container keeps
    // serving and the newcomer goes away.
    if !wait_healthy_slot(incoming, HEALTH_GATE_MS).await {
        let _ = remove_container(&slot_container(incoming)).await;
        // Steady render: both slots name the digest the live one runs, so
        // the file on disk stops referencing the failed one.
        write_project(&spec, me, &from.digest, incoming, &from.digest).await?;
        transition(pg, Some(fresh()), |r| {
            r.state = RunState::Failed;
            r.finished_at = Some(now_iso());
            r.error = Some("the replacement never became healthy — kept the old container".into());
        })
        .await
        .ok();
        return Err("the replacement never became healthy — kept the old container".into());
    }

    // THE agent-plane cutover: the shared alias moves to green. Until
    // blue's stop lands, docker may round-robin two HEALTHY backends —
    // same DB, same wire — and the stop ends the overlap.
    if let Err(e) = attach_fleet_alias(&spec.fleet_network, &slot_container(incoming)).await {
        let _ = remove_container(&slot_container(incoming)).await;
        write_project(&spec, me, &from.digest, incoming, &from.digest).await?;
        transition(pg, Some(fresh()), |r| {
            r.state = RunState::Failed;
            r.finished_at = Some(now_iso());
            r.error = Some(format!("the fleet alias did not move: {e}"));
        })
        .await
        .ok();
        return Err(format!("the fleet alias did not move: {e}"));
    }

    // Crash-safe cutover record: any reader from here on sees a run only
    // green can finish.
    transition(pg, Some(fresh()), |r| r.state = RunState::CuttingOver).await?;
    tracing::info!("{LOG} cutover: green healthy and aliased, stopping this container");

    // The last act. This process's container drains (in-flight requests
    // finish, agents get a clean socket close) and does not come back;
    // green's boot reconcile finishes the run.
    let _ = stop_container(&slot_container(me), roll_drain_ms()).await;
    Ok(())
}

/// Green's half: if the last run is in-flight and THIS container is the
/// digest it was rolling to, the edge is verified, the run lands done and
/// the pin moves. Called from the update-check job's every tick and the
/// admin route's read — whichever gets there first after green boots.
///
/// Also the staleness heal: an in-flight run older than STALE_RUN_MS on a
/// container that is NOT its green (blue crashed between records; a fresh
/// blue process is running it) is marked failed so the panel and the auto
/// gate stop waiting for a reconcile that will never come.
pub async fn reconcile_boot(pg: &PgPool) -> Result<Option<String>, String> {
    if install_mode() != InstallMode::Image {
        return Ok(None);
    }
    let state = load(pg).await;
    let Some(run) = state.last_run.clone() else {
        return Ok(None);
    };
    if !run_in_flight(&run) {
        return Ok(None);
    }

    // Are we green? The one honest signal: our own image is the digest the
    // run was rolling to.
    let doc = match inspect_self().await {
        Ok(d) => d,
        Err(_) => return Ok(None), // not in a container (a dev box reading the row)
    };
    let ours = doc
        .get("Config")
        .and_then(|c| c.get("Image"))
        .and_then(|i| i.as_str())
        .and_then(digest_suffix);
    let green = ours == Some(run.to.digest.as_str());

    if green {
        // The edge must route to us before the run lands done: the port the
        // world dials is the port that must answer.
        if !edge_healthy(60_000).await {
            return Err("the edge is not healthy after the cutover — the run stays open".into());
        }
        if !verify_through_edge().await {
            return Err(
                "the edge is up but /api/healthz through it did not answer ok — the run stays open"
                    .into(),
            );
        }
        let at = now_iso();
        patch(pg, |mut s| {
            let mut r = run.clone();
            r.state = RunState::Done;
            r.finished_at = Some(at.clone());
            s.pinned = Some(r.to.clone());
            record_run(&mut s, r);
            s
        })
        .await
        .map_err(|e| format!("the done record did not write: {e}"))?;
        return Ok(Some("the pending roll reconciled to done".into()));
    }

    // Not green. A run still in-flight past every legitimate phase timeout
    // is a dead roller; the auto gate and the panel both need it closed.
    let started = crate::agent_auth::iso_to_epoch_ms(&run.started_at).unwrap_or(0);
    if now_ms() - started > STALE_RUN_MS {
        let at = now_iso();
        patch(pg, |mut s| {
            let mut r = run.clone();
            r.state = RunState::Failed;
            r.finished_at = Some(at.clone());
            r.error =
                Some("the rolling container died mid-roll — the old container kept serving".into());
            record_run(&mut s, r);
            s
        })
        .await
        .map_err(|e| format!("the stale-run record did not write: {e}"))?;
        return Ok(Some("a stale in-flight run was closed as failed".into()));
    }
    Ok(None)
}

/// GET /api/healthz through the edge container, on the internal network —
/// green proving itself along the exact path the world will dial, not just
/// its own loopback.
async fn verify_through_edge() -> bool {
    let host = format!("http://{}/api/healthz", super::layout::edge_container());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build();
    let Ok(client) = client else {
        return false;
    };
    matches!(
        client.get(&host).send().await,
        Ok(resp) if resp.status().is_success()
    )
}

/// Roll back to the other slot — the previous image, still on the host
/// (the keep-window kept its container stopped as exactly this material).
/// The calling process is typically the NEWEST container (the bad one):
/// old comes up healthy, the alias moves, the run records, and only then
/// does this container stop ITSELF — same split as roll, mirrored.
pub async fn rollback(pg: &PgPool, _conn: ConnectionManager) -> Result<String, String> {
    let state = load(pg).await;
    acting_gate(&state, install_mode())?;
    if let Some(run) = &state.last_run
        && run_in_flight(run)
    {
        return Err("an update is in flight — wait for it to land before rolling back".into());
    }
    let me = self_slot().await?;
    let old = other(me);
    let old_container = slot_container(old);
    let old_image = slot_image_ref(old).await;
    let Some(old_image) = old_image else {
        return Err(format!(
            "the other slot has no container to roll back to ({old_container})"
        ));
    };
    let Some(old_digest) = digest_suffix(&old_image).map(str::to_string) else {
        return Err(format!(
            "the other slot's image carries no digest ({old_image})"
        ));
    };

    // Old first, gated on health, exactly like any incoming slot: a
    // rollback that boots a container that cannot answer is not a rollback.
    start_container(&old_container).await?;
    if !wait_healthy_slot(old, HEALTH_GATE_MS).await {
        // Leave it stopped where it was — it was stopped material, and it
        // still is.
        let _ = stop_container(&old_container, 10_000).await;
        return Err("the old container did not come back healthy — nothing moved".into());
    }

    // Alias to old; green (this container) keeps serving until its own stop
    // below — the overlap is two healthy backends on the same DB, the same
    // overlap the roll accepts.
    let spec = slot_spec_from_inspect(&inspect_self().await?)?;
    attach_fleet_alias(&spec.fleet_network, &old_container).await?;

    // The record BEFORE the self-stop, crash-safe like the roll's.
    let at = now_iso();
    let empty = || Pin {
        digest: String::new(),
        version: String::new(),
    };
    let to_pin = Pin {
        digest: old_digest,
        version: state
            .last_run
            .as_ref()
            .map(|r| r.from.version.clone())
            .unwrap_or_default(),
    };
    let from_pin = state.pinned.clone().unwrap_or_else(empty);
    patch(pg, |mut s| {
        record_run(
            &mut s,
            RunRecord {
                state: RunState::RolledBack,
                from: from_pin,
                to: to_pin.clone(),
                by: RunBy::Manual,
                started_at: at.clone(),
                finished_at: Some(at.clone()),
                error: None,
            },
        );
        s.pinned = Some(to_pin);
        s
    })
    .await
    .map_err(|e| format!("the rollback record did not write: {e}"))?;
    tracing::info!("{LOG} rolled back to {old_container}");

    // And this container — the one the run was rolling TO — stops itself.
    // Its slot entry stays in the compose file as the next rollback's
    // material, until tidy removes the container.
    let _ = stop_container(&slot_container(me), roll_drain_ms()).await;
    Ok(format!("rolled back to {old_container}"))
}

/// Retire what the keep-window expired: the stopped old slot's container
/// (rollback material has a shelf life) and images of ours that neither
/// the pin nor the previous slot references. Best-effort by design — a
/// tidy that cannot run today runs tomorrow.
pub async fn tidy(pg: &PgPool) -> Result<Option<String>, String> {
    if install_mode() != InstallMode::Image {
        return Ok(None);
    }
    let state = load(pg).await;
    let Some(run) = state
        .last_run
        .as_ref()
        .filter(|r| matches!(r.state, RunState::Done | RunState::RolledBack))
    else {
        return Ok(None);
    };
    let Some(finished) = run.finished_at.as_deref() else {
        return Ok(None);
    };
    let finished_ms = crate::agent_auth::iso_to_epoch_ms(finished).unwrap_or(i64::MAX);
    if now_ms() - finished_ms < KEEP_WINDOW_MS as i64 {
        return Ok(None);
    }
    // The retired slot is whichever one is not us — it was left stopped at
    // the cutover; if it is running (an operator intervened by hand) tidy
    // does not fight them.
    let me = self_slot().await?;
    let retired = slot_container(other(me));
    let status = docker(
        &["inspect", "-f", "{{.State.Status}}", &retired],
        std::time::Duration::from_secs(10),
    )
    .await
    .map(|(out, _)| out)
    .unwrap_or_else(|_| "gone".into());
    if status.trim() == "exited" {
        remove_container(&retired).await?;
        tracing::info!("{LOG} tidy: retired {retired} past the keep-window");
        return Ok(Some(format!("retired {retired}")));
    }
    Ok(None)
}

/// The compose project this engine owns, for the routes' status read (the
/// panel names the project so an operator ssh-ing in lands on the same
/// one the app would show them).
pub fn project() -> String {
    update_project()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(state: RunState, started_minutes_ago: i64) -> RunRecord {
        RunRecord {
            state,
            from: Pin {
                digest: "sha256:old".into(),
                version: "v1".into(),
            },
            to: Pin {
                digest: "sha256:new".into(),
                version: "v2".into(),
            },
            by: RunBy::Auto,
            started_at: epoch_ms_to_iso(now_ms() - started_minutes_ago * 60_000),
            finished_at: None,
            error: None,
        }
    }

    #[test]
    fn in_flight_is_exactly_the_three_live_states() {
        assert!(run_in_flight(&run(RunState::Pulling, 0)));
        assert!(run_in_flight(&run(RunState::Starting, 0)));
        assert!(run_in_flight(&run(RunState::CuttingOver, 0)));
        assert!(!run_in_flight(&run(RunState::Done, 0)));
        assert!(!run_in_flight(&run(RunState::Failed, 0)));
        assert!(!run_in_flight(&run(RunState::RolledBack, 0)));
    }

    #[test]
    fn the_acting_gate_refuses_unadopted_installs_with_a_sentence() {
        let mut state = UpdateState::default();
        assert!(
            acting_gate(&state, InstallMode::Image)
                .unwrap_err()
                .contains("adopt")
        );
        state.migrated = true;
        assert!(acting_gate(&state, InstallMode::Image).is_ok());
        // The mode refusal outranks the adoption gate — a checkout install
        // that somehow adopted still gets the truer sentence.
        assert!(acting_gate(&state, InstallMode::Checkout).is_err());
        assert!(acting_gate(&state, InstallMode::Off).is_err());
    }

    #[test]
    fn digest_suffix_reads_only_real_digests() {
        let full = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            digest_suffix(&format!("ghcr.io/x/talaria@{full}")),
            Some(full.as_str())
        );
        // A separator alone does not make a digest — is_digest's 64-hex
        // rule applies to the suffix too.
        assert_eq!(digest_suffix("ghcr.io/x/talaria@sha256:aa"), None);
        assert_eq!(digest_suffix("ghcr.io/x/talaria:main"), None);
    }
}
