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
//   canary    in the maintenance window (02:00–05:00 in the instance's time
//             zone by default) or on Roll now: boot it as a throwaway agent
//             with a rendered config and nothing to reach, and ask it only
//             non-model questions (probe.rs).
//   roll      agents one at a time through the ordinary in-place roll,
//             passing over one that is mid-turn or mid-conversation (busy.rs)
//             and coming back to it, each asked the canary's questions once
//             it is up.
//   rollback  any failure stops it: that agent goes back to the last known
//             good at once, the digest is marked bad and never rolled out
//             again, every agent already moved is moved back, and every
//             admin is notified (`hermes_update_failed`).
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
// a step a minute from the scheduler, resumable at any point: the state is
// one settings row and the records, and a step that dies mid-roll leaves
// the roll's own registry and the next step to pick up where it stood.

pub mod busy;
pub mod check;
pub mod prepare;
pub mod probe;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_agent_auth::now_ms;
use talaria_fleet_reconcile::RollOutcome;
use talaria_hermes_images::{
    self as images, ChannelState, ImageRecord, Phase, Rollout, RolloutKind, Status,
};
use talaria_notify::{
    ActorKind, Coalesce, NotificationInput, NotifyActor, NotifyDeps, NotifyEntity, NotifyMeta,
    add_notification_with, entity, group_key,
};
use talaria_scheduler::{JobName, JobSpec};
use talaria_secretbox::SecretBox;

pub use check::{STABLE_REF_DEFAULT, stable_ref};

/// How often the channel is checked, before jitter.
pub const CHECK_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// Each check lands up to this far either side of its six hours, so a
/// release does not see every instance at once.
const CHECK_JITTER_MS: i64 = 30 * 60 * 1000;
/// A host with no check on record checks within this long of boot.
const FIRST_CHECK_WITHIN_MS: i64 = 10 * 60 * 1000;
/// The scheduler's beat for the step.
pub const STEP_EVERY_MS: u64 = 60_000;
/// How long one step keeps rolling agents before it hands over to the next.
const STEP_BUDGET: Duration = Duration::from_secs(12 * 60);
/// The last known good stays on the host this long after the current image
/// became good, then is pruned.
pub const KEEP_PREVIOUS_MS: i64 = 3 * 24 * 60 * 60 * 1000;
/// Prune at most this often.
const PRUNE_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// The advisory lock every step takes.
const LOCK_KEY: &str = "talaria:hermes-channel";
/// Where the notification opens: the Manage overview carries the panel.
const HREF: &str = "/manage";
const LOG: &str = "[hermes]";

/// The notification kind (talaria-notify's registry routes it to the
/// admins' workspace-health class).
pub const KIND_UPDATE_FAILED: &str = "hermes_update_failed";

/// What a step needs.
#[derive(Clone)]
pub struct ChannelDeps {
    pub pg: PgPool,
    pub sb: SecretBox,
    pub notify: NotifyDeps,
}

// ── Where the channel stands ────────────────────────────────────────────────

/// Whether the channel drives the agent image on this instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Following `stable`.
    Following,
    /// An admin pinned this digest.
    Pinned(String),
    /// The operator chose the image; why, in a sentence.
    Off(String),
}

impl Standing {
    pub fn word(&self) -> &'static str {
        match self {
            Standing::Following => "following",
            Standing::Pinned(_) => "pinned",
            Standing::Off(_) => "off",
        }
    }
}

/// Where the channel stands now, from the chassis, the env and the state.
pub async fn standing(state: &ChannelState) -> Standing {
    let chassis = match talaria_fleet_render::read_chassis().await {
        Ok(c) => c,
        Err(e) => return Standing::Off(e),
    };
    let image = chassis
        .service
        .get("image")
        .and_then(Value::as_str)
        .map(str::to_string);
    if !talaria_fleet_render::is_stock_image(image.as_deref()) {
        return Standing::Off(format!(
            "the fleet chassis names its own agent image ({}), so it stays the operator's",
            image.unwrap_or_else(|| "none".into())
        ));
    }
    if let Some(mine) = images::operator_image().await {
        return Standing::Off(format!(
            "HERMES_IMAGE is set ({mine:.120}), so every agent runs that image"
        ));
    }
    match &state.pinned {
        Some(d) => Standing::Pinned(d.clone()),
        None => Standing::Following,
    }
}

/// The compiled pin's digest, version and repository.
pub fn compiled_pin() -> (String, String, String) {
    let (digest, tag, repo) = talaria_fleet_render::compiled_pin_parts();
    (digest.into(), tag.into(), repo.into())
}

/// The digest an agent with no assignment runs.
pub fn effective_current(state: &ChannelState) -> String {
    state.current.clone().unwrap_or_else(|| compiled_pin().0)
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

/// Adopt the stripped env names of every image an agent may run here.
async fn adopt_stripped(pg: &PgPool, state: &ChannelState) {
    let Ok(records) = images::records(pg).await else {
        return;
    };
    let assigned = images::assignments(pg).await.unwrap_or_default();
    let mut in_use: Vec<&str> = assigned.values().map(String::as_str).collect();
    in_use.extend(state.current.as_deref());
    in_use.extend(state.rollout.as_ref().map(|r| r.digest.as_str()));
    let mut names: Vec<String> = Vec::new();
    for r in records
        .iter()
        .filter(|r| in_use.contains(&r.digest.as_str()))
    {
        names.extend(r.stripped_env());
    }
    let added = talaria_agent_secrets::adopt_image_stripped(names);
    if added > 0 {
        tracing::info!("{LOG} adopted {added} env names the running Hermes images strip");
    }
}

// ── The fleet as the channel sees it ────────────────────────────────────────

/// One agent the channel moves: a department's runner.
#[derive(Debug, Clone)]
pub struct ChannelAgent {
    pub id: String,
    pub slug: String,
    pub department: String,
    pub model: String,
    pub display_name: String,
    pub developer: bool,
}

type AgentRow = (String, String, String, String, String, bool);

/// Every managed, enabled agent that runs in its department's container
/// (the render's runner), oldest first.
pub async fn channel_agents(pg: &PgPool) -> Result<Vec<ChannelAgent>, sqlx::Error> {
    let rows: Vec<AgentRow> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, developer from ( \
           select d.*, row_number() over (partition by d.department order by \
             exists (select 1 from agent_versions v \
               where v.agent_id = d.id and v.version = d.current_version) desc, \
             d.created_at, d.id) as rank \
           from agent_defs d where d.managed and d.enabled \
         ) ranked where rank = 1 order by created_at, slug",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, slug, department, model, display_name, developer)| ChannelAgent {
                id,
                slug,
                department,
                model,
                display_name,
                developer,
            },
        )
        .collect())
}

/// Whether the agent runs the channel's image at all: a Developer Agent on
/// the operator's workbench image (`TALARIA_WORKBENCH_IMAGE`) does not.
fn on_channel(agent: &ChannelAgent) -> bool {
    !(agent.developer && workbench_image_set())
}

fn workbench_image_set() -> bool {
    std::env::var("TALARIA_WORKBENCH_IMAGE").is_ok_and(|v| !v.trim().is_empty())
}

// ── The step ────────────────────────────────────────────────────────────────

/// The step's session-level advisory lock, held on its own connection.
struct EngineLock {
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
}

impl EngineLock {
    async fn take(pg: &PgPool) -> Result<Option<EngineLock>, String> {
        let mut conn = pg
            .acquire()
            .await
            .map_err(|e| format!("no database connection for the channel lock: {e}"))?;
        let (locked,): (bool,) = sqlx::query_as("select pg_try_advisory_lock(hashtext($1))")
            .bind(LOCK_KEY)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| format!("the channel lock could not be asked for: {e}"))?;
        Ok(locked.then_some(EngineLock { conn }))
    }

    async fn release(mut self) {
        let unlocked = sqlx::query("select pg_advisory_unlock(hashtext($1))")
            .bind(LOCK_KEY)
            .execute(&mut *self.conn)
            .await;
        if let Err(e) = unlocked {
            tracing::warn!("{LOG} the channel lock could not be released: {e}");
            let _ = self.conn.close().await;
        }
    }
}

/// One step of the channel: check when due, open a rollout when one is
/// called for, and advance it as far as its phase and the step's budget
/// allow. Answers a sentence for the scheduler's log, or None when there
/// was nothing to do (or another step holds the lock).
pub async fn step(deps: &ChannelDeps) -> Result<Option<String>, String> {
    let Some(lock) = EngineLock::take(&deps.pg).await? else {
        return Ok(None);
    };
    let ran = AssertUnwindSafe(step_locked(deps)).catch_unwind().await;
    lock.release().await;
    match ran {
        Ok(r) => r,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// Run a step on a task of its own (an admin action's kick): the request
/// answers at once, and a step already running carries the change instead.
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

async fn step_locked(deps: &ChannelDeps) -> Result<Option<String>, String> {
    let pg = &deps.pg;
    seed_compiled(pg).await?;
    let state = images::load_state(pg).await;
    adopt_stripped(pg, &state).await;
    let standing = standing(&state).await;
    if let Standing::Off(_) = standing {
        return Ok(None);
    }
    let mut said: Vec<String> = Vec::new();
    let now = now_ms();
    match state.next_check_at {
        None => {
            // A host with no check on record: soon, but not with every other
            // instance that booted on the same release.
            let at = now + jitter(0, FIRST_CHECK_WITHIN_MS);
            patch(pg, |s| s.next_check_at = Some(at)).await?;
        }
        Some(due) if now >= due => said.push(run_check(pg).await),
        Some(_) => {}
    }
    if let Some(opened) = open_rollout(pg, &standing).await? {
        said.push(opened);
    }
    said.extend(advance(deps).await?);
    if let Some(pruned) = prune_if_due(pg).await {
        said.push(pruned);
    }
    Ok((!said.is_empty()).then(|| said.join("; ")))
}

/// A uniformly random offset in `lo..hi` ms.
fn jitter(lo: i64, hi: i64) -> i64 {
    let span = (hi - lo).max(1) as u128;
    lo + (uuid::Uuid::new_v4().as_u128() % span) as i64
}

async fn patch<F: FnOnce(&mut ChannelState)>(pg: &PgPool, f: F) -> Result<ChannelState, String> {
    images::patch_state(pg, f)
        .await
        .map_err(|e| format!("the channel state could not be written: {e}"))
}

/// Check the channel and record what it said, whatever it said. Answers the
/// sentence.
pub async fn run_check(pg: &PgPool) -> String {
    let now = now_ms();
    let found = check::check(pg).await;
    let next = now + CHECK_EVERY_MS + jitter(-CHECK_JITTER_MS, CHECK_JITTER_MS);
    let sentence = match &found {
        Ok(f) => format!(
            "stable is {}{}",
            images::label(f.version.as_deref(), &f.digest),
            if f.new { " (new)" } else { "" }
        ),
        Err(e) => format!("the check failed: {e}"),
    };
    let record = match found {
        Ok(f) => images::CheckRecord {
            at: now,
            digest: Some(f.digest),
            version: f.version,
            error: None,
        },
        Err(e) => {
            tracing::warn!("{LOG} the stable check failed — agents stay where they are: {e}");
            images::CheckRecord {
                at: now,
                digest: None,
                version: None,
                error: Some(e.chars().take(500).collect()),
            }
        }
    };
    if let Err(e) = images::patch_state(pg, |s| {
        // A failed check keeps what the last good one found, so a registry
        // outage never reads as "nothing new" — and never opens a rollout.
        let keep = s.last_check.clone();
        s.last_check = Some(match (&record.digest, keep) {
            (None, Some(prev)) => images::CheckRecord {
                at: record.at,
                digest: prev.digest,
                version: prev.version,
                error: record.error.clone(),
            },
            _ => record.clone(),
        });
        s.next_check_at = Some(next);
    })
    .await
    {
        tracing::warn!("{LOG} the check record did not write: {e}");
    }
    sentence
}

/// Open the rollout the channel calls for, when none is open: the pinned
/// digest, or the digest `stable` named when it is new here and not bad.
async fn open_rollout(pg: &PgPool, standing: &Standing) -> Result<Option<String>, String> {
    let state = images::load_state(pg).await;
    if state.rollout.as_ref().is_some_and(Rollout::open) {
        return Ok(None);
    }
    let current = effective_current(&state);
    let (target, kind) = match standing {
        Standing::Off(_) => return Ok(None),
        Standing::Pinned(d) => (d.clone(), RolloutKind::Pin),
        Standing::Following => match state.last_check.as_ref().and_then(|c| c.digest.clone()) {
            Some(d) => (d, RolloutKind::Upgrade),
            None => return Ok(None),
        },
    };
    if target == current {
        return Ok(None);
    }
    let Some(record) = images::record(pg, &target)
        .await
        .map_err(|e| format!("the image record could not be read: {e}"))?
    else {
        return Ok(None);
    };
    // A digest that failed here is never rolled out again, however long
    // `stable` keeps naming it.
    if record.status == Status::Bad {
        return Ok(None);
    }
    let now = now_ms();
    let label = record.label();
    let rollout = Rollout {
        digest: target,
        from: Some(current),
        kind,
        phase: Phase::Preparing,
        started_at: now,
        updated_at: now,
        // A pin is an admin's action: it runs now.
        forced: kind == RolloutKind::Pin,
        ..Default::default()
    };
    let sentence = match kind {
        RolloutKind::Pin => format!("pinned to {label}: moving the fleet onto it"),
        _ => format!("{label} is new on stable: preparing it"),
    };
    let text = sentence.clone();
    patch(pg, move |s| {
        s.rollout = Some(rollout);
        s.note(now, text);
    })
    .await?;
    Ok(Some(sentence))
}

/// Whether the maintenance window is open now, in the instance's zone.
pub async fn window_open(pg: &PgPool, state: &ChannelState) -> (bool, String) {
    let zone = talaria_notify::prefs::workspace_zone(pg).await;
    let m = talaria_tz::local_moment(&zone, now_ms());
    (state.window.contains(m.hour * 60 + m.minute), m.zone)
}

/// Advance the open rollout. Answers what happened.
async fn advance(deps: &ChannelDeps) -> Result<Vec<String>, String> {
    let pg = &deps.pg;
    let started = tokio::time::Instant::now();
    let mut said = Vec::new();
    loop {
        let state = images::load_state(pg).await;
        let Some(rollout) = state.rollout.clone().filter(Rollout::open) else {
            return Ok(said);
        };
        let Some(record) = images::record(pg, &rollout.digest)
            .await
            .map_err(|e| format!("the image record could not be read: {e}"))?
        else {
            end_rollout(pg, Phase::Failed, "its image record is gone").await?;
            return Ok(said);
        };
        let label = record.label();
        match rollout.phase {
            Phase::Preparing => match prepare::prepare(pg, &record).await {
                prepare::Prepared::Ready => {
                    let next = if needs_canary(&rollout, &record) {
                        Phase::Waiting
                    } else {
                        Phase::Rolling
                    };
                    set_phase(
                        pg,
                        next,
                        Some(format!("{label} is downloaded and inspected")),
                    )
                    .await?;
                }
                prepare::Prepared::Pulling(p) | prepare::Prepared::Waiting(p) => {
                    set_detail(pg, &p).await?;
                    return Ok(said);
                }
                prepare::Prepared::Failed {
                    why,
                    definitive: true,
                } => {
                    said.push(fail_candidate(deps, &rollout, &record, &why, None).await?);
                    return Ok(said);
                }
                prepare::Prepared::Failed { why, .. } => {
                    set_detail(pg, &format!("{why} — tried again on the next step")).await?;
                    return Ok(said);
                }
            },
            Phase::Waiting => {
                let (open, zone) = window_open(pg, &state).await;
                if !(rollout.forced || open) {
                    set_detail(
                        pg,
                        &format!(
                            "{label} is ready; it rolls out in the maintenance window ({}–{} {zone}) or on Roll now",
                            state.window.start, state.window.end
                        ),
                    )
                    .await?;
                    return Ok(said);
                }
                set_phase(
                    pg,
                    Phase::Canary,
                    Some(format!("booting {label} as the canary")),
                )
                .await?;
            }
            Phase::Canary => {
                let agents = channel_agents(pg)
                    .await
                    .map_err(|e| format!("the agents could not be read: {e}"))?;
                let base = match agents.iter().find(|a| on_channel(a)) {
                    Some(a) => tokio::fs::read_to_string(
                        talaria_fleet_layout::fleet_dir()
                            .join("agents")
                            .join(&a.slug)
                            .join("config.yaml"),
                    )
                    .await
                    .ok(),
                    None => None,
                };
                match probe::run_canary(&record, base.as_deref()).await {
                    Ok(passed) => {
                        let now = now_ms();
                        patch(pg, |s| {
                            if let Some(r) = s.rollout.as_mut() {
                                r.phase = Phase::Rolling;
                                r.canary_ok_at = Some(now);
                                r.detail = Some(passed.clone());
                                r.updated_at = now;
                            }
                            s.note(now, format!("{label}: {passed}"));
                        })
                        .await?;
                        said.push(format!("{label}: the canary passed"));
                    }
                    Err(why) => {
                        let why = format!("the canary failed: {why}");
                        said.push(fail_candidate(deps, &rollout, &record, &why, None).await?);
                        return Ok(said);
                    }
                }
            }
            Phase::Rolling => {
                let (open, zone) = window_open(pg, &state).await;
                if !(rollout.forced || rollout.kind == RolloutKind::Rollback || open) {
                    set_detail(
                        pg,
                        &format!(
                            "paused outside the maintenance window ({}–{} {zone}); {} moved so far",
                            state.window.start,
                            state.window.end,
                            rollout.moved.len()
                        ),
                    )
                    .await?;
                    return Ok(said);
                }
                said.extend(roll_pending(deps, &state, &rollout, &record, started).await?);
                return Ok(said);
            }
            Phase::Done | Phase::Failed => return Ok(said),
        }
    }
}

/// A rollout runs the canary unless its image is already good here (a pin
/// to a known-good version, a rollback).
fn needs_canary(rollout: &Rollout, record: &ImageRecord) -> bool {
    rollout.kind == RolloutKind::Upgrade || record.status != Status::Good
}

async fn set_phase(pg: &PgPool, phase: Phase, detail: Option<String>) -> Result<(), String> {
    let now = now_ms();
    patch(pg, |s| {
        if let Some(r) = s.rollout.as_mut() {
            r.phase = phase;
            r.detail = detail;
            r.updated_at = now;
        }
    })
    .await
    .map(|_| ())
}

async fn set_detail(pg: &PgPool, detail: &str) -> Result<(), String> {
    let now = now_ms();
    patch(pg, |s| {
        if let Some(r) = s.rollout.as_mut() {
            r.detail = Some(detail.to_string());
            r.updated_at = now;
        }
    })
    .await
    .map(|_| ())
}

async fn end_rollout(pg: &PgPool, phase: Phase, detail: &str) -> Result<(), String> {
    set_phase(pg, phase, Some(detail.to_string())).await
}

/// What one agent's turn in the rollout came to.
enum Move {
    /// It runs the target (moved, or already there; or stopped and assigned
    /// so its next start reads it).
    Moved(String),
    /// Passed over for now, and why.
    Skipped(String),
    /// It failed on the target, and why.
    Failed(String),
}

/// Move every agent the rollout has not moved yet, one at a time, until
/// each is moved or passed over, or the step's budget is spent.
async fn roll_pending(
    deps: &ChannelDeps,
    state: &ChannelState,
    rollout: &Rollout,
    record: &ImageRecord,
    started: tokio::time::Instant,
) -> Result<Vec<String>, String> {
    let pg = &deps.pg;
    let mut said = Vec::new();
    let agents = channel_agents(pg)
        .await
        .map_err(|e| format!("the agents could not be read: {e}"))?;
    let pending: Vec<&ChannelAgent> = agents
        .iter()
        .filter(|a| !rollout.moved.contains(&a.slug))
        .collect();
    let mut skipped_any = false;
    for agent in pending {
        if started.elapsed() > STEP_BUDGET {
            return Ok(said);
        }
        patch(pg, |s| {
            if let Some(r) = s.rollout.as_mut() {
                r.agent = Some(agent.slug.clone());
                r.updated_at = now_ms();
            }
        })
        .await?;
        let moved = move_agent(deps, state, rollout, record, agent).await;
        let now = now_ms();
        match moved {
            Move::Moved(how) => {
                tracing::info!("{LOG} {} ({}): {how}", agent.display_name, agent.slug);
                patch(pg, |s| {
                    if let Some(r) = s.rollout.as_mut() {
                        r.moved.push(agent.slug.clone());
                        r.skipped.retain(|k| k.slug != agent.slug);
                        r.agent = None;
                        r.detail = Some(format!("{}: {how}", agent.display_name));
                        r.updated_at = now;
                    }
                })
                .await?;
            }
            Move::Skipped(why) => {
                skipped_any = true;
                tracing::info!(
                    "{LOG} {} ({}) passed over for now: {why}",
                    agent.display_name,
                    agent.slug
                );
                patch(pg, |s| {
                    if let Some(r) = s.rollout.as_mut() {
                        r.note_skip(&agent.slug, &why, now);
                        r.agent = None;
                        r.detail = Some(format!(
                            "{} passed over for now: {why}; it is tried again on the next step",
                            agent.display_name
                        ));
                        r.updated_at = now;
                    }
                })
                .await?;
            }
            Move::Failed(why) => {
                // The roll's own sentence often opens with the agent's name.
                let why = why
                    .strip_prefix(&format!("{}: ", agent.display_name))
                    .unwrap_or(&why)
                    .to_string();
                let why = format!("{} failed on it: {why}", agent.display_name);
                said.push(match rollout.kind {
                    RolloutKind::Rollback => {
                        // Putting an agent BACK failed: there is nothing
                        // further back to go to. A person decides.
                        let text = format!(
                            "could not put {} back on {}: {why}",
                            agent.display_name,
                            record.label()
                        );
                        notify_admins(deps, record, &text).await;
                        end_rollout(pg, Phase::Failed, &text).await?;
                        text
                    }
                    _ => fail_candidate(deps, rollout, record, &why, Some(agent)).await?,
                });
                return Ok(said);
            }
        }
    }
    if !skipped_any {
        said.push(finish(pg, rollout, record).await?);
    }
    Ok(said)
}

/// The agent's assigned digest, or the channel's current one.
async fn agent_digest(pg: &PgPool, state: &ChannelState, agent_id: &str) -> String {
    images::assignments(pg)
        .await
        .ok()
        .and_then(|a| a.get(agent_id).cloned())
        .unwrap_or_else(|| effective_current(state))
}

/// One agent onto the rollout's image.
async fn move_agent(
    deps: &ChannelDeps,
    state: &ChannelState,
    rollout: &Rollout,
    record: &ImageRecord,
    agent: &ChannelAgent,
) -> Move {
    let pg = &deps.pg;
    if !on_channel(agent) {
        return Move::Moved(
            "runs the operator's workbench image (TALARIA_WORKBENCH_IMAGE), not the channel's"
                .into(),
        );
    }
    let image = record.image_ref();
    let container = talaria_fleet_docker::managed_container(pg, &agent.department).await;
    let running = talaria_fleet_docker::container_running(&container).await;
    // Already there (an earlier step that died after the roll, an apply
    // that rolled it meanwhile, an agent that never left): no restart.
    if agent_digest(pg, state, &agent.id).await == rollout.digest {
        if !running {
            return Move::Moved(format!(
                "stopped; it starts on {} next time",
                record.label()
            ));
        }
        if probe::runs_image(&container, &image).await.unwrap_or(false) {
            // A rollback's agent that never left is not asked again: its
            // log is its whole life, and an old line in it is not this
            // rollout's fault.
            if rollout.kind == RolloutKind::Rollback {
                return Move::Moved(format!("still on {}", record.label()));
            }
            return match agent_checks(agent, &container).await {
                Ok(()) => Move::Moved(format!("already on {}", record.label())),
                Err(why) => Move::Failed(why),
            };
        }
    }
    if talaria_fleet_reconcile::is_rolling(&agent.department) {
        return Move::Skipped("another roll of it is in progress".into());
    }
    match busy::busy(pg, &agent.model).await {
        Ok(Some(why)) => return Move::Skipped(why),
        Ok(None) => {}
        Err(e) => {
            return Move::Skipped(format!("whether it is in use could not be read ({e})"));
        }
    }
    if let Err(e) = images::assign(pg, &agent.id, &rollout.digest).await {
        return Move::Skipped(format!("its image could not be recorded ({e})"));
    }
    if !running {
        // Rendered onto the new image; its next start reads it.
        if let Err(e) =
            talaria_fleet_reconcile::apply_agent_change(pg, &deps.sb, &agent.department).await
        {
            tracing::warn!(
                "{LOG} {}: the render after its move failed: {e}",
                agent.slug
            );
        }
        return Move::Moved(format!(
            "stopped; it starts on {} next time",
            record.label()
        ));
    }
    let outcome =
        talaria_fleet_reconcile::roll_running_agent_outcome(pg, &deps.sb, &agent.department).await;
    let landed = match outcome {
        Ok(RollOutcome::Rolled) => Ok(()),
        Ok(RollOutcome::LeftAlone) => {
            return Move::Moved(format!(
                "stopped before its roll; it starts on {} next time",
                record.label()
            ));
        }
        Ok(RollOutcome::StillStarting { late, .. }) => match late.await {
            Ok(None) => Ok(()),
            Ok(Some(why)) => Err(why),
            Err(e) => Err(format!("its late health wait was lost: {e}")),
        },
        Ok(RollOutcome::Soft(s)) if s.contains("already mid-roll") => {
            return Move::Skipped("another roll of it is in progress".into());
        }
        Ok(RollOutcome::Soft(s)) => Err(s),
        Err(e) => Err(e),
    };
    if let Err(why) = landed {
        return Move::Failed(why);
    }
    if !probe::runs_image(&container, &image).await.unwrap_or(false) {
        return Move::Failed(format!(
            "its recreated container does not run {image} — HERMES_IMAGE or a chassis override is in the way"
        ));
    }
    match agent_checks(agent, &container).await {
        Ok(()) => Move::Moved(format!("rolled onto {} and healthy", record.label())),
        Err(why) => Move::Failed(why),
    }
}

/// The canary's questions, asked of a moved agent once it is up: its API
/// server through the gateway's dial (its manifest entry), its log, and its
/// drain tools.
async fn agent_checks(agent: &ChannelAgent, container: &str) -> Result<(), String> {
    let entry = talaria_fleet_layout::read_manifest()
        .await
        .into_iter()
        .find(|e| e.model == agent.model)
        .ok_or_else(|| {
            "it is not in the fleet manifest, so the gateway cannot dial it".to_string()
        })?;
    probe::api_answers(&entry.url, entry.key.as_deref()).await?;
    if let Some(fault) = probe::log_fault(&probe::logs(container, 2000).await) {
        return Err(format!("its log says: {fault}"));
    }
    probe::drain_ready(container).await
}

/// The rollout's image failed: mark it bad (never rolled out again), put the
/// failed agent back at once, move every agent already moved back, and tell
/// the admins. Answers the sentence.
async fn fail_candidate(
    deps: &ChannelDeps,
    rollout: &Rollout,
    record: &ImageRecord,
    why: &str,
    agent: Option<&ChannelAgent>,
) -> Result<String, String> {
    let pg = &deps.pg;
    let label = record.label();
    if let Err(e) = images::mark_bad(pg, &rollout.digest, why).await {
        tracing::warn!("{LOG} {label} could not be marked bad: {e}");
    }
    let back_to = rollout.from.clone().unwrap_or_else(|| compiled_pin().0);
    let back_label = match images::record(pg, &back_to).await {
        Ok(Some(r)) => r.label(),
        _ => images::label(None, &back_to),
    };
    // The agent that failed goes back now, whatever it is doing: it is on
    // an image that does not work.
    let mut put_back = None;
    if let Some(a) = agent {
        let back = match images::assign(pg, &a.id, &back_to).await {
            Err(e) => Err(format!("its image could not be recorded ({e})")),
            Ok(()) => {
                match talaria_fleet_reconcile::roll_agent_back_outcome(pg, &deps.sb, &a.department)
                    .await
                {
                    Ok(RollOutcome::Soft(s)) => Err(s),
                    Ok(_) => Ok(()),
                    Err(e) => Err(e),
                }
            }
        };
        put_back = Some(match &back {
            Ok(()) => format!("{} is back on {back_label}", a.display_name),
            Err(e) => format!(
                "{} could not be put back on {back_label} ({e}) — see its logs",
                a.display_name
            ),
        });
    }
    let moved: Vec<String> = rollout
        .moved
        .iter()
        .filter(|s| agent.is_none_or(|a| &a.slug != *s))
        .cloned()
        .collect();
    let mut text = format!(
        "{label} was not rolled out: {why}. It is marked bad and will not be tried again; agents stay on {back_label}"
    );
    if let Some(p) = &put_back {
        text.push_str(&format!(". {p}"));
    }
    if !moved.is_empty() {
        text.push_str(&format!(
            ". Moving the {} agent{} it reached back",
            moved.len(),
            if moved.len() == 1 { "" } else { "s" }
        ));
    }
    tracing::warn!("{LOG} {text}");
    notify_admins(deps, record, &text).await;
    let now = now_ms();
    let unpin = rollout.kind == RolloutKind::Pin;
    let back = Rollout {
        digest: back_to,
        from: Some(rollout.digest.clone()),
        kind: RolloutKind::Rollback,
        phase: if moved.is_empty() {
            Phase::Done
        } else {
            Phase::Rolling
        },
        started_at: now,
        updated_at: now,
        forced: true,
        moved: agent.map(|a| vec![a.slug.clone()]).unwrap_or_default(),
        detail: Some(text.clone()),
        ..Default::default()
    };
    let note = text.clone();
    patch(pg, move |s| {
        s.rollout = Some(back);
        if unpin {
            s.pinned = None;
        }
        s.note(now, note);
    })
    .await?;
    Ok(text)
}

/// Every agent runs the rollout's image: it is good, and current.
async fn finish(pg: &PgPool, rollout: &Rollout, record: &ImageRecord) -> Result<String, String> {
    if let Err(e) = images::mark_good(pg, &rollout.digest).await {
        tracing::warn!("{LOG} {} could not be marked good: {e}", record.label());
    }
    let now = now_ms();
    let label = record.label();
    let text = match rollout.kind {
        RolloutKind::Rollback => format!("every agent is back on {label}"),
        _ => format!("every agent runs {label}"),
    };
    let note = text.clone();
    let rollout = rollout.clone();
    patch(pg, move |s| {
        let before = effective_current(s);
        if before != rollout.digest {
            s.previous = match rollout.kind {
                RolloutKind::Rollback => None,
                _ => Some(before),
            };
        }
        s.current = Some(rollout.digest.clone());
        if let Some(r) = s.rollout.as_mut() {
            r.phase = Phase::Done;
            r.agent = None;
            r.detail = Some(note.clone());
            r.updated_at = now;
        }
        s.note(now, note);
    })
    .await?;
    tracing::info!("{LOG} {text}");
    Ok(text)
}

/// Tell every admin, one row per digest (a re-alert of the same row).
async fn notify_admins(deps: &ChannelDeps, record: &ImageRecord, text: &str) {
    let admins: Vec<(String,)> =
        match sqlx::query_as("select id::text from users where role = 'admin'")
            .fetch_all(&deps.pg)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!("{LOG} the admins could not be read to notify: {e}");
                return;
            }
        };
    let title = format!("Agent image update to {} failed", record.label());
    let meta = NotifyMeta {
        group_key: Some(group_key::hermes_update(&record.digest)),
        coalesce: Coalesce::Realert,
        entity: Some(NotifyEntity::new(
            entity::HERMES_IMAGE,
            record.digest.as_str(),
        )),
        actor: Some(NotifyActor {
            kind: ActorKind::System,
            id: "hermes-channel".into(),
            label: "Agent image updates".into(),
        }),
        ..Default::default()
    };
    for (admin,) in admins {
        if let Err(e) = add_notification_with(
            &deps.notify,
            &admin,
            &NotificationInput {
                kind: KIND_UPDATE_FAILED,
                title: &title,
                body: Some(text),
                href: Some(HREF),
            },
            &meta,
        )
        .await
        {
            tracing::warn!("{LOG} the notification to {admin} failed: {e}");
        }
    }
}

// ── Pruning ─────────────────────────────────────────────────────────────────

static PRUNED_AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Remove the images this instance pulled and no longer needs: everything
/// but the current image, the pinned one, any assigned one, the compiled
/// pin, and the previous image until the current one has been good for
/// [`KEEP_PREVIOUS_MS`]. An image a container still uses is refused by
/// docker and kept. An image this instance did not pull is never touched.
async fn prune_if_due(pg: &PgPool) -> Option<String> {
    let state = images::load_state(pg).await;
    if state.rollout.as_ref().is_some_and(Rollout::open) {
        return None;
    }
    let now = now_ms();
    if now - PRUNED_AT.load(std::sync::atomic::Ordering::Relaxed) < PRUNE_EVERY_MS {
        return None;
    }
    PRUNED_AT.store(now, std::sync::atomic::Ordering::Relaxed);
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
        match talaria_fleet_docker::docker(
            &["image", "rm", &r.image_ref()],
            Duration::from_secs(120),
        )
        .await
        {
            Ok(_) => {
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
    let _ = images::patch_state(pg, move |s| s.note(now, note)).await;
    Some(text)
}

// ── Admin actions ───────────────────────────────────────────────────────────

/// Check `stable` now, and step.
pub async fn check_now(deps: &ChannelDeps) -> Result<images::CheckRecord, String> {
    seed_compiled(&deps.pg).await?;
    run_check(&deps.pg).await;
    kick(deps);
    Ok(images::load_state(&deps.pg)
        .await
        .last_check
        .unwrap_or_default())
}

/// Run the rollout now, outside the maintenance window: the open one, or the
/// one `stable` calls for.
pub async fn roll_now(deps: &ChannelDeps) -> Result<String, String> {
    let pg = &deps.pg;
    seed_compiled(pg).await?;
    let state = images::load_state(pg).await;
    let standing = standing(&state).await;
    if let Standing::Off(why) = &standing {
        return Err(why.clone());
    }
    if !state.rollout.as_ref().is_some_and(Rollout::open) {
        open_rollout(pg, &standing).await?;
    }
    let state = patch(pg, |s| {
        if let Some(r) = s.rollout.as_mut().filter(|r| r.open()) {
            r.forced = true;
            r.updated_at = now_ms();
        }
    })
    .await?;
    let Some(r) = state.rollout.filter(Rollout::open) else {
        return Err("there is nothing to roll out: every agent runs what the channel names".into());
    };
    kick(deps);
    let label = images::record(pg, &r.digest)
        .await
        .ok()
        .flatten()
        .map(|x| x.label())
        .unwrap_or_else(|| images::label(None, &r.digest));
    Ok(format!("rolling out {label} now"))
}

/// Roll back: an open rollout is abandoned (its image marked bad) and every
/// agent it reached moves back; otherwise the current image is marked bad
/// and every agent moves to the previous one (or the compiled pin).
pub async fn roll_back(deps: &ChannelDeps, by: &str) -> Result<String, String> {
    let pg = &deps.pg;
    seed_compiled(pg).await?;
    let state = images::load_state(pg).await;
    let (bad, target) = match state.rollout.as_ref().filter(|r| r.open()) {
        Some(r) if r.kind != RolloutKind::Rollback => (
            r.digest.clone(),
            r.from.clone().unwrap_or_else(|| compiled_pin().0),
        ),
        Some(_) => return Err("a rollback is already in progress".into()),
        None => {
            let current = effective_current(&state);
            let target = state.previous.clone().unwrap_or_else(|| compiled_pin().0);
            if current == target {
                return Err(
                    "there is no earlier version to go back to: the fleet runs this release's own pin"
                        .into(),
                );
            }
            (current, target)
        }
    };
    let reason = format!("rolled back by {by}");
    images::mark_bad(pg, &bad, &reason)
        .await
        .map_err(|e| format!("the image could not be marked bad: {e}"))?;
    let label_of = |r: Option<ImageRecord>, d: &str| {
        r.map(|r| r.label())
            .unwrap_or_else(|| images::label(None, d))
    };
    let bad_label = label_of(images::record(pg, &bad).await.ok().flatten(), &bad);
    let target_label = label_of(images::record(pg, &target).await.ok().flatten(), &target);
    let now = now_ms();
    let text = format!("{bad_label} {reason}: moving every agent to {target_label}");
    let note = text.clone();
    patch(pg, move |s| {
        s.rollout = Some(Rollout {
            digest: target,
            from: Some(bad.clone()),
            kind: RolloutKind::Rollback,
            phase: Phase::Preparing,
            started_at: now,
            updated_at: now,
            forced: true,
            detail: Some(note.clone()),
            ..Default::default()
        });
        if s.pinned.as_deref() == Some(bad.as_str()) {
            s.pinned = None;
        }
        s.note(now, note);
    })
    .await?;
    kick(deps);
    Ok(text)
}

/// Pin every agent to `digest` (a version this instance has run well, or
/// the compiled pin); `None` lifts the pin and follows `stable` again.
pub async fn pin(deps: &ChannelDeps, digest: Option<&str>, by: &str) -> Result<String, String> {
    let pg = &deps.pg;
    seed_compiled(pg).await?;
    let now = now_ms();
    let Some(digest) = digest else {
        patch(pg, |s| {
            s.pinned = None;
            s.note(now, format!("{by} lifted the pin: following stable"));
        })
        .await?;
        kick(deps);
        return Ok("following stable again".into());
    };
    if !images::is_digest(digest) {
        return Err("that is not an image digest".into());
    }
    let record = images::record(pg, digest)
        .await
        .map_err(|e| format!("the image record could not be read: {e}"))?
        .ok_or_else(|| "this instance has no record of that image".to_string())?;
    if record.status != Status::Good {
        return Err(format!(
            "{} has not run well here ({}), so it cannot be pinned",
            record.label(),
            record.status.as_str()
        ));
    }
    let label = record.label();
    let digest = digest.to_string();
    let note = format!("{by} pinned agents to {label}");
    patch(pg, move |s| {
        s.pinned = Some(digest.clone());
        // An open rollout to something else gives way to the pin.
        if s.rollout
            .as_ref()
            .is_some_and(|r| r.open() && r.digest != digest)
        {
            s.rollout = None;
        }
        s.note(now, note);
    })
    .await?;
    kick(deps);
    Ok(format!("pinned to {label}"))
}

/// Set the maintenance window.
pub async fn set_window(pg: &PgPool, start: &str, end: &str) -> Result<images::Window, String> {
    let (Some(s), Some(e)) = (images::clock_minutes(start), images::clock_minutes(end)) else {
        return Err("the window's ends are HH:MM times".into());
    };
    if s == e {
        return Err("the window cannot start and end at the same minute".into());
    }
    let window = images::Window {
        start: start.trim().into(),
        end: end.trim().into(),
    };
    let set = window.clone();
    patch(pg, move |st| st.window = set).await?;
    Ok(window)
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
    })
}

/// Everything the admin panel shows.
pub async fn status(pg: &PgPool) -> Result<Value, String> {
    seed_compiled(pg).await?;
    let state = images::load_state(pg).await;
    let standing = standing(&state).await;
    let records = images::records(pg)
        .await
        .map_err(|e| format!("the image records could not be read: {e}"))?;
    let assigned = images::assignments(pg)
        .await
        .map_err(|e| format!("the agents' images could not be read: {e}"))?;
    let agents = channel_agents(pg)
        .await
        .map_err(|e| format!("the agents could not be read: {e}"))?;
    let label_of = |d: &str| {
        records
            .iter()
            .find(|r| r.digest == d)
            .map(|r| r.label())
            .unwrap_or_else(|| images::label(None, d))
    };
    let current = effective_current(&state);
    let (open, zone) = window_open(pg, &state).await;
    let off_reason = match &standing {
        Standing::Off(why) => Some(why.clone()),
        _ => None,
    };
    let agents_json: Vec<Value> = agents
        .iter()
        .map(|a| {
            let digest = assigned
                .get(&a.id)
                .cloned()
                .unwrap_or_else(|| current.clone());
            let image = if off_reason.is_some() || !on_channel(a) {
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
            "detail": r.detail,
        })
    });
    let (pin_digest, pin_tag, _) = compiled_pin();
    let previous_label = state.previous.as_deref().map(label_of);
    Ok(json!({
        "standing": standing.word(),
        "offReason": off_reason,
        "channel": stable_ref(),
        "pinned": state.pinned.as_deref().map(|d| json!({ "digest": d, "label": label_of(d) })),
        "current": { "digest": current, "label": label_of(&current) },
        "previous": state.previous.as_deref().map(|d| json!({ "digest": d, "label": previous_label })),
        "compiled": { "digest": pin_digest, "label": pin_tag },
        "lastCheck": state.last_check.as_ref().map(|c| json!({
            "at": c.at,
            "digest": c.digest,
            "label": c.digest.as_deref().map(|d| images::label(c.version.as_deref(), d)),
            "error": c.error,
        })),
        "nextCheckAt": state.next_check_at,
        "window": { "start": state.window.start, "end": state.window.end, "zone": zone, "open": open },
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
        // A step can hold a canary (up to ~13 min) or roll agents for its
        // budget plus one agent's late health wait.
        max_run_ms: Some(40 * 60_000),
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
        assert_eq!(effective_current(&ChannelState::default()), digest);
    }
}
