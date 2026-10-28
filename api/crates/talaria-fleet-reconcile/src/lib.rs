// Fleet reconciliation. A ROLL recreates an agent's container IN PLACE:
// re-render, pull the image if the host lacks it, stop the running container
// gracefully, start a fresh one from the render on the same state volume, and
// wait for real health. The agent is offline for the restart — `proxy_chat`
// holds a turn up to two minutes across it.
//
// WHY NOT BLUE/GREEN. This used to start the replacement beside the live
// container and cut over once it was healthy. Every Hermes since v2026.4.30
// takes an `flock` on `/opt/data/gateway.lock` in the agent's state volume —
// one gateway per HERMES_HOME — so the replacement, sharing that volume,
// logged "Cannot reserve gateway profiles: Gateway runtime already owns
// profile /opt/data" on every s6 respawn, never bound :8642, and was
// discarded after 120s. A roll of a running agent could never pass, and
// nothing it was meant to deliver (a rotated key, a new image) landed.

use sqlx::PgPool;

use std::collections::{HashMap, HashSet};

use talaria_fleet_docker::Slot;
use talaria_secretbox::SecretBox;

use futures_util::future::BoxFuture;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

/// The departments THIS process is mid-roll on. Two rolls of one department
/// would recreate the container under each other (and race the move of a
/// legacy `-b` agent home), so a roll holds its department here for its
/// whole run. Same guard shape as chat-persist's `CONTINUING`: the set
/// closes the in-process window; the advisory lock below is the
/// cross-instance one.
static ROLLING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Whether a roll of this department is in flight in this process — what
/// the control route answers 409 with, and what `reconcile_fleet` skips.
pub fn is_rolling(department: &str) -> bool {
    ROLLING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(department)
}

/// Holds the department in `ROLLING` for exactly as long as it lives.
struct RollGuard(String);

impl RollGuard {
    fn take(department: &str) -> Option<RollGuard> {
        let mut set = ROLLING.lock().unwrap_or_else(|p| p.into_inner());
        if set.contains(department) {
            return None;
        }
        set.insert(department.to_string());
        Some(RollGuard(department.to_string()))
    }
}

impl Drop for RollGuard {
    fn drop(&mut self) {
        ROLLING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.0);
    }
}

/// The advisory-lock key for a department's roll — `hashtext` of a
/// namespaced string, so it cannot collide with the migration lock or any
/// other advisory key the app takes.
fn roll_lock_key(department: &str) -> String {
    format!("fleet-roll:{department}")
}

// ── The roll registry ────────────────────────────────────────────────────────
//
// `ROLLING` above answers one yes/no per department for THIS process. The
// registry is the story: when each roll started, how it ended, and the
// sentence it failed with — what `GET /api/fleet/roll` shows and what a
// second api instance reads back through the Redis hash the sink writes.
// Recorded by `roll_agent_mode` itself (start, then the verdict) so a roll from
// any door — the fleet-wide button, one agent's control, an MCP apply —
// lands on the same page.

/// Finished rolls stay visible this long, then fall off the picture.
pub const ROLL_OUTCOME_TTL_MS: i64 = 3_600_000;

/// The Redis hash the sink mirrors the registry into, one field per
/// department, so every api instance answers the same picture.
pub const ROLL_REGISTRY_KEY: &str = "talaria:fleet-roll:v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RollState {
    Rolling,
    Done,
    Failed,
    /// The wire word for a department the registry has no row for, so the
    /// picture can list every agent in one vocabulary. Recorded only by a
    /// roll that was applying a change and found, under the service's lock,
    /// that its agent had stopped: it left it stopped, and the row says no
    /// roll happened rather than "rolled".
    Idle,
}

/// One department's roll as the registry remembers it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollRecord {
    pub department: String,
    pub state: RollState,
    /// Epoch ms the roll started (for a queued batch member, when it was
    /// queued).
    pub since: i64,
    /// Epoch ms of the last transition — the newer record wins a merge.
    pub updated: i64,
    /// The failure sentence, or a progress note while rolling.
    pub detail: Option<String>,
    /// The fleet-wide batch this roll belongs to, if any.
    pub batch: Option<String>,
}

impl RollRecord {
    /// Whether this record has aged past [`ROLL_OUTCOME_TTL_MS`] — only a
    /// finished roll expires; a roll still marked rolling is kept until its
    /// verdict overwrites it.
    pub fn expired(&self, now: i64) -> bool {
        self.state != RollState::Rolling && now - self.updated > ROLL_OUTCOME_TTL_MS
    }
}

static ROLL_REGISTRY: LazyLock<Mutex<HashMap<String, RollRecord>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Where each registry write is mirrored (the Redis hash). Wired by the
/// routes layer, which owns the connection; unset, the registry is
/// in-process only and still complete for this instance.
pub static ROLL_SINK: OnceLock<Arc<dyn Fn(RollRecord) -> BoxFuture<'static, ()> + Send + Sync>> =
    OnceLock::new();

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn record_roll(record: RollRecord) {
    ROLL_REGISTRY
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(record.department.clone(), record.clone());
    if let Some(sink) = ROLL_SINK.get() {
        let sink = sink.clone();
        tokio::spawn(async move { sink(record).await });
    }
}

/// This process's view of every roll it recorded, expired outcomes pruned.
pub fn roll_records() -> Vec<RollRecord> {
    let now = now_ms();
    let mut map = ROLL_REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    map.retain(|_, r| !r.expired(now));
    let mut out: Vec<RollRecord> = map.values().cloned().collect();
    out.sort_by(|a, b| a.department.cmp(&b.department));
    out
}

/// Fold another instance's records (the Redis hash) into this one's: per
/// department the newer `updated` wins, expired outcomes drop.
pub fn merge_roll_records(local: Vec<RollRecord>, remote: Vec<RollRecord>) -> Vec<RollRecord> {
    let now = now_ms();
    let mut by_dept: HashMap<String, RollRecord> = HashMap::new();
    for r in local.into_iter().chain(remote) {
        if r.expired(now) {
            continue;
        }
        match by_dept.get(&r.department) {
            Some(have) if have.updated >= r.updated => {}
            _ => {
                by_dept.insert(r.department.clone(), r);
            }
        }
    }
    let mut out: Vec<RollRecord> = by_dept.into_values().collect();
    out.sort_by(|a, b| a.department.cmp(&b.department));
    out
}

/// The renderer, wired from the api binary so this crate does not depend on
/// fleet-render: (rendered agent count, warnings).
pub static RENDER_FLEET: OnceLock<
    Arc<
        dyn Fn(sqlx::PgPool, SecretBox) -> BoxFuture<'static, Result<(usize, Vec<String>), String>>
            + Send
            + Sync,
    >,
> = OnceLock::new();

/// How long a recreated container gets to report healthy: the chassis
/// healthcheck's 60s start period plus five 15s probes, then headroom for
/// a slow first boot of a freshly pulled image.
const ROLL_HEALTH_WAIT_MS: u64 = 180_000;

// ── Applying a change ────────────────────────────────────────────────────────
//
// Before 2026-10-22 a roll was how any change reached a running agent, and
// most edit paths kept rolling after the render learned to rewrite
// config.yaml in place. A roll restarts the agent: every conversation on it
// waits out the restart, for a toolset checkbox Hermes would have read on
// its next turn anyway.
//
// So an edit APPLIES instead: render, then ask whether the running
// container still matches what the render describes (talaria-fleet-docker's
// `container_fit` — compose's own config hash against the one the container
// was created from, which covers its env, env_file contents, image, mounts,
// limits, and the label carrying the part of config.yaml Hermes reads only
// at start). Only a container that no longer matches is rolled. A stopped
// agent stays stopped; its next start reads the render.
//
// AN APPLY NEVER STARTS AN AGENT. A recreate starts the container it makes,
// whatever state the old one was in, so an apply rolls only a container it
// has SEEN running: a fit docker would not answer is asked again, patiently,
// and one it still will not describe is left alone with a sentence for the
// caller (the change is rendered, not applied). The roll itself reads the
// container once more under the service's lock (`fleet_recreate_running`),
// so a person who stops the agent while the roll renders and pulls is not
// overruled by it.
//
// What still rolls on purpose: the Developer switch (env, image, mounts),
// a key rotation (env), the operator's own roll and restart, the update
// job's fleet roll (a new image), and an MCP timeout or auth-mode edit
// (talaria-mcp-apply), whose effect Hermes only takes from a reconnect.
//
// NO ROLL STARTS AN AGENT A PERSON STOPPED EITHER. Each of those rolls
// recreates a RUNNING agent whatever its fit, and leaves a stopped one as it
// is ([`roll_running_agent`], the same in-lock read an apply's roll makes):
// its next start reads the render — compose recreates a container whose
// service moved, and a plain start boots Hermes on the rewritten config.yaml.
// The one roll that starts stopped agents is the fleet roll a person asks to
// include them in (`/api/fleet/roll` with `all`).

/// Apply a change to one department's agent: render, then roll it only if
/// the render moved its container ([`talaria_fleet_docker::ContainerFit`]).
/// The verdict has [`roll_running_agent`]'s shape: `Ok(None)` when the change is in
/// place (live, or waiting for a stopped agent's next start), `Ok(Some(_))`
/// a roll's soft failure, `Err` a failed step.
pub async fn apply_agent_change(
    pg: &PgPool,
    sb: &SecretBox,
    department: &str,
) -> Result<Option<String>, String> {
    render(pg, sb).await?;
    apply_rendered(pg, sb, department).await
}

/// [`apply_agent_change`] for several departments on ONE render — what a
/// burst of registry edits costs: one render, one fit check per agent, and a
/// roll only where a container moved.
pub async fn apply_agent_changes(
    pg: &PgPool,
    sb: &SecretBox,
    departments: &[String],
) -> Vec<(String, Result<Option<String>, String>)> {
    if let Err(e) = render(pg, sb).await {
        return departments
            .iter()
            .map(|d| (d.clone(), Err(e.clone())))
            .collect();
    }
    apply_after_render(pg, sb, departments).await
}

/// [`apply_agent_changes`] for a caller that has just rendered itself and
/// reported that render's own error (the model cascade): one fit check per
/// agent and a roll only where a container moved. A stopped agent stays
/// stopped — its next start reads the render.
pub async fn apply_after_render(
    pg: &PgPool,
    sb: &SecretBox,
    departments: &[String],
) -> Vec<(String, Result<Option<String>, String>)> {
    let mut out = Vec::with_capacity(departments.len());
    for department in departments {
        out.push((department.clone(), apply_rendered(pg, sb, department).await));
    }
    out
}

/// The fleet-wide form, for an identity edit every soul carries (the org
/// profile): one render, then a roll for each RUNNING agent whose container
/// the render moved — none, for an edit that only rewrote SOUL.md. Answers
/// the agents it rolled and the warnings, like [`roll_running_agents`].
pub async fn apply_running_agents(
    pg: &PgPool,
    sb: &SecretBox,
) -> Result<(Vec<String>, Vec<String>), String> {
    let (_, mut warnings) = render(pg, sb).await?;
    let managed: Vec<(String, String)> = sqlx::query_as(DEPARTMENT_RUNNERS_SQL)
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string())?;
    let mut rolled = Vec::new();
    for (department, display_name) in &managed {
        match apply_one(pg, sb, department).await {
            Applied::Untouched => {}
            Applied::Rolled(Ok(None)) => rolled.push(display_name.clone()),
            Applied::Rolled(Ok(Some(error))) | Applied::Rolled(Err(error)) => warnings.push(error),
            Applied::Unconfirmed(warning) => warnings.push(warning),
        }
    }
    Ok((rolled, warnings))
}

/// What an apply does with a department, from its container's fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyStep {
    /// The running container already matches the render: in place.
    InPlace,
    /// No running container: left as it is, and its next start reads the
    /// render.
    LeaveStopped,
    /// A running container the render moved: roll it.
    Roll,
    /// Docker would not say whether it even runs: start nothing, recreate
    /// nothing, and tell the caller the change is not applied yet.
    Unconfirmed,
}

/// The decision, from the fit [`apply_one`] settled on (after the patient
/// second read, when the first was unknown). Only a container seen running
/// and moved is rolled: a recreate starts the container it makes, so
/// rolling one that is not known to run could start an agent a person
/// stopped.
fn apply_step(fit: talaria_fleet_docker::ContainerFit) -> ApplyStep {
    use talaria_fleet_docker::ContainerFit;
    match fit {
        ContainerFit::Matches => ApplyStep::InPlace,
        ContainerFit::NotRunning => ApplyStep::LeaveStopped,
        ContainerFit::Differs => ApplyStep::Roll,
        ContainerFit::Unknown => ApplyStep::Unconfirmed,
    }
}

/// What applying a change to one department came to.
enum Applied {
    /// In place, or a stopped agent left stopped.
    Untouched,
    /// The roll's verdict ([`roll_running_agent`]'s shape).
    Rolled(Result<Option<String>, String>),
    /// Docker would not describe the container: the sentence for the caller.
    Unconfirmed(String),
}

/// One department against a render that already ran: the fit, read again
/// with docker's patience when the first read got no answer, then
/// [`apply_step`].
async fn apply_one(pg: &PgPool, sb: &SecretBox, department: &str) -> Applied {
    use talaria_fleet_docker::{CONTAINER_FIT_PATIENT, ContainerFit, container_fit_within};
    let mut fit = talaria_fleet_docker::container_fit(pg, department).await;
    if fit == ContainerFit::Unknown {
        fit = container_fit_within(pg, department, CONTAINER_FIT_PATIENT).await;
    }
    match apply_step(fit) {
        ApplyStep::InPlace => {
            tracing::info!(
                "[fleet] {department}: change applied in place — the running container already matches the render"
            );
            Applied::Untouched
        }
        ApplyStep::LeaveStopped => {
            tracing::info!(
                "[fleet] {department}: not running — left as it is; its next start reads the render"
            );
            Applied::Untouched
        }
        ApplyStep::Roll => {
            tracing::info!(
                "[fleet] {department}: the render moved the container (env, image, limits or start-time config) — rolling it"
            );
            Applied::Rolled(roll_agent_mode(pg, sb, department, None, RollMode::IfRunning).await)
        }
        ApplyStep::Unconfirmed => {
            tracing::warn!(
                "[fleet] {department}: docker did not say whether the agent runs — the change is rendered, and nothing was started or recreated"
            );
            Applied::Unconfirmed(unconfirmed_apply(department))
        }
    }
}

/// The sentence an apply answers when docker would not describe the
/// container: what happened, and what lands the change.
fn unconfirmed_apply(department: &str) -> String {
    format!(
        "department \"{department}\": docker did not say whether the agent is running, so the change is saved and rendered but not applied — it lands when the agent next starts, or roll the agent to apply it now"
    )
}

/// [`apply_one`] in [`roll_running_agent`]'s verdict shape.
async fn apply_rendered(
    pg: &PgPool,
    sb: &SecretBox,
    department: &str,
) -> Result<Option<String>, String> {
    match apply_one(pg, sb, department).await {
        Applied::Untouched => Ok(None),
        Applied::Rolled(verdict) => verdict,
        Applied::Unconfirmed(warning) => Ok(Some(warning)),
    }
}

/// Whether a roll recreates whatever it finds, or only a running agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RollMode {
    /// The fleet roll a person asked to include stopped agents in (`all`):
    /// recreate, which starts the agent if it was stopped.
    Always,
    /// Every other roll, and an apply's: recreate only a container still
    /// running when the recreate holds the service's lock; leave any other
    /// as it is.
    IfRunning,
}

/// The mode a fleet-roll batch member is rolled in: a stopped agent is
/// started only when the person asked for stopped agents too.
fn batch_mode(include_stopped: bool) -> RollMode {
    if include_stopped {
        RollMode::Always
    } else {
        RollMode::IfRunning
    }
}

/// What [`roll_agent_locked`] did.
enum Rolled {
    /// Recreated and healthy.
    Done,
    /// A soft failure, with its sentence.
    Soft(String),
    /// [`RollMode::IfRunning`] found the agent not running and left it.
    LeftStopped,
}

/// The roll verdict. `Err` is a failed step (the caller decides what that
/// means); `Ok(Some(error))` is a deliberate soft failure with a sentence
/// for the UI.
///
/// What a roll is FOR: everything a container fixes at creation — its env
/// (a rotated key, the Developer switch), its image, its limits, and the
/// part of config.yaml Hermes reads only at start. An edit that may or may
/// not touch those goes through [`apply_agent_change`], which rolls only
/// when the render moved the container.
///
/// A roll NEVER STARTS AN AGENT (see "Applying a change"): a running agent is
/// recreated whatever its fit; one that is stopped, gone, or that docker will
/// not describe when the recreate holds the service's lock is left as it is,
/// and its next start reads the render. `Ok(None)` either way — the change is
/// in place for a stopped agent too, waiting for its start — and the roll
/// registry records the second as `idle`, not `rolled`.
pub async fn roll_running_agent(
    pg: &PgPool,
    sb: &SecretBox,
    department: &str,
) -> Result<Option<String>, String> {
    roll_agent_mode(pg, sb, department, None, RollMode::IfRunning).await
}

/// One member of a fleet-wide roll batch, its batch stamped on its registry
/// records so the live picture can group them. `include_stopped` is the
/// batch's `all`: the person asked for stopped agents to be rolled — and so
/// started — too; otherwise this is [`roll_running_agent`].
async fn roll_agent_in_batch(
    pg: &PgPool,
    sb: &SecretBox,
    department: &str,
    batch: &str,
    include_stopped: bool,
) -> Result<Option<String>, String> {
    roll_agent_mode(pg, sb, department, Some(batch), batch_mode(include_stopped)).await
}

async fn roll_agent_mode(
    pg: &PgPool,
    sb: &SecretBox,
    department: &str,
    batch: Option<&str>,
    mode: RollMode,
) -> Result<Option<String>, String> {
    // One roll per department at a time. The in-process set answers first
    // (cheap, and what a double-click hits); the Postgres session-level
    // advisory lock serializes across api instances. `try` rather than a
    // blocking wait: a second roll queued behind the first would only roll
    // the fresh container again, so "already rolling" is the honest verdict.
    let Some(_guard) = RollGuard::take(department) else {
        return Ok(Some(format!(
            "department \"{department}\" is already mid-roll — wait for it to finish"
        )));
    };
    let mut conn = pg.acquire().await.map_err(|e| e.to_string())?;
    let key = roll_lock_key(department);
    let locked: (bool,) = sqlx::query_as("select pg_try_advisory_lock(hashtext($1))")
        .bind(&key)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
    if !locked.0 {
        return Ok(Some(format!(
            "department \"{department}\" is already mid-roll on another api instance — wait for it to finish"
        )));
    }
    // Both locks held: this is the roll, and the registry says so from here
    // until the verdict — recorded on every path, so a failed step reaches
    // the page and not only the log.
    let since = now_ms();
    record_roll(RollRecord {
        department: department.to_string(),
        state: RollState::Rolling,
        since,
        updated: since,
        detail: None,
        batch: batch.map(str::to_string),
    });
    let outcome = roll_agent_locked(pg, sb, department, mode).await;
    let (state, detail) = match &outcome {
        Ok(Rolled::Done) => (RollState::Done, None),
        Ok(Rolled::Soft(soft)) => (RollState::Failed, Some(soft.clone())),
        Ok(Rolled::LeftStopped) => (RollState::Idle, None),
        Err(thrown) => (RollState::Failed, Some(thrown.clone())),
    };
    let verdict = match outcome {
        Ok(Rolled::Done | Rolled::LeftStopped) => Ok(None),
        Ok(Rolled::Soft(soft)) => Ok(Some(soft)),
        Err(thrown) => Err(thrown),
    };
    record_roll(RollRecord {
        department: department.to_string(),
        state,
        since,
        updated: now_ms(),
        detail,
        batch: batch.map(str::to_string),
    });
    // A session-level advisory lock outlives the statement that took it and
    // would ride back into the pool with the connection: release it on
    // every path, and if the release itself fails, drop the connection so
    // the pool never hands the held lock to the next checkout.
    let unlocked = sqlx::query("select pg_advisory_unlock(hashtext($1))")
        .bind(&key)
        .execute(&mut *conn)
        .await;
    if let Err(e) = unlocked {
        tracing::warn!("[fleet] roll lock for {department} could not be released: {e}");
        let _ = conn.close().await;
    }
    verdict
}

async fn roll_agent_locked(
    pg: &PgPool,
    sb: &SecretBox,
    department: &str,
    mode: RollMode,
) -> Result<Rolled, String> {
    // Where two agents hold one department (a pair that predates the
    // one-agent rule), the first row is the one the render runs in its
    // container: one with a current version (the render's own condition),
    // then the oldest — talaria-fleet-render's `department_runners`.
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "select d.slug, d.display_name, d.active_slot from agent_defs d \
         where d.department = $1 and d.managed and d.enabled \
         order by exists (select 1 from agent_versions v \
             where v.agent_id = d.id and v.version = d.current_version) desc, \
           d.created_at, d.id",
    )
    .bind(department)
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let Some((slug, display_name, active)) = rows.first().cloned() else {
        return Ok(Rolled::Soft(format!(
            "no managed agent in department \"{department}\""
        )));
    };

    // An agent a blue/green roll left in `agent-<dept>-b` comes home to
    // `agent-<dept>`: point the def back at slot a so the render names the
    // home service (and the manifest dials it), pull the home service's
    // image if the host lacks it, and only then retire the -b container —
    // compose no longer addresses it — the graceful way. The pull comes
    // first because the first roll after a pin bump is the one that pulls
    // gigabytes: done after the retire, the agent would be offline for the
    // whole download, and a failed pull would leave it with no container.
    // The home container starts below, on the same volume, once -b has let
    // go of the gateway lock.
    //
    // A failure before -b stopped puts the def back on b and re-renders, so
    // the agent stays where it was, still serving (a failed stop brings the
    // drained gateway back up). A -b that stopped but would not be removed
    // no longer serves or holds the lock, so the move home goes on and the
    // leftover is named in the log.
    // A legacy agent's move home stops `-b` itself, below, so its recreate
    // is always a plain one: the agent was running when the apply decided
    // to roll it, and the move is what takes it from -b to its home.
    let legacy = active.as_deref() == Some("b");
    // The move home starts the home container whatever `-b` was doing, so a
    // roll that must not start an agent checks `-b` first (and again before
    // the retire, below): one that is not running (or that docker will not
    // describe) stays where it is, and its next start brings it up there on
    // the render.
    if legacy
        && mode == RollMode::IfRunning
        && !talaria_fleet_docker::container_running(&talaria_fleet_docker::slot_container(
            department,
            Slot::B,
        ))
        .await
    {
        tracing::info!(
            "[fleet] {display_name} ({department}): its legacy -b container is not running when its roll reached it — left as it is; its next start reads the render"
        );
        return Ok(Rolled::LeftStopped);
    }
    if legacy {
        let legacy = talaria_fleet_docker::slot_container(department, Slot::B);
        set_active_slot(pg, &slug, "a").await?;
        let ready = match render(pg, sb).await {
            Ok(_) => talaria_fleet_docker::pull_agent_image(pg, department).await,
            Err(e) => Err(e),
        };
        // The read above came before the render and the pull — minutes, for a
        // new pin — so a roll that must not start an agent reads `-b` again,
        // under its service's lock, just before the retire that lets the home
        // container start: one stopped meanwhile stays where it is
        // (`retire_running`). A stop that lands after the def points home
        // addresses the home service instead, and is not seen here.
        let moved = match ready {
            Ok(()) => {
                let retired = if mode == RollMode::IfRunning {
                    talaria_fleet_docker::retire_running(
                        &talaria_fleet_docker::slot_service(department, Slot::B),
                        &legacy,
                    )
                    .await
                } else {
                    talaria_fleet_docker::retire_container(&legacy)
                        .await
                        .map(|()| talaria_fleet_docker::Retired::Done)
                };
                match retired {
                    Ok(talaria_fleet_docker::Retired::Done) => Ok(None),
                    Ok(talaria_fleet_docker::Retired::LeftAlone(seen)) => Ok(Some(seen)),
                    Err(e) if talaria_fleet_docker::container_running(&legacy).await => Err(e),
                    Err(e) => {
                        tracing::warn!(
                            "[fleet] {legacy} stopped but was not removed ({e}) — moving {department} home anyway; remove it by hand"
                        );
                        Ok(None)
                    }
                }
            }
            Err(e) => Err(e),
        };
        match moved {
            Ok(None) => {}
            Ok(Some(seen)) => {
                let _ = set_active_slot(pg, &slug, "b").await;
                let _ = render(pg, sb).await;
                if seen == talaria_fleet_docker::Observed::Unreadable {
                    return Ok(Rolled::Soft(unconfirmed_apply(department)));
                }
                tracing::info!(
                    "[fleet] {display_name} ({department}): its legacy -b container was {} by the time its roll would retire it — left as it is; its next start reads the render",
                    seen.describe()
                );
                return Ok(Rolled::LeftStopped);
            }
            Err(e) => {
                let _ = set_active_slot(pg, &slug, "b").await;
                let _ = render(pg, sb).await;
                return Err(e);
            }
        }
    } else {
        render(pg, sb).await?;
    }

    // The roll itself: pull if the host lacks the image (a failed pull stops
    // here with the running container untouched — for a legacy -b agent the
    // pull already ran above), stop gracefully, recreate. An apply's roll
    // recreates only a container still running under the service's lock.
    if mode == RollMode::IfRunning && !legacy {
        match talaria_fleet_docker::fleet_recreate_running(pg, department).await? {
            talaria_fleet_docker::UpOutcome::Done(_) => {}
            talaria_fleet_docker::UpOutcome::LeftAlone(
                talaria_fleet_docker::Observed::Unreadable,
            ) => return Ok(Rolled::Soft(unconfirmed_apply(department))),
            talaria_fleet_docker::UpOutcome::LeftAlone(seen) => {
                tracing::info!(
                    "[fleet] {display_name} ({department}): the agent was {} when its roll reached it — left as it is; its next start reads the render",
                    seen.describe()
                );
                return Ok(Rolled::LeftStopped);
            }
        }
    } else {
        talaria_fleet_docker::fleet_recreate(pg, department).await?;
    }
    if !talaria_fleet_docker::wait_healthy(pg, department, ROLL_HEALTH_WAIT_MS).await {
        return Ok(Rolled::Soft(format!(
            "{display_name}: the recreated container did not report healthy within {}s — see its logs",
            ROLL_HEALTH_WAIT_MS / 1000
        )));
    }
    Ok(Rolled::Done)
}

/// One render through the wired edge.
async fn render(pg: &PgPool, sb: &SecretBox) -> Result<(usize, Vec<String>), String> {
    let f = RENDER_FLEET
        .get()
        .ok_or_else(|| "fleet render not wired".to_string())?;
    f(pg.clone(), sb.clone()).await
}

async fn set_active_slot(pg: &PgPool, slug: &str, slot: &str) -> Result<(), String> {
    sqlx::query("update agent_defs set active_slot = $2 where slug = $1")
        .bind(slug)
        .bind(slot)
        .execute(pg)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// What [`reconcile_fleet`] did: the render's agent count, the agents it
/// started, the ones already running, and the render's warnings.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileResult {
    pub rendered: usize,
    pub started: Vec<String>,
    pub already_running: Vec<String>,
    pub warnings: Vec<String>,
}

/// One row per department: the managed, enabled agent that runs in its
/// container, slug-ordered. A department is one container, so a loop over
/// these starts or rolls each container once. Where two agents hold one
/// department (a pair that predates the one-agent rule) the row is the one
/// the render runs there — one with a current version first, then the
/// oldest, as talaria-fleet-render's `department_runners` ranks them — and
/// the left-out agent is not reported as started or rolled: it has no
/// container. A department of one agent reads exactly as before.
const DEPARTMENT_RUNNERS_SQL: &str = "select department, display_name from ( \
       select distinct on (d.department) d.department, d.display_name, d.slug \
       from agent_defs d where d.managed and d.enabled \
       order by d.department, \
         exists (select 1 from agent_versions v \
           where v.agent_id = d.id and v.version = d.current_version) desc, \
         d.created_at, d.id \
     ) runner order by slug";

/// Bring the running fleet to the desired state in one shot: re-render every
/// managed agent's config, then start any enabled managed agent whose
/// container isn't running. Reboot survival is already handled by `restart:
/// unless-stopped` on the generated services; this covers drift (an agent
/// enabled/created while Talaria was down, a stopped container, a manifest
/// change).
pub async fn reconcile_fleet(pg: &PgPool, sb: &SecretBox) -> Result<ReconcileResult, String> {
    let render = {
        let f = RENDER_FLEET
            .get()
            .ok_or_else(|| "fleet render not wired".to_string())?;
        f(pg.clone(), sb.clone()).await?
    };
    let managed: Vec<(String, String)> = sqlx::query_as(DEPARTMENT_RUNNERS_SQL)
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string())?;
    let states = if managed.is_empty() {
        Vec::new()
    } else {
        talaria_fleet_docker::container_status(
            &managed.iter().map(|m| m.0.clone()).collect::<Vec<_>>(),
        )
        .await
        .unwrap_or_default()
    };
    let running = |dept: &str| {
        states
            .iter()
            .find(|s| s.department == dept)
            .and_then(|s| s.managed.as_ref())
            .is_some_and(|m| m.state == "running")
    };
    let mut started = Vec::new();
    let mut already_running = Vec::new();
    let mut warnings = render.1.clone();
    for (department, display_name) in &managed {
        // Mid-roll, the department's container is being recreated (and a
        // legacy -b agent moved home); starting its service here would race
        // the roll. The roll's own render covers it.
        if is_rolling(department) {
            already_running.push(display_name.clone());
            continue;
        }
        if running(department) {
            already_running.push(display_name.clone());
            continue;
        }
        match talaria_fleet_docker::fleet_up(pg, department).await {
            Ok(_) => started.push(display_name.clone()),
            Err(e) => warnings.push(format!("{display_name}: {e}")),
        }
    }
    Ok(ReconcileResult {
        rendered: render.0,
        started,
        already_running,
        warnings,
    })
}

/// Render, then ROLL every running managed agent one at a time, whether or
/// not its container moved — the update job's fleet roll (a new image is
/// the point). Agents someone deliberately stopped stay stopped (they read
/// the new render on next start), including one stopped while the job works
/// down the list: each roll reads its container again under the service's
/// lock ([`roll_running_agent`]). An edit uses [`apply_running_agents`].
pub async fn roll_running_agents(
    pg: &PgPool,
    sb: &SecretBox,
) -> Result<(Vec<String>, Vec<String>), String> {
    let render = {
        let f = RENDER_FLEET
            .get()
            .ok_or_else(|| "fleet render not wired".to_string())?;
        f(pg.clone(), sb.clone()).await?
    };
    let managed: Vec<(String, String)> = sqlx::query_as(DEPARTMENT_RUNNERS_SQL)
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string())?;
    let running: HashSet<String> = if managed.is_empty() {
        HashSet::new()
    } else {
        talaria_fleet_docker::running_departments(
            &managed.iter().map(|m| m.0.clone()).collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .collect()
    };
    let mut rolled = Vec::new();
    let mut warnings = render.1;
    for (department, display_name) in &managed {
        if !running.contains(department) {
            continue;
        }
        match roll_running_agent(pg, sb, department).await {
            Ok(None) => rolled.push(display_name.clone()),
            Ok(Some(error)) => warnings.push(error),
            Err(e) => warnings.push(e),
        }
    }
    Ok((rolled, warnings))
}

// ── Roll the whole fleet ─────────────────────────────────────────────────────

/// How many departments roll at once. Each roll is a container stopping and
/// a fresh one booting — a fleet of twenty rolling together would put twenty
/// cold Hermes boots on the host in one breath, and take twenty agents
/// offline at once.
pub fn roll_concurrency() -> usize {
    std::env::var("TALARIA_FLEET_ROLL_CONCURRENCY")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(2)
}

/// Why a department was left out of a batch.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollSkip {
    pub slug: String,
    pub why: String,
}

/// What `start_roll_batch` answers before any roll has moved: the agents it
/// queued and the ones it left alone.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollBatch {
    pub batch: String,
    pub rolling: Vec<String>,
    pub skipped: Vec<RollSkip>,
}

/// One roll of a batch that did not land — handed to the caller's hook, which
/// is where the audit entry gets written (this crate has no audit of its own).
#[derive(Debug, Clone)]
pub struct RollFailure {
    pub batch: String,
    pub agent_id: String,
    pub slug: String,
    pub department: String,
    pub display_name: String,
    pub error: String,
}

pub type RollFailureHook = Arc<dyn Fn(RollFailure) -> BoxFuture<'static, ()> + Send + Sync>;

/// Decide the batch — managed, enabled, currently running (every managed
/// enabled agent with `all`), minus departments already mid-roll — and return
/// at once; the rolls themselves run detached, `roll_concurrency()` at a time
/// in roster order. Each roll goes through `roll_agent_in_batch`, so the
/// registry carries its start and its verdict, and a failed one also reaches
/// `on_failed`.
pub async fn start_roll_batch(
    pg: &PgPool,
    sb: &SecretBox,
    batch: &str,
    all: bool,
    on_failed: RollFailureHook,
) -> Result<RollBatch, String> {
    let managed: Vec<(String, String, String, String)> = sqlx::query_as(
        "select id::text, slug, department, display_name from agent_defs \
         where managed and enabled order by slug",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let running: HashSet<String> = if managed.is_empty() {
        HashSet::new()
    } else {
        talaria_fleet_docker::running_departments(
            &managed.iter().map(|m| m.2.clone()).collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .collect()
    };
    let mut plan = RollBatch {
        batch: batch.to_string(),
        rolling: Vec::new(),
        skipped: Vec::new(),
    };
    let mut queue: Vec<(String, String, String, String)> = Vec::new();
    for (id, slug, department, display_name) in managed {
        if is_rolling(&department) {
            plan.skipped.push(RollSkip {
                slug,
                why: "already rolling".into(),
            });
            continue;
        }
        if !all && !running.contains(&department) {
            plan.skipped.push(RollSkip {
                slug,
                why: "not running".into(),
            });
            continue;
        }
        plan.rolling.push(slug.clone());
        queue.push((id, slug, department, display_name));
    }
    // Queued members show on the page from the moment the batch exists —
    // "rolling" with a note, since nothing else they could honestly be is
    // "idle". The roll's own start record replaces the note.
    let queued_at = now_ms();
    for (_, _, department, _) in &queue {
        record_roll(RollRecord {
            department: department.clone(),
            state: RollState::Rolling,
            since: queued_at,
            updated: queued_at,
            detail: Some("queued".into()),
            batch: Some(batch.to_string()),
        });
    }
    let semaphore = Arc::new(tokio::sync::Semaphore::new(roll_concurrency()));
    for (agent_id, slug, department, display_name) in queue {
        let pg = pg.clone();
        let sb = sb.clone();
        let batch = batch.to_string();
        let on_failed = on_failed.clone();
        let semaphore = semaphore.clone();
        tokio::spawn(async move {
            // A closed semaphore is impossible here (nobody closes it); a
            // failed acquire would only mean the runtime is going down.
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return;
            };
            let error = match roll_agent_in_batch(&pg, &sb, &department, &batch, all).await {
                Ok(None) => return,
                Ok(Some(soft)) => soft,
                Err(thrown) => thrown,
            };
            tracing::warn!("[fleet] roll of {display_name} ({department}) failed: {error}");
            on_failed(RollFailure {
                batch,
                agent_id,
                slug,
                department,
                display_name,
                error,
            })
            .await;
        });
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(department: &str, state: RollState, updated: i64) -> RollRecord {
        RollRecord {
            department: department.into(),
            state,
            since: updated,
            updated,
            detail: None,
            batch: None,
        }
    }

    #[test]
    fn registry_records_start_and_verdict_and_prunes_old_outcomes() {
        let now = now_ms();
        record_roll(rec("reg-a", RollState::Rolling, now));
        record_roll(RollRecord {
            detail: Some("x: the recreated container did not report healthy".into()),
            ..rec("reg-a", RollState::Failed, now)
        });
        // An outcome older than the window is gone on the next read; a
        // roll still in flight is never aged out, however old its start.
        record_roll(rec(
            "reg-old",
            RollState::Done,
            now - ROLL_OUTCOME_TTL_MS - 1,
        ));
        record_roll(rec(
            "reg-stuck",
            RollState::Rolling,
            now - ROLL_OUTCOME_TTL_MS - 1,
        ));
        let records = roll_records();
        let a = records
            .iter()
            .find(|r| r.department == "reg-a")
            .expect("the verdict is recorded");
        assert_eq!(a.state, RollState::Failed);
        assert_eq!(
            a.detail.as_deref(),
            Some("x: the recreated container did not report healthy")
        );
        assert!(!records.iter().any(|r| r.department == "reg-old"));
        assert!(records.iter().any(|r| r.department == "reg-stuck"));
    }

    #[test]
    fn merge_prefers_the_newer_record_per_department() {
        let now = now_ms();
        let local = vec![
            rec("m-a", RollState::Rolling, now - 10),
            rec("m-b", RollState::Done, now),
        ];
        let remote = vec![
            RollRecord {
                detail: Some("thrown".into()),
                ..rec("m-a", RollState::Failed, now)
            },
            rec("m-b", RollState::Rolling, now - 500),
            rec("m-c", RollState::Done, now - ROLL_OUTCOME_TTL_MS - 1),
        ];
        let merged = merge_roll_records(local, remote);
        let states: Vec<(String, RollState)> = merged
            .iter()
            .map(|r| (r.department.clone(), r.state))
            .collect();
        assert_eq!(
            states,
            vec![
                ("m-a".into(), RollState::Failed),
                ("m-b".into(), RollState::Done)
            ]
        );
        assert_eq!(merged[0].detail.as_deref(), Some("thrown"));
    }

    #[test]
    fn an_apply_rolls_only_a_container_it_saw_running_and_moved() {
        use talaria_fleet_docker::ContainerFit;
        assert_eq!(apply_step(ContainerFit::Matches), ApplyStep::InPlace);
        assert_eq!(apply_step(ContainerFit::Differs), ApplyStep::Roll);
        // A stopped agent stays stopped: its next start reads the render.
        assert_eq!(
            apply_step(ContainerFit::NotRunning),
            ApplyStep::LeaveStopped
        );
        // Docker would not say (twice, by the time this is asked): never a
        // roll, which would start a container a person stopped.
        assert_eq!(apply_step(ContainerFit::Unknown), ApplyStep::Unconfirmed);
        let sentence = unconfirmed_apply("ops");
        assert!(sentence.contains("\"ops\""), "{sentence}");
        assert!(sentence.contains("not applied"), "{sentence}");
    }

    #[test]
    fn only_a_fleet_roll_asked_to_include_stopped_agents_starts_one() {
        // `all`: the person ticked "also start and roll agents that are
        // stopped".
        assert_eq!(batch_mode(true), RollMode::Always);
        // Every other batch member is rolled only if it still runs when its
        // recreate holds the service's lock.
        assert_eq!(batch_mode(false), RollMode::IfRunning);
    }

    #[test]
    fn concurrency_defaults_to_two_and_refuses_zero() {
        // The env var is process-global; this test only pins the parser's
        // fallback, which `roll_concurrency` reads when the var is unset or
        // unparseable. The default is what a fleet of twenty is paced by.
        let parse = |v: &str| {
            v.trim()
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .unwrap_or(2)
        };
        assert_eq!(parse("0"), 2);
        assert_eq!(parse("x"), 2);
        assert_eq!(parse(" 4 "), 4);
        assert!(roll_concurrency() >= 1);
    }
}
