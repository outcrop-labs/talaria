// THE AGENT IMAGE FOLLOWS HERMES' STABLE CHANNEL.
//
// Each instance keeps its own agents on Hermes' `stable` releases, with no
// Talaria release, pull request or repository change per Hermes release
// (docs/ARCHITECTURE.md, "The fleet"; docs/UPDATES.md). The steps:
//
//   check     every ~6 h, jittered, resolve `stable` to a digest with an
//             anonymous manifest HEAD — not a pull on Docker Hub's limit
//             (check.rs). A new digest is recorded as a candidate, with the
//             `vX.Y.Z` tag that shares it.
//   wait      until stable has named it for 48 h (counted from the first
//             check that saw it there, and started again whenever stable
//             moves), before it is even downloaded: a release Hermes pulls
//             off stable in that time is never rolled out — the waiting
//             rollout gives way to whatever stable names now.
//   prepare   pull it in the background through the agents' shared pull,
//             giving way to any other download and never past the disk's
//             room, and read the image with networking off for its own
//             `_config_version`, the env names its Hermes strips, and its
//             drain tools — and check the Hermes internals Talaria relies
//             on (its config migration leaves Talaria's keys as they are;
//             sealed CONFIG_* credentials stay out of commands), holding a
//             release that changed one for an admin (prepare.rs). Then wait
//             for the maintenance window (02:00–05:00 in the workspace's
//             time zone), or Roll now.
//   canary    boot it as a throwaway agent with a rendered config and nothing
//             to reach, ask it only non-model questions, and keep it up and
//             ask again for a soak (probe.rs).
//   roll      agents one at a time through the ordinary in-place roll,
//             passing over one that is working or about to (busy.rs), each
//             watched for a soak before the next moves, every moved agent
//             asked again before every move, then the whole rollout soaked
//             before the image is called good (engine.rs).
//   rollback  a failure the image confirms (twice) puts every agent it
//             reached back on the last known good, marks it bad until an
//             admin allows it again, and notifies every admin. A failure that
//             is the host's is tried again later and never bans.
//
// AN AGENT ALWAYS RUNS A RESOLVED DIGEST. The render names each agent's
// image by digest from the records (talaria-hermes-images), never `stable`.
// The compiled pin is the floor: a fresh install, an upgraded host before its
// first rollout, a registry that cannot be reached — all render the pin.
//
// THE OPERATOR'S CHOICE WINS. `HERMES_IMAGE` (in the api's env or
// fleet/.env), or a chassis whose image is not a stock default, turns the
// channel off; so does an admin's Pin, which holds every agent on one
// version until it is lifted.
//
// One step at a time across every api instance (a Postgres advisory lock,
// asked again before every write), a step a minute from the scheduler — the
// only runner. The admin actions do not run a step: they change the state
// row under its row lock and the next step (within a minute) carries them
// out; every write a step makes names its rollout, so a step that finds its
// rollout replaced stops and re-reads (engine.rs).

pub mod busy;
pub mod check;
pub mod engine;
pub mod ops;
pub mod prepare;
pub mod probe;

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use futures_util::FutureExt;
use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_hermes_images::{
    self as images, ChannelState, ImageRecord, Phase, Rollout, RolloutKind, StateError, Status,
};
use talaria_notify::NotifyDeps;
use talaria_scheduler::{JobName, JobSpec};
use talaria_secretbox::SecretBox;

pub use check::{STABLE_REF_DEFAULT, stable_ref};
pub use engine::{Engine, Tuning, backoff, watching};
pub use ops::{ChannelAgent, DockerOps, Fault, FleetOps, Snapshot, channel_agents, on_channel};

/// How often the channel is checked, before jitter.
pub const CHECK_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// Each check lands up to this far either side of its six hours, so a
/// release does not see every instance at once.
pub(crate) const CHECK_JITTER_MS: i64 = 30 * 60 * 1000;
/// A host with no check on record checks within this long of boot.
pub(crate) const FIRST_CHECK_WITHIN_MS: i64 = 10 * 60 * 1000;
/// The scheduler's beat for the step.
pub const STEP_EVERY_MS: u64 = 60_000;
/// The last known good stays on the host this long after the current image
/// became good, then is pruned.
pub const KEEP_PREVIOUS_MS: i64 = 3 * 24 * 60 * 60 * 1000;
/// Prune at most this often.
const PRUNE_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// A candidate no rollout names is pruned once it is this old.
pub const ABANDONED_AFTER_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Where the notification opens: the Manage overview carries the panel.
pub(crate) const HREF: &str = "/manage";
const LOG: &str = "[hermes]";

/// A rollout the channel stopped (talaria-notify routes both kinds to the
/// admins' workspace-health class).
pub const KIND_UPDATE_FAILED: &str = "hermes_update_failed";
/// The channel cannot do its job: checks failing for days, a rollout open
/// for a week, its state row unreadable.
pub const KIND_CHANNEL_STUCK: &str = "hermes_channel_stuck";

/// A number from the environment (seconds, or hours for the minimum age),
/// or `default`.
pub fn env_secs(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// A uniformly random offset in `lo..hi` ms.
pub(crate) fn jitter(lo: i64, hi: i64) -> i64 {
    let span = (hi - lo).max(1) as u128;
    lo + (uuid::Uuid::new_v4().as_u128() % span) as i64
}

/// The compiled pin's digest, version and repository.
pub fn compiled_pin() -> (String, String, String) {
    let (digest, tag, repo) = talaria_fleet_render::compiled_pin_parts();
    (digest.into(), tag.into(), repo.into())
}

/// Why the channel stands aside for the operator's own image, or None.
pub async fn off_reason() -> Option<String> {
    let chassis = match talaria_fleet_render::read_chassis().await {
        Ok(c) => c,
        Err(e) => return Some(e),
    };
    let image = chassis
        .service
        .get("image")
        .and_then(Value::as_str)
        .map(str::to_string);
    if !talaria_fleet_render::is_stock_image(image.as_deref()) {
        return Some(format!(
            "the fleet chassis names its own agent image ({}), so it stays the operator's",
            image.unwrap_or_else(|| "none".into())
        ));
    }
    images::operator_image()
        .await
        .map(|mine| format!("HERMES_IMAGE is set ({mine:.120}), so every agent runs that image"))
}

/// Record the compiled pin as known good (the insert is a no-op after the
/// first).
async fn seed_compiled(pg: &PgPool) -> Result<(), String> {
    let (digest, tag, repo) = compiled_pin();
    images::seed_known_good(
        pg,
        &digest,
        &repo,
        &tag,
        talaria_fleet_render::HERMES_CONFIG_VERSION,
        "the pin this release of Talaria was checked against",
    )
    .await
    .map_err(|e| format!("the compiled pin's record could not be written: {e}"))
}

/// What a step needs.
#[derive(Clone)]
pub struct ChannelDeps {
    pub pg: PgPool,
    pub sb: SecretBox,
    pub notify: NotifyDeps,
}

impl ChannelDeps {
    fn ops(&self) -> DockerOps {
        DockerOps {
            pg: self.pg.clone(),
            sb: self.sb.clone(),
            notify: self.notify.clone(),
        }
    }
}

// ── The lock and the step ───────────────────────────────────────────────────

/// The step's session-level advisory lock, held on its own connection. The
/// key follows the state row's, so a test's engine never contends with the
/// channel's. Asked again before every write the step makes ([`Fence::held`]):
/// a lock that went with its connection (a database restart, a failover, a
/// terminated backend) while this process lives would let another step in,
/// so this one stops before it moves anything else.
pub struct Fence {
    conn: tokio::sync::Mutex<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    key: String,
}

impl Fence {
    async fn take(pg: &PgPool, state_key: &str) -> Result<Option<Fence>, String> {
        let key = format!("talaria:hermes-channel:{state_key}");
        let mut conn = pg
            .acquire()
            .await
            .map_err(|e| format!("no database connection for the channel lock: {e}"))?;
        let (locked,): (bool,) = sqlx::query_as("select pg_try_advisory_lock(hashtext($1))")
            .bind(&key)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| format!("the channel lock could not be asked for: {e}"))?;
        Ok(locked.then(|| Fence {
            conn: tokio::sync::Mutex::new(conn),
            key,
        }))
    }

    /// Whether this connection still holds the lock.
    pub async fn held(&self) -> bool {
        let mut conn = self.conn.lock().await;
        sqlx::query_scalar::<_, bool>(
            "select exists (select 1 from pg_locks where locktype = 'advisory' \
               and pid = pg_backend_pid() and granted)",
        )
        .fetch_one(&mut **conn)
        .await
        .unwrap_or(false)
    }

    async fn release(self) {
        let mut conn = self.conn.into_inner();
        let unlocked = sqlx::query("select pg_advisory_unlock(hashtext($1))")
            .bind(&self.key)
            .execute(&mut *conn)
            .await;
        if let Err(e) = unlocked {
            tracing::warn!("{LOG} the channel lock could not be released: {e}");
            let _ = conn.close().await;
        }
    }
}

/// One step against any fleet, under the lock for `key`'s state row:
/// `Ok(None)` when another step holds the lock.
pub async fn step_with(
    pg: &PgPool,
    key: &str,
    ops: &dyn FleetOps,
    tuning: &Tuning,
) -> Result<Option<Vec<String>>, String> {
    let Some(fence) = Fence::take(pg, key).await? else {
        return Ok(None);
    };
    let engine = Engine {
        pg,
        key,
        ops,
        tuning,
        fence: Some(&fence),
    };
    let ran = AssertUnwindSafe(async {
        seed_compiled(pg).await?;
        let mut said = engine.step().await?;
        if key == images::STATE_KEY
            && let Some(p) = prune_if_due(&engine).await
        {
            said.push(p);
        }
        Ok::<Vec<String>, String>(said)
    })
    .catch_unwind()
    .await;
    fence.release().await;
    match ran {
        Ok(r) => r.map(Some),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// The last time this process told the admins the state row is unreadable.
static UNREADABLE_ALERTED: AtomicI64 = AtomicI64::new(0);

/// One step of the channel on the real fleet. Answers a sentence for the
/// scheduler's log, or None when there was nothing to do (or another step
/// holds the lock). A state row that cannot be read is said to every admin
/// (at most every six hours from one process) and nothing moves.
pub async fn step(deps: &ChannelDeps) -> Result<Option<String>, String> {
    let ops = deps.ops();
    let tuning = Tuning::from_env();
    match step_with(&deps.pg, images::STATE_KEY, &ops, &tuning).await {
        Ok(said) => Ok(said.filter(|s| !s.is_empty()).map(|s| s.join("; "))),
        Err(e) => {
            if let Err(StateError::Unreadable(_)) = images::load_state(&deps.pg).await {
                let now = talaria_agent_auth::now_ms();
                if now - UNREADABLE_ALERTED.load(Ordering::Relaxed) > 6 * 60 * 60 * 1000 {
                    UNREADABLE_ALERTED.store(now, Ordering::Relaxed);
                    ops.notify(
                        KIND_CHANNEL_STUCK,
                        "state-unreadable",
                        "Agent image updates are paused: their state is unreadable",
                        &format!(
                            "{e}. Agents keep running on their images, and hiring, starting and editing agents all work; only automatic image updates wait. To resume them, press Reset on Manage → Agent image updates: it sets the unreadable state aside (kept as it was) and keeps every agent on the image it runs. To hold every agent on one image instead, set HERMES_IMAGE in fleet/.env."
                        ),
                    )
                    .await;
                }
            }
            Err(e)
        }
    }
}

// ── Pruning ─────────────────────────────────────────────────────────────────

static PRUNED_AT: AtomicI64 = AtomicI64::new(0);

/// The images to remove: every one this instance pulled but the compiled
/// pin, the current image, the pinned one, any agent's, an open rollout's
/// two, the previous image until the current one has been good for
/// [`KEEP_PREVIOUS_MS`] — and a candidate only once it is
/// [`ABANDONED_AFTER_MS`] old (one a Pin replaced, a release that left
/// stable). An image this instance did not pull is never touched.
pub fn prune_plan(
    state: &ChannelState,
    records: &[ImageRecord],
    assigned: &HashMap<String, String>,
    compiled: &str,
    now: i64,
) -> Vec<String> {
    let current_good_since = state
        .current
        .as_ref()
        .and_then(|c| records.iter().find(|r| &r.digest == c))
        .and_then(|r| r.good_since);
    let keep_previous = current_good_since.is_none_or(|t| now - t < KEEP_PREVIOUS_MS);
    let mut keep: Vec<&str> = vec![compiled];
    keep.extend(state.current.as_deref());
    keep.extend(state.pinned.as_deref());
    if let Some(r) = state.rollout.as_ref().filter(|r| r.open()) {
        keep.push(&r.digest);
        keep.extend(r.from.as_deref());
    }
    if keep_previous {
        keep.extend(state.previous.as_deref());
    }
    keep.extend(assigned.values().map(String::as_str));
    records
        .iter()
        .filter(|r| r.pulled_here && !keep.contains(&r.digest.as_str()))
        .filter(|r| r.status != Status::Candidate || now - r.first_seen > ABANDONED_AFTER_MS)
        .map(|r| r.digest.clone())
        .collect()
}

/// Prune what [`prune_plan`] names, at most every six hours, never while a
/// rollout is open and never on a read that failed (an assignment it could
/// not read is one it would prune). Docker refuses an image a container
/// still uses, and it is kept.
async fn prune_if_due(engine: &Engine<'_>) -> Option<String> {
    let now = engine.ops.now_ms();
    if now - PRUNED_AT.load(Ordering::Relaxed) < PRUNE_EVERY_MS {
        return None;
    }
    let state = engine.load().await.ok()?;
    if !prune_may_run(&state) {
        return None;
    }
    PRUNED_AT.store(now, Ordering::Relaxed);
    prune(engine, &state).await
}

/// Whether the prune may run now: not while agents move. A rollout still
/// preparing or waiting (one held for disk room among them) is no reason not
/// to make room — the plan keeps its image and the one it leaves.
fn prune_may_run(state: &ChannelState) -> bool {
    !state
        .rollout
        .as_ref()
        .is_some_and(|r| r.open() && !matches!(r.phase, Phase::Preparing | Phase::Waiting))
}

/// Remove [`prune_plan`]'s images now: None when there was nothing to
/// remove, or a read failed.
pub async fn prune(engine: &Engine<'_>, state: &ChannelState) -> Option<String> {
    let pg = engine.pg;
    let now = engine.ops.now_ms();
    let records = images::records(pg).await.ok()?;
    let assigned = images::assignments(pg).await.ok()?;
    let mut pruned = Vec::new();
    for digest in prune_plan(state, &records, &assigned, &compiled_pin().0, now) {
        let Some(r) = records.iter().find(|r| r.digest == digest) else {
            continue;
        };
        match engine.ops.remove_image(&r.image_ref()).await {
            Ok(()) => {
                let _ = images::note_pruned(pg, &r.digest).await;
                pruned.push(r.label());
            }
            Err(e) => tracing::info!("{LOG} {} kept: {e}", r.label()),
        }
    }
    if pruned.is_empty() {
        return None;
    }
    let text = format!("pruned {}", pruned.join(", "));
    let note = text.clone();
    let _ = engine
        .patch(move |s| {
            s.note(now, note);
            Ok(())
        })
        .await;
    Some(text)
}

// ── Admin actions ───────────────────────────────────────────────────────────
//
// Each is one transactional write of the state row (its row lock serializes
// it with the step's writes), with what belongs with it — a ban, an allow —
// in the same transaction. None runs a step: the scheduler's next one, within
// a minute, carries it out, and a step already running stops at its next
// write, because every write the step makes names its rollout and is refused
// once it is replaced.

fn label_or(record: Option<&ImageRecord>, digest: &str) -> String {
    record
        .map(|r| r.label())
        .unwrap_or_else(|| images::label(None, digest))
}

/// The message of a refusal raised inside a state patch.
fn refusal(e: StateError) -> String {
    match e {
        StateError::Store(m) => m,
        other => other.to_string(),
    }
}

/// Check `stable` now.
pub async fn check_now_with(e: &Engine<'_>) -> Result<images::CheckRecord, String> {
    seed_compiled(e.pg).await?;
    e.run_check().await.map_err(|x| x.to_string())?;
    Ok(e.load()
        .await
        .map_err(|x| x.to_string())?
        .last_check
        .unwrap_or_default())
}

/// Run the rollout now — outside the maintenance window, and before its
/// minimum time on stable: the open one, or the one the channel calls for.
pub async fn roll_now_with(e: &Engine<'_>) -> Result<String, String> {
    seed_compiled(e.pg).await?;
    if let Some(why) = e.ops.off_reason().await {
        return Err(why);
    }
    let state = e.load().await.map_err(|x| x.to_string())?;
    if !state.catchup_failed.is_empty() {
        // An admin acting is the signal to try left-behind agents again.
        e.patch(|s| {
            s.catchup_failed.clear();
            Ok(())
        })
        .await
        .map_err(|x| x.to_string())?;
    }
    if !state.rollout.as_ref().is_some_and(Rollout::open) {
        e.open_rollout().await?;
    }
    let (state, forced) = e
        .patch(|s| {
            Ok(match s.rollout.as_mut().filter(|r| r.open()) {
                Some(r) => {
                    r.forced = true;
                    r.retry_at = None;
                    // A person is watching: the host gets its first chance
                    // again, and a pin stopped at an unhealthy agent goes on.
                    r.infra_failures = 0;
                    r.halted = None;
                    true
                }
                None => false,
            })
        })
        .await
        .map_err(|x| x.to_string())?;
    let Some(r) = state.rollout.filter(|r| forced && r.open()) else {
        return Err("there is nothing to roll out: every agent runs what the channel names".into());
    };
    let rec = images::record(e.pg, &r.digest).await.ok().flatten();
    Ok(format!(
        "rolling out {} now: the next step, within a minute, moves it",
        label_or(rec.as_ref(), &r.digest)
    ))
}

/// Roll back. Every agent on the version being left goes back now, mid-turn
/// included — a person has said that version is bad.
/// - An open Upgrade is abandoned: its image is marked bad (an admin can
///   allow it again) and every agent on it moves back.
/// - An open Pin is cancelled: nothing is marked bad, the pin is lifted, and
///   its agents go back to what the fleet ran.
/// - With nothing open (or a catch-up), the fleet moves to the previous
///   version, which becomes current at once (a hire meanwhile renders it):
///   the version it leaves is marked bad if it came from stable (so stable
///   does not roll it forward again), and if a Pin put it there, the pin
///   moves to the previous version instead — a known-good version is never
///   banned for being pinned away from.
///
/// The ban is written with the rollback, in one transaction.
pub async fn roll_back_with(e: &Engine<'_>, by: &str) -> Result<String, String> {
    seed_compiled(e.pg).await?;
    let now = e.ops.now_ms();
    let compiled = compiled_pin().0;
    let who = by.to_string();
    // The agents that follow `current` with no assignment of their own: when
    // the rollback moves `current` back, they are assigned the version being
    // left in the same write, so the rollback moves them too (a hire after
    // this read follows the new `current`, which is what it should run).
    let assigned = images::assignments(e.pg)
        .await
        .map_err(|x| format!("the agents' images could not be read: {x}"))?;
    let following: Vec<String> = e
        .ops
        .agents()
        .await?
        .into_iter()
        .filter(|a| on_channel(a) && !assigned.contains_key(&a.id))
        .map(|a| a.id)
        .collect();
    let (_, (ban, target, left)) = e
        .patch_fx(move |s, fx| {
            let current = s.current.clone().unwrap_or_else(|| compiled.clone());
            let open = s
                .rollout
                .clone()
                .filter(|r| r.open() && r.kind != RolloutKind::Catchup);
            let (ban, target, left) = match open {
                Some(r) if r.kind == RolloutKind::Rollback => {
                    return Err(StateError::Store(
                        "a rollback is already in progress".into(),
                    ));
                }
                Some(r) => {
                    let target = r.from.clone().unwrap_or_else(|| current.clone());
                    if r.kind == RolloutKind::Pin {
                        s.pinned = None;
                    }
                    (r.kind == RolloutKind::Upgrade, target, r.digest.clone())
                }
                None => {
                    let target = s.previous.clone().unwrap_or_else(|| compiled.clone());
                    if current == target {
                        return Err(StateError::Store(
                            "there is no earlier version to go back to: the fleet runs this release's own pin".into(),
                        ));
                    }
                    let ban = if s.pinned.as_deref() == Some(current.as_str()) {
                        s.pinned = Some(target.clone());
                        false
                    } else {
                        true
                    };
                    for agent in &following {
                        fx.assign(agent, &current);
                    }
                    s.current = Some(target.clone());
                    s.previous = None;
                    (ban, target, current)
                }
            };
            if ban {
                fx.mark_bad(&left, &format!("rolled back by {who}"));
            }
            s.rollout = Some(Rollout {
                id: images::new_rollout_id(now),
                digest: target.clone(),
                from: Some(left.clone()),
                kind: RolloutKind::Rollback,
                phase: Phase::Preparing,
                started_at: now,
                updated_at: now,
                forced: true,
                detail: Some(format!("rolled back by {who}")),
                ..Default::default()
            });
            Ok((ban, target, left))
        })
        .await
        .map_err(refusal)?;
    let left_rec = images::record(e.pg, &left).await.ok().flatten();
    let target_rec = images::record(e.pg, &target).await.ok().flatten();
    let mut text = format!(
        "moving every agent from {} to {} on the next step, within a minute",
        label_or(left_rec.as_ref(), &left),
        label_or(target_rec.as_ref(), &target)
    );
    if ban {
        text.push_str(&format!(
            "; {} is marked bad until an admin allows it again",
            label_or(left_rec.as_ref(), &left)
        ));
    }
    let note = text.clone();
    let _ = e
        .patch(move |s| {
            s.note(now, note);
            Ok(())
        })
        .await;
    Ok(text)
}

/// Pin every agent to a version (a digest on record, or a `vX.Y.Z` tag
/// resolved now): any version not marked bad. One that never ran here is
/// prepared and canaried first; a cancelled rollout's agents come back with
/// the rest (every agent not on the pinned version is moved; one a person
/// stopped is reassigned, never started). Neither a digest nor a version
/// lifts the pin.
pub async fn pin_with(
    e: &Engine<'_>,
    digest: Option<&str>,
    version: Option<&str>,
    by: &str,
) -> Result<String, String> {
    seed_compiled(e.pg).await?;
    let now = e.ops.now_ms();
    let digest = match (digest, version) {
        (Some(d), _) => {
            if !images::is_digest(d) {
                return Err("that is not an image digest".into());
            }
            d.to_string()
        }
        (None, Some(v)) => e.ops.resolve(v).await?.digest,
        (None, None) => {
            let note = format!("{by} lifted the pin: following stable");
            e.patch(move |s| {
                s.pinned = None;
                // The move onto the pin stops with it (a halted one would
                // otherwise hold the channel open): the agents it moved stay
                // where they are, and stable's next rollout takes them on.
                if let Some(r) = s
                    .rollout
                    .as_mut()
                    .filter(|r| r.open() && r.kind == RolloutKind::Pin && r.agent.is_none())
                {
                    r.phase = Phase::Cancelled;
                    r.detail = Some(note.clone());
                    r.updated_at = now;
                }
                s.note(now, note);
                Ok(())
            })
            .await
            .map_err(|x| x.to_string())?;
            return Ok("following stable again".into());
        }
    };
    let record = images::record(e.pg, &digest)
        .await
        .map_err(|x| format!("the image record could not be read: {x}"))?
        .ok_or_else(|| "this instance has no record of that image".to_string())?;
    if record.status == Status::Bad {
        return Err(format!(
            "{} is marked bad here ({}): allow it again first",
            record.label(),
            record.reason.clone().unwrap_or_default()
        ));
    }
    let label = record.label();
    let note = format!("{by} pinned agents to {label}");
    let compiled = compiled_pin().0;
    e.patch(move |s| {
        s.pinned = Some(digest.clone());
        let current = s.current.clone().unwrap_or(compiled);
        let open = s.rollout.clone().filter(|r| r.open());
        // A Pin to this digest already running stands; anything else gives
        // way, and the agents it reached are brought along.
        if open
            .as_ref()
            .is_none_or(|r| !(r.kind == RolloutKind::Pin && r.digest == digest))
        {
            s.rollout = Some(Rollout {
                id: images::new_rollout_id(now),
                digest: digest.clone(),
                from: Some(current),
                kind: RolloutKind::Pin,
                phase: Phase::Preparing,
                started_at: now,
                updated_at: now,
                forced: true,
                ..Default::default()
            });
        }
        s.note(now, note);
        Ok(())
    })
    .await
    .map_err(|x| x.to_string())?;
    Ok(format!("pinned to {label}"))
}

/// Lift a ban: the version may be rolled out (or pinned) again. The lift
/// and its note are one write: an unreadable state refuses both.
pub async fn allow_with(e: &Engine<'_>, digest: &str, by: &str) -> Result<String, String> {
    if !images::is_digest(digest) {
        return Err("that is not an image digest".into());
    }
    let record = images::record(e.pg, digest)
        .await
        .map_err(|x| format!("the image record could not be read: {x}"))?
        .ok_or_else(|| "this instance has no record of that image".to_string())?;
    if record.status != Status::Bad {
        return Err(format!("{} is not marked bad", record.label()));
    }
    let now = e.ops.now_ms();
    let text = format!("{by} allowed {} again", record.label());
    let note = text.clone();
    let d = digest.to_string();
    e.patch_fx(move |s, fx| {
        fx.allow_again(&d);
        s.note(now, note);
        Ok(())
    })
    .await
    .map_err(|x| x.to_string())?;
    Ok(text)
}

/// Accept a release the contract checks held: it rolls out anyway (the
/// admin has read what changed).
pub async fn accept_with(e: &Engine<'_>, digest: &str, by: &str) -> Result<String, String> {
    if !images::is_digest(digest) {
        return Err("that is not an image digest".into());
    }
    let record = images::record(e.pg, digest)
        .await
        .map_err(|x| format!("the image record could not be read: {x}"))?
        .ok_or_else(|| "this instance has no record of that image".to_string())?;
    let now = e.ops.now_ms();
    // A state this build cannot read accepts nothing either.
    e.load().await.map_err(|x| x.to_string())?;
    if !images::accept_contract(e.pg, digest, by, now)
        .await
        .map_err(|x| format!("the acceptance could not be recorded: {x}"))?
    {
        return Err(format!("{} is not held", record.label()));
    }
    let text = format!(
        "{by} accepted {} despite: {}",
        record.label(),
        record.contract_problems().join("; ")
    );
    let note = text.clone();
    e.patch(move |s| {
        s.note(now, note);
        Ok(())
    })
    .await
    .map_err(|x| x.to_string())?;
    Ok(text)
}

/// Set an unreadable state aside (kept, exactly as it was) and start a
/// fresh one on the image the fleet runs.
pub async fn reset_with(e: &Engine<'_>, by: &str) -> Result<String, String> {
    let archive = images::reset_state_at(e.pg, e.key, e.ops.now_ms(), by)
        .await
        .map_err(refusal)?;
    Ok(format!(
        "the unreadable state is set aside as app_settings '{archive}'; agents stay on their images and updates resume"
    ))
}

/// Set the maintenance window.
pub async fn set_window_with(
    e: &Engine<'_>,
    start: &str,
    end: &str,
) -> Result<images::Window, String> {
    let (Some(s), Some(f)) = (images::clock_minutes(start), images::clock_minutes(end)) else {
        return Err("the window's ends are HH:MM times".into());
    };
    if s == f {
        return Err("the window cannot start and end at the same minute".into());
    }
    let window = images::Window {
        start: start.trim().into(),
        end: end.trim().into(),
        ..Default::default()
    };
    let set = window.clone();
    e.patch(move |st| {
        st.window = set;
        Ok(())
    })
    .await
    .map_err(|x| x.to_string())?;
    Ok(window)
}

// The same actions on the real fleet, each followed by a kick.

macro_rules! on_real_fleet {
    ($deps:expr, |$e:ident| $body:expr) => {{
        let ops = $deps.ops();
        let tuning = Tuning::from_env();
        let $e = Engine {
            pg: &$deps.pg,
            key: images::STATE_KEY,
            ops: &ops,
            tuning: &tuning,
            fence: None,
        };
        $body
    }};
}

pub async fn check_now(deps: &ChannelDeps) -> Result<images::CheckRecord, String> {
    on_real_fleet!(deps, |e| check_now_with(&e).await)
}

pub async fn roll_now(deps: &ChannelDeps) -> Result<String, String> {
    on_real_fleet!(deps, |e| roll_now_with(&e).await)
}

pub async fn roll_back(deps: &ChannelDeps, by: &str) -> Result<String, String> {
    on_real_fleet!(deps, |e| roll_back_with(&e, by).await)
}

pub async fn pin(
    deps: &ChannelDeps,
    digest: Option<&str>,
    version: Option<&str>,
    by: &str,
) -> Result<String, String> {
    on_real_fleet!(deps, |e| pin_with(&e, digest, version, by).await)
}

pub async fn allow(deps: &ChannelDeps, digest: &str, by: &str) -> Result<String, String> {
    on_real_fleet!(deps, |e| allow_with(&e, digest, by).await)
}

pub async fn accept(deps: &ChannelDeps, digest: &str, by: &str) -> Result<String, String> {
    on_real_fleet!(deps, |e| accept_with(&e, digest, by).await)
}

pub async fn reset(deps: &ChannelDeps, by: &str) -> Result<String, String> {
    on_real_fleet!(deps, |e| reset_with(&e, by).await)
}

pub async fn set_window(
    deps: &ChannelDeps,
    start: &str,
    end: &str,
) -> Result<images::Window, String> {
    on_real_fleet!(deps, |e| set_window_with(&e, start, end).await)
}

// ── The window's time zone ──────────────────────────────────────────────────

/// Where the maintenance window's zone came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneSource {
    /// The workspace's own (the daily brief's time zone, in Settings).
    Workspace,
    /// The api's `TZ`.
    Server,
    /// The zone most admins set on their profiles.
    Admins,
    /// Nothing set anywhere: UTC.
    Unset,
}

/// The window's zone and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowZone {
    pub zone: String,
    pub source: ZoneSource,
}

impl WindowZone {
    /// The zone as the panel and the Waiting detail say it.
    pub fn said(&self) -> String {
        match self.source {
            ZoneSource::Workspace => self.zone.clone(),
            ZoneSource::Server => format!("{} (the server's TZ)", self.zone),
            ZoneSource::Admins => format!("{} (the admins' profile zone)", self.zone),
            ZoneSource::Unset => {
                "UTC — no time zone is set; set the workspace's in Settings".into()
            }
        }
    }
}

/// The zone, from what is set: the workspace's, else the server's `TZ`, else
/// the zone most admins chose for themselves (a tie: the oldest admin's),
/// else UTC — and the panel says which, so a window that lands in someone's
/// evening is visible.
pub fn pick_zone(workspace: Option<&str>, tz_env: Option<&str>, admins: &[String]) -> WindowZone {
    let set = |z: Option<&str>| {
        z.map(str::trim)
            .filter(|z| !z.is_empty())
            .map(str::to_string)
    };
    if let Some(zone) = set(workspace) {
        return WindowZone {
            zone,
            source: ZoneSource::Workspace,
        };
    }
    if let Some(zone) = set(tz_env) {
        return WindowZone {
            zone,
            source: ZoneSource::Server,
        };
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    for z in admins.iter().filter_map(|z| set(Some(z))) {
        match counts.iter_mut().find(|(k, _)| *k == z) {
            Some((_, n)) => *n += 1,
            None => counts.push((z, 1)),
        }
    }
    let mut best: Option<&(String, usize)> = None;
    for c in &counts {
        if best.is_none_or(|b| c.1 > b.1) {
            best = Some(c);
        }
    }
    match best {
        Some((zone, _)) => WindowZone {
            zone: zone.clone(),
            source: ZoneSource::Admins,
        },
        None => WindowZone {
            zone: "UTC".into(),
            source: ZoneSource::Unset,
        },
    }
}

/// The maintenance window's zone on this instance.
pub async fn window_zone(pg: &PgPool) -> WindowZone {
    let workspace: Option<String> =
        sqlx::query_scalar("select value->>'timeZone' from app_settings where key = $1")
            .bind(talaria_daily_brief_config::BRIEF_CONFIG_KEY)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten()
            .flatten();
    let admins: Vec<String> = sqlx::query_scalar(
        "select timezone from users where role = 'admin' and coalesce(trim(timezone), '') <> '' \
         order by created_at, id",
    )
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    pick_zone(
        workspace.as_deref(),
        std::env::var("TZ").ok().as_deref(),
        &admins,
    )
}

/// Whether the channel has adopted the fleet as it found it (each agent's
/// running image and its stop), or stands aside — so the deploy's fleet roll
/// recreates every agent on the Hermes it runs, never moving it: the channel
/// is the one thing that changes an agent's Hermes. An unreadable state
/// answers no (the roll then waits out its bound).
pub async fn fleet_adopted(pg: &PgPool) -> bool {
    if off_reason().await.is_some() {
        return true;
    }
    images::load_state(pg).await.is_ok_and(|s| {
        s.extra
            .get(engine::ADOPTED_KEY)
            .is_some_and(|v| v.is_number())
    })
}

/// How long the deploy's fleet roll waits for the channel to adopt the
/// fleet before it goes ahead anyway (a channel that cannot run must not
/// hold a Talaria update's roll forever).
pub const DEPLOY_ROLL_WAIT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Whether the deploy's fleet roll may go now: once the fleet is adopted (or
/// the channel stands aside), or after [`DEPLOY_ROLL_WAIT`] since boot.
pub fn deploy_roll_may_go(adopted: bool, since_boot: std::time::Duration) -> bool {
    adopted || since_boot >= DEPLOY_ROLL_WAIT
}

/// At boot: the env names the good images strip are in force before the
/// first render, so a secret saved before it is judged by them too.
pub async fn boot(pg: &PgPool) {
    match images::stripped_env_of_good(pg).await {
        Ok(names) => {
            talaria_agent_secrets::set_image_stripped(names);
        }
        Err(e) => tracing::warn!(
            "{LOG} the good images' stripped env could not be read at boot ({e}); the first render reads it"
        ),
    }
}

// ── What the panel reads ────────────────────────────────────────────────────

fn record_json(r: &ImageRecord) -> Value {
    json!({
        "digest": r.digest,
        "version": r.version,
        "label": r.label(),
        "configVersion": r.config_version,
        "status": r.status.as_str(),
        "reason": r.reason,
        "firstSeen": r.first_seen,
        "preparedAt": r.prepared_at,
        "goodSince": r.good_since,
        "canaryOkAt": r.canary_ok_at(),
        "pinnable": r.pinnable(),
        "held": r.held(),
        "contractProblems": r.contract_problems(),
        "contractAccepted": r.contract_accepted().map(|(by, at)| json!({ "by": by, "at": at })),
    })
}

/// Everything the admin panel shows. Reads only: a state row it cannot read
/// is the answer's error, never a default.
pub async fn status(pg: &PgPool) -> Result<Value, String> {
    let state: ChannelState = images::load_state(pg).await.map_err(|e| e.to_string())?;
    let off = off_reason().await;
    let records = images::records(pg)
        .await
        .map_err(|e| format!("the image records could not be read: {e}"))?;
    let assigned = images::assignments(pg)
        .await
        .map_err(|e| format!("the agents' images could not be read: {e}"))?;
    let agents = channel_agents(pg)
        .await
        .map_err(|e| format!("the agents could not be read: {e}"))?;
    let (pin_digest, pin_tag, _) = compiled_pin();
    let label_of = |d: &str| {
        records
            .iter()
            .find(|r| r.digest == d)
            .map(|r| r.label())
            .unwrap_or_else(|| {
                if d == pin_digest {
                    pin_tag.clone()
                } else {
                    images::label(None, d)
                }
            })
    };
    let current = state.current.clone().unwrap_or_else(|| pin_digest.clone());
    let zone = window_zone(pg).await;
    let m = talaria_tz::local_moment(&zone.zone, talaria_agent_auth::now_ms());
    let open = state.window.contains(m.hour * 60 + m.minute);
    let standing = match (&off, &state.pinned) {
        (Some(_), _) => "off",
        (None, Some(_)) => "pinned",
        (None, None) => "following",
    };
    let agents_json: Vec<Value> = agents
        .iter()
        .map(|a| {
            let digest = assigned
                .get(&a.id)
                .cloned()
                .unwrap_or_else(|| current.clone());
            let image = if off.is_some() || !on_channel(a) {
                Value::Null
            } else {
                json!({ "digest": digest, "label": label_of(&digest) })
            };
            json!({
                "slug": a.slug,
                "displayName": a.display_name,
                "department": a.department,
                "image": image,
                "onChannel": on_channel(a),
            })
        })
        .collect();
    let rollout = state.rollout.as_ref().map(|r| {
        json!({
            "id": r.id,
            "digest": r.digest,
            "label": label_of(&r.digest),
            "from": r.from,
            "fromLabel": r.from.as_deref().map(label_of),
            "kind": r.kind,
            "phase": r.phase,
            "open": r.open(),
            "forced": r.forced,
            "startedAt": r.started_at,
            "updatedAt": r.updated_at,
            "canaryOkAt": r.canary_ok_at,
            "agent": r.agent,
            "moved": r.moved,
            "skipped": r.skipped,
            "deferred": r.deferred,
            "strikes": r.strikes,
            "strikeOn": r.strike_on,
            "hostFailures": r.infra_failures,
            "retryAt": r.retry_at,
            "soakUntil": r.soak_until,
            "detail": r.detail,
        })
    });
    let tuning = Tuning::from_env();
    Ok(json!({
        "standing": standing,
        "offReason": off,
        "channel": stable_ref(),
        "pinned": state.pinned.as_deref().map(|d| json!({ "digest": d, "label": label_of(d) })),
        "current": { "digest": current, "label": label_of(&current) },
        "previous": state.previous.as_deref().map(|d| json!({ "digest": d, "label": label_of(d) })),
        "compiled": { "digest": pin_digest, "label": pin_tag },
        "lastCheck": state.last_check.as_ref().map(|c| json!({
            "at": c.at,
            "digest": c.digest,
            "label": c.digest.as_deref().map(|d| images::label(c.version.as_deref(), d)),
            "error": c.error,
        })),
        "nextCheckAt": state.next_check_at,
        "window": {
            "start": state.window.start,
            "end": state.window.end,
            "zone": m.zone,
            "zoneSaid": zone.said(),
            "zoneSet": zone.source != ZoneSource::Unset,
            "open": open,
        },
        "minStableHours": tuning.min_stable_ms / 3_600_000,
        "rollout": rollout,
        "agents": agents_json,
        "images": records.iter().map(record_json).collect::<Vec<_>>(),
        "events": state.events,
    }))
}

// ── The scheduler job ───────────────────────────────────────────────────────

/// The step on the scheduler's minute beat. Optional (not a boot-required
/// job): its absence leaves agents on the image they run.
pub fn job_spec(deps: Arc<ChannelDeps>) -> JobSpec {
    JobSpec {
        name: JobName::HermesChannel,
        every_ms: STEP_EVERY_MS,
        first_run_delay_ms: Some(90_000),
        // A step can hold a canary and its soak (up to ~25 min) or roll
        // agents for its budget plus one agent's late health wait.
        max_run_ms: Some(45 * 60_000),
        per_instance: false,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move { step(&deps).await })
        }),
    }
}

/// Register the job.
pub fn register_job(deps: Arc<ChannelDeps>) {
    talaria_scheduler::register_job(job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_lands_within_its_jitter() {
        for _ in 0..200 {
            let j = jitter(-CHECK_JITTER_MS, CHECK_JITTER_MS);
            assert!((-CHECK_JITTER_MS..CHECK_JITTER_MS).contains(&j));
            let first = jitter(0, FIRST_CHECK_WITHIN_MS);
            assert!((0..FIRST_CHECK_WITHIN_MS).contains(&first));
        }
    }

    #[test]
    fn the_window_zone_is_the_workspaces_then_the_servers_then_the_admins() {
        let admins = vec![
            "America/Chicago".to_string(),
            "Europe/Paris".into(),
            "Europe/Paris".into(),
        ];
        let z = pick_zone(Some("America/New_York"), Some("Asia/Tokyo"), &admins);
        assert_eq!(
            (z.zone.as_str(), z.source),
            ("America/New_York", ZoneSource::Workspace)
        );
        let z = pick_zone(Some("  "), Some("Asia/Tokyo"), &admins);
        assert_eq!(z.source, ZoneSource::Server);
        let z = pick_zone(None, None, &admins);
        assert_eq!(
            (z.zone.as_str(), z.source),
            ("Europe/Paris", ZoneSource::Admins)
        );
        let z = pick_zone(
            None,
            None,
            &["America/Chicago".to_string(), "Europe/Paris".into()],
        );
        assert_eq!(z.zone, "America/Chicago", "a tie goes to the oldest admin");
        let z = pick_zone(None, Some(""), &[]);
        assert_eq!(z.source, ZoneSource::Unset);
        assert!(z.said().contains("no time zone is set"));
    }

    fn rec(digest: &str, status: Status, pulled: bool, first_seen: i64) -> ImageRecord {
        ImageRecord {
            digest: digest.into(),
            repository: "r".into(),
            version: None,
            config_version: Some(52),
            status,
            reason: None,
            inspect: json!({}),
            pulled_here: pulled,
            first_seen,
            prepared_at: Some(1),
            good_since: Some(first_seen),
        }
    }

    #[test]
    fn the_prune_keeps_what_runs_or_may_and_drops_the_previous_and_the_abandoned() {
        const DAY: i64 = 24 * 3_600_000;
        let now = 100 * DAY;
        let d = |n: u8| format!("sha256:{}", format!("{n:02x}").repeat(32));
        let records = vec![
            rec(&d(1), Status::Good, true, now - 4 * DAY), // current, good 4 days
            rec(&d(2), Status::Good, true, now - 9 * DAY), // previous
            rec(&d(3), Status::Good, true, now - 20 * DAY), // an agent's
            rec(&d(4), Status::Candidate, true, now - 8 * DAY), // abandoned
            rec(&d(5), Status::Candidate, true, now - DAY), // a fresh candidate
            rec(&d(6), Status::Bad, true, now - 2 * DAY),  // banned
            rec(&d(7), Status::Good, false, now - 30 * DAY), // not ours
            rec(&d(8), Status::Good, true, now - 30 * DAY), // the compiled pin
        ];
        let state = ChannelState {
            current: Some(d(1)),
            previous: Some(d(2)),
            ..Default::default()
        };
        let assigned = HashMap::from([("a".to_string(), d(3))]);
        let mut plan = prune_plan(&state, &records, &assigned, &d(8), now);
        plan.sort();
        assert_eq!(plan, vec![d(2), d(4), d(6)]);
        // The current one good for under three days keeps the previous.
        let mut fresh = records.clone();
        fresh[0].good_since = Some(now - DAY);
        assert!(!prune_plan(&state, &fresh, &assigned, &d(8), now).contains(&d(2)));
        // An open rollout's image and the one it leaves stay.
        let open = ChannelState {
            rollout: Some(Rollout {
                digest: d(4),
                from: Some(d(6)),
                phase: Phase::Rolling,
                ..Default::default()
            }),
            ..state.clone()
        };
        let plan = prune_plan(&open, &records, &assigned, &d(8), now);
        assert!(!plan.contains(&d(4)) && !plan.contains(&d(6)));
    }

    #[test]
    fn the_prune_waits_while_agents_move_but_not_while_a_rollout_waits_for_room() {
        let with = |phase: Phase| ChannelState {
            rollout: Some(images::Rollout {
                phase,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(prune_may_run(&ChannelState::default()));
        for phase in [
            Phase::Preparing,
            Phase::Waiting,
            Phase::Done,
            Phase::Failed,
            Phase::Cancelled,
        ] {
            assert!(prune_may_run(&with(phase)), "{phase:?}");
        }
        for phase in [Phase::Canary, Phase::Rolling, Phase::Soaking] {
            assert!(!prune_may_run(&with(phase)), "{phase:?}");
        }
    }

    #[test]
    fn the_deploy_roll_waits_for_the_fleet_to_be_adopted_but_not_for_ever() {
        use std::time::Duration;
        assert!(!deploy_roll_may_go(false, Duration::from_secs(60)));
        assert!(deploy_roll_may_go(true, Duration::from_secs(60)));
        assert!(deploy_roll_may_go(false, DEPLOY_ROLL_WAIT));
    }

    #[test]
    fn the_compiled_pin_is_the_floor() {
        let (digest, tag, repo) = compiled_pin();
        assert!(images::is_digest(&digest));
        assert!(images::is_version_tag(&tag));
        assert_eq!(repo, "nousresearch/hermes-agent");
    }

    #[test]
    fn the_tuning_defaults_are_the_documented_ones() {
        let t = Tuning::from_env();
        assert_eq!(t.min_stable_ms, 48 * 3_600_000, "48 h on stable");
        assert_eq!(t.first_agent_soak_ms, 10 * 60_000);
        assert_eq!(t.agent_soak_ms, 3 * 60_000);
        assert_eq!(t.rollout_soak_ms, 15 * 60_000);
    }
}
