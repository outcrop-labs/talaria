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
//   prepare   pull it in the background through the agents' shared pull,
//             giving way to any other download, and read the image with
//             networking off for its own `_config_version`, the env names
//             its Hermes strips, and its drain tools (prepare.rs).
//   wait      until it has been on stable for 48 h (from this instance's
//             first sight of it), so a release the community finds broken
//             is usually replaced before any instance runs it; then until the
//             maintenance window (02:00–05:00 in the instance's zone), or Roll
//             now.
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
// One step at a time across every api instance (a Postgres advisory lock),
// a step a minute from the scheduler. The admin actions do not wait for a
// step: they change the state row under its row lock, and every write the
// step makes names its rollout, so a step that finds its rollout replaced
// stops and re-reads (engine.rs).

pub mod busy;
pub mod check;
pub mod engine;
pub mod ops;
pub mod prepare;
pub mod probe;

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
pub use engine::{Engine, Tuning};
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

/// A rollout runs the canary unless its image is already good here (a pin
/// to a known-good version, a rollback, a catch-up).
pub(crate) fn needs_canary(rollout: &Rollout, record: &ImageRecord) -> bool {
    rollout.kind == RolloutKind::Upgrade || record.status != Status::Good
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
/// channel's.
struct EngineLock {
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
    key: String,
}

impl EngineLock {
    async fn take(pg: &PgPool, state_key: &str) -> Result<Option<EngineLock>, String> {
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
        Ok(locked.then_some(EngineLock { conn, key }))
    }

    async fn release(mut self) {
        let unlocked = sqlx::query("select pg_advisory_unlock(hashtext($1))")
            .bind(&self.key)
            .execute(&mut *self.conn)
            .await;
        if let Err(e) = unlocked {
            tracing::warn!("{LOG} the channel lock could not be released: {e}");
            let _ = self.conn.close().await;
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
    let Some(lock) = EngineLock::take(pg, key).await? else {
        return Ok(None);
    };
    let engine = Engine {
        pg,
        key,
        ops,
        tuning,
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
    lock.release().await;
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
                        "Agent image updates are stopped: their state is unreadable",
                        &format!(
                            "{e}. Nothing was changed. Fix or delete the app_settings row '{}' (deleting it starts the channel over on the release's own pin).",
                            images::STATE_KEY
                        ),
                    )
                    .await;
                }
            }
            Err(e)
        }
    }
}

/// Run a step on a task of its own (an admin action's kick): the request
/// answers at once, and a step already running picks the change up when it
/// next reads the row.
pub fn kick(deps: &ChannelDeps) {
    let deps = deps.clone();
    tokio::spawn(async move {
        match step(&deps).await {
            Ok(Some(s)) => tracing::info!("{LOG} {s}"),
            Ok(None) => {}
            Err(e) => tracing::warn!("{LOG} {e}"),
        }
    });
}

// ── Pruning ─────────────────────────────────────────────────────────────────

static PRUNED_AT: AtomicI64 = AtomicI64::new(0);

/// Remove the images this instance pulled and no longer needs: everything
/// but the current image, the pinned one, any assigned one, the compiled
/// pin, and the previous image until the current one has been good for
/// [`KEEP_PREVIOUS_MS`]. An image a container still uses is refused by
/// docker and kept. An image this instance did not pull is never touched.
async fn prune_if_due(engine: &Engine<'_>) -> Option<String> {
    let pg = engine.pg;
    let state = engine.load().await.ok()?;
    if state.rollout.as_ref().is_some_and(Rollout::open) {
        return None;
    }
    let now = engine.ops.now_ms();
    if now - PRUNED_AT.load(Ordering::Relaxed) < PRUNE_EVERY_MS {
        return None;
    }
    PRUNED_AT.store(now, Ordering::Relaxed);
    let records = images::records(pg).await.ok()?;
    let current = state.current.clone();
    let current_good_since = current
        .as_ref()
        .and_then(|c| records.iter().find(|r| &r.digest == c))
        .and_then(|r| r.good_since);
    let keep_previous = current_good_since.is_none_or(|t| now - t < KEEP_PREVIOUS_MS);
    let mut keep: Vec<String> = vec![compiled_pin().0];
    keep.extend(current);
    keep.extend(state.pinned.clone());
    if keep_previous {
        keep.extend(state.previous.clone());
    }
    let assigned = images::assignments(pg).await.unwrap_or_default();
    keep.extend(assigned.into_values());
    let mut pruned = Vec::new();
    for r in records
        .iter()
        .filter(|r| r.pulled_here && r.status != Status::Candidate && !keep.contains(&r.digest))
    {
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
// it with the step's writes), then a kick. None waits for a running step:
// replacing the rollout is enough, because every write the step makes names
// its rollout and is refused once it is replaced.

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
    if !state.rollout.as_ref().is_some_and(Rollout::open) {
        e.open_rollout().await?;
    }
    let (state, forced) = e
        .patch(|s| {
            Ok(match s.rollout.as_mut().filter(|r| r.open()) {
                Some(r) => {
                    r.forced = true;
                    r.retry_at = None;
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
        "rolling out {} now",
        label_or(rec.as_ref(), &r.digest)
    ))
}

/// Roll back.
/// - An open Upgrade is abandoned: its image is marked bad (an admin can
///   allow it again) and every agent it reached moves back.
/// - An open Pin (or catch-up) is cancelled: nothing is marked bad, the pin
///   is lifted, and agents go back to what the fleet ran.
/// - With nothing open, the fleet moves to the previous version: the version
///   it leaves is marked bad if it came from stable (so stable does not roll
///   it forward again), and if a Pin put it there, the pin moves to the
///   previous version instead — a known-good version is never banned for
///   being pinned away from.
pub async fn roll_back_with(e: &Engine<'_>, by: &str) -> Result<String, String> {
    seed_compiled(e.pg).await?;
    let now = e.ops.now_ms();
    let compiled = compiled_pin().0;
    let who = by.to_string();
    let (_, (ban, target, left)) = e
        .patch(move |s| {
            let current = s.current.clone().unwrap_or_else(|| compiled.clone());
            let (ban, target, left, reached) = match s.rollout.clone().filter(|r| r.open()) {
                Some(r) if r.kind == RolloutKind::Rollback => {
                    return Err(StateError::Store(
                        "a rollback is already in progress".into(),
                    ));
                }
                Some(r) => {
                    let mut reached = r.moved.clone();
                    reached.extend(r.agent.clone());
                    let target = r.from.clone().unwrap_or_else(|| current.clone());
                    if r.kind == RolloutKind::Pin {
                        s.pinned = None;
                    }
                    let ban = (r.kind == RolloutKind::Upgrade).then(|| r.digest.clone());
                    (ban, target, r.digest.clone(), reached)
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
                        None
                    } else {
                        Some(current.clone())
                    };
                    (ban, target, current, Vec::new())
                }
            };
            s.rollout = Some(Rollout {
                id: images::new_rollout_id(now),
                digest: target.clone(),
                from: Some(left.clone()),
                kind: RolloutKind::Rollback,
                phase: Phase::Preparing,
                started_at: now,
                updated_at: now,
                forced: true,
                reached,
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
        "moving every agent from {} to {}",
        label_or(left_rec.as_ref(), &left),
        label_or(target_rec.as_ref(), &target)
    );
    if let Some(bad) = ban {
        match images::mark_bad(e.pg, &bad, &format!("rolled back by {by}")).await {
            Ok(()) => text.push_str(&format!(
                "; {} is marked bad until an admin allows it again",
                label_or(left_rec.as_ref(), &bad)
            )),
            Err(x) => text.push_str(&format!(
                "; it could NOT be marked bad ({x}), so stable may roll it forward again — pin to hold the fleet"
            )),
        }
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
/// the rest. Neither a digest nor a version lifts the pin.
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
            let mut reached = open.as_ref().map(|r| r.moved.clone()).unwrap_or_default();
            reached.extend(open.as_ref().and_then(|r| r.agent.clone()));
            s.rollout = Some(Rollout {
                id: images::new_rollout_id(now),
                digest: digest.clone(),
                from: Some(current),
                kind: RolloutKind::Pin,
                phase: Phase::Preparing,
                started_at: now,
                updated_at: now,
                forced: true,
                reached,
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

/// Lift a ban: the version may be rolled out (or pinned) again.
pub async fn allow_with(e: &Engine<'_>, digest: &str, by: &str) -> Result<String, String> {
    if !images::is_digest(digest) {
        return Err("that is not an image digest".into());
    }
    let record = images::record(e.pg, digest)
        .await
        .map_err(|x| format!("the image record could not be read: {x}"))?
        .ok_or_else(|| "this instance has no record of that image".to_string())?;
    if !images::allow_again(e.pg, digest)
        .await
        .map_err(|x| format!("the ban could not be lifted: {x}"))?
    {
        return Err(format!("{} is not marked bad", record.label()));
    }
    let now = e.ops.now_ms();
    let text = format!("{by} allowed {} again", record.label());
    let note = text.clone();
    e.patch(move |s| {
        s.note(now, note);
        Ok(())
    })
    .await
    .map_err(|x| x.to_string())?;
    Ok(text)
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
        };
        $body
    }};
}

pub async fn check_now(deps: &ChannelDeps) -> Result<images::CheckRecord, String> {
    let out = on_real_fleet!(deps, |e| check_now_with(&e).await);
    kick(deps);
    out
}

pub async fn roll_now(deps: &ChannelDeps) -> Result<String, String> {
    let out = on_real_fleet!(deps, |e| roll_now_with(&e).await);
    kick(deps);
    out
}

pub async fn roll_back(deps: &ChannelDeps, by: &str) -> Result<String, String> {
    let out = on_real_fleet!(deps, |e| roll_back_with(&e, by).await);
    kick(deps);
    out
}

pub async fn pin(
    deps: &ChannelDeps,
    digest: Option<&str>,
    version: Option<&str>,
    by: &str,
) -> Result<String, String> {
    let out = on_real_fleet!(deps, |e| pin_with(&e, digest, version, by).await);
    kick(deps);
    out
}

pub async fn allow(deps: &ChannelDeps, digest: &str, by: &str) -> Result<String, String> {
    let out = on_real_fleet!(deps, |e| allow_with(&e, digest, by).await);
    kick(deps);
    out
}

pub async fn set_window(
    deps: &ChannelDeps,
    start: &str,
    end: &str,
) -> Result<images::Window, String> {
    on_real_fleet!(deps, |e| set_window_with(&e, start, end).await)
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
    let zone = talaria_notify::prefs::workspace_zone(pg).await;
    let m = talaria_tz::local_moment(&zone, talaria_agent_auth::now_ms());
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
        "window": { "start": state.window.start, "end": state.window.end, "zone": m.zone, "open": open },
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
    fn a_rollout_canaries_anything_not_already_good_here() {
        let mut r = Rollout {
            kind: RolloutKind::Upgrade,
            ..Default::default()
        };
        let mut rec = ImageRecord {
            digest: "sha256:00".into(),
            repository: "r".into(),
            version: None,
            config_version: Some(51),
            status: Status::Good,
            reason: None,
            inspect: json!({}),
            pulled_here: false,
            first_seen: 0,
            prepared_at: Some(1),
            good_since: Some(1),
        };
        assert!(needs_canary(&r, &rec), "an upgrade always canaries");
        r.kind = RolloutKind::Rollback;
        assert!(!needs_canary(&r, &rec), "back to a known good does not");
        rec.status = Status::Candidate;
        assert!(needs_canary(&r, &rec));
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
