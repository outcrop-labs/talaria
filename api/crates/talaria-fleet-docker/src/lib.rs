// Docker control for the managed fleet — Talaria drives `docker compose` on
// the rendered fleet/docker-compose.yml (interpolation env = the fleet's own
// .env, so per-agent keys/secrets stay in one Talaria-owned, gitignored
// place).
//
// Covers service naming, the external network guarantee, the lifecycle verbs
// (up, recreate, stop, restart, remove), the image pull, the healthcheck
// wait, the fit check that decides whether a change needs a recreate at all
// (`container_fit`), the 5s-cached `docker ps` status the roster and Home
// poll, and the shared `docker exec` wrapper that reaches into a running
// agent: the memory surface (talaria-agent-memory), the lost-turn discard
// (`lost_turns.rs`) and the workbench's jobs, dev env and teardown
// (talaria-workbench-mcp). Bring-up itself runs no post-start exec step:
// Hermes keeps replaced bundled skills off by itself (talaria-hermes-skills),
// and its image already carries the browser the toolset drives.
//
// ONE CONTAINER PER AGENT. Each agent runs as `agent-<dept>`, and a roll
// recreates that container in place (`fleet_recreate`). There is no second
// slot to roll into: every Hermes since v2026.4.30 takes an `flock` on
// `/opt/data/gateway.lock` in the agent's state volume, so a second container
// on the same volume exits at start and never serves. Before 2026-10-22 a
// blue/green roll could leave an agent in `agent-<dept>-b` (`active_slot =
// 'b'`); callers still pass a department and the service resolves here, so
// such an agent keeps working until its next roll moves it back.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

mod lost_turns;
pub use lost_turns::discard_lost_turns;

/// Set from the api binary so `fleet_up` can run preflight without this crate
/// depending on the rest of the fleet engine.
pub static PREFLIGHT: std::sync::OnceLock<fn(sqlx::PgPool)> = std::sync::OnceLock::new();

use sqlx::PgPool;

/// How long a stop may drain before docker SIGKILLs the agent — the
/// `stop_grace_period` the render gives every service, the timeout this
/// crate's own stops pass, and the bound on `drain_gateway`'s wait. On
/// SIGTERM Hermes keeps cron and `/v1` runs open for up to
/// `agent.cron_drain_timeout` (30s) plus a 10s cleanup reserve; docker's
/// default 10s would cut them off mid-write.
pub const AGENT_STOP_GRACE_SECS: u64 = 45;

/// The agent gateway's s6 service in the Hermes image (`hermes gateway run`
/// registers it as `gateway-<profile>` under the dynamic scandir), and where
/// the image keeps the s6 tools — not on `docker exec`'s PATH.
const GATEWAY_SERVICE_DIR: &str = "/run/service/gateway-default";
const S6_BIN: &str = "/command";

/// Which compose service an agent lives in. `A` is `agent-<dept>`, the only
/// one anything creates now. `B` (`agent-<dept>-b`) is where a blue/green
/// roll from before 2026-10-22 could leave an agent; its next roll moves it
/// back. The app updater's own slot pair reuses this enum
/// (talaria-update-layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

/// The compose service name for a department's slot.
pub fn slot_service(department: &str, slot: Slot) -> String {
    format!(
        "agent-{}{}",
        department,
        if slot == Slot::B { "-b" } else { "" }
    )
}

/// The container docker actually names for that service (compose project +
/// service + replica index).
pub fn slot_container(department: &str, slot: Slot) -> String {
    format!(
        "{}-{}-1",
        talaria_fleet_layout::fleet_project(),
        slot_service(department, slot)
    )
}

/// Which service a department's managed agent currently runs in, from the
/// def row. Missing row / null reads as 'a'; only an agent a pre-2026-10-22
/// blue/green roll left behind reads 'b', until its next roll.
pub async fn active_slot(pg: &PgPool, department: &str) -> Slot {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "select active_slot from agent_defs where department = $1 and managed limit 1",
    )
    .bind(department)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    match row.unwrap_or_default().0.as_deref() {
        Some("b") => Slot::B,
        _ => Slot::A,
    }
}

/// One docker CLI invocation: non-zero exit is an error carrying stderr,
/// success returns (stdout, stderr) trimmed as written. Crate-visible —
/// the app's own update engine (api/src/update/) speaks docker through
/// the same shape rather than a second copy of it.
pub async fn docker(args: &[&str], timeout: Duration) -> Result<(String, String), String> {
    let out = tokio::time::timeout(timeout, async {
        // kill_on_drop: a timed-out `docker compose up` or `docker exec`
        // must die with the future that was waiting on it, not outlive its
        // own deadline as an orphan CLI process.
        tokio::process::Command::new("docker")
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| format!("docker {} …: {e}", args.first().unwrap_or(&"")))
    })
    .await
    .map_err(|_| {
        format!(
            "docker {} …: timed out after {}s",
            args.first().unwrap_or(&""),
            timeout.as_secs()
        )
    })??;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "docker {} failed: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        ));
    }
    Ok((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn compose_args(extra: &[&str]) -> Vec<String> {
    let mut args = vec![
        "compose".to_string(),
        "-p".to_string(),
        talaria_fleet_layout::fleet_project(),
        "-f".to_string(),
        talaria_fleet_layout::fleet_dir()
            .join("docker-compose.yml")
            .to_string_lossy()
            .into_owned(),
        "--env-file".to_string(),
        talaria_fleet_layout::fleet_env()
            .to_string_lossy()
            .into_owned(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

async fn ensure_fleet_network() -> Result<(), String> {
    let name = talaria_fleet_layout::fleet_network_name().await;
    let name = name.as_str();
    if docker(&["network", "inspect", name], Duration::from_secs(10))
        .await
        .is_ok()
    {
        return Ok(());
    }
    if docker(&["network", "create", name], Duration::from_secs(10))
        .await
        .is_ok()
    {
        return Ok(());
    }
    // lost a create race, or a real failure — only the latter should throw
    docker(&["network", "inspect", name], Duration::from_secs(10))
        .await
        .map(|_| ())
}

/// Bring a department's managed agent up. Answers with compose's stderr
/// (trimmed), which on success carries its progress/warnings — the run logs
/// nothing of it.
pub async fn fleet_up(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_up(pg, department, &["up", "-d"], verb_timeout(120)).await
}

/// Recreate a department's managed agent IN PLACE — the roll. Compose stops
/// the running container (SIGTERM, then `stop_grace_period` before a KILL),
/// then creates and starts a fresh one from the current render on the same
/// state volume. `--no-deps`: an agent has none, and a roll must never touch
/// another service. The image is pulled first when the host lacks it, so a
/// pull that fails leaves the running container exactly as it was. A
/// recreate that fails or runs out its time cleans up after itself
/// (`recover_failed_recreate`).
pub async fn fleet_recreate(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_up(
        pg,
        department,
        &["up", "-d", "--force-recreate", "--no-deps"],
        verb_timeout(AGENT_STOP_GRACE_SECS + 120),
    )
    .await
}

/// How long an agent's compose verb (up, recreate, stop, restart, remove)
/// may run before its CLI is killed: `default_secs`, unless
/// `TALARIA_FLEET_COMPOSE_TIMEOUT_SECS` names another deadline for all of
/// them — longer, for a host whose disk is slow enough that healthy verbs
/// need it; shorter, to drive the timeout paths on purpose (a killed
/// recreate's cleanup, a failed verb answered by the container's state).
/// The image pull and the cleanup's own steps keep their deadlines.
fn verb_timeout(default_secs: u64) -> Duration {
    Duration::from_secs(verb_timeout_secs(
        std::env::var("TALARIA_FLEET_COMPOSE_TIMEOUT_SECS")
            .ok()
            .as_deref(),
        default_secs,
    ))
}

fn verb_timeout_secs(env: Option<&str>, default_secs: u64) -> u64 {
    env.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default_secs)
}

/// The label compose stamps on every container it creates: a digest of the
/// service definition it created it from — image, environment (with each
/// `${VAR}` interpolated and every `env_file` read in), mounts, limits and
/// labels, the render's start-config label among them. `docker compose up`
/// recreates exactly when the rendered service's digest differs from this.
pub const CONFIG_HASH_LABEL: &str = "com.docker.compose.config-hash";

/// Whether a department's running container is what the current render
/// describes — the question `apply_agent_change` (fleet_reconcile) asks
/// before it recreates anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFit {
    /// Created from exactly the rendered service: everything that changed
    /// reached it in place, and a recreate would only interrupt it.
    Matches,
    /// Created from something else — or the container could not be
    /// inspected, or the digest could not be read on either side, which is
    /// read as a change: a recreate it did not need costs a restart, a
    /// skipped one strands the change.
    Differs,
    /// No running container: its next start reads the render.
    NotRunning,
}

/// What `docker inspect` said of a department's container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inspected {
    /// Docker answered that there is no such container.
    Absent,
    /// The inspect itself failed — a timeout, a daemon error — so nothing is
    /// known about the container, running or not.
    Unreadable,
    /// It exists: whether it runs, and the [`CONFIG_HASH_LABEL`] it carries.
    Seen { running: bool, hash: Option<String> },
}

/// Read a `docker inspect -f '{{.State.Running}} {{index .Config.Labels …}}'`
/// result. Only docker's own "no such object" is an absent container; any
/// other failure is [`Inspected::Unreadable`], never taken for one.
pub fn parse_inspect(result: &Result<(String, String), String>) -> Inspected {
    match result {
        Ok((out, _)) => {
            let mut parts = out.split_whitespace();
            Inspected::Seen {
                running: parts.next() == Some("true"),
                hash: parts.next().map(str::to_string),
            }
        }
        Err(e) => {
            let e = e.to_ascii_lowercase();
            if e.contains("no such object") || e.contains("no such container") {
                Inspected::Absent
            } else {
                Inspected::Unreadable
            }
        }
    }
}

/// The fit, from what `docker inspect` said of the container and the digest
/// compose computes for the rendered service. An inspect that failed is read
/// as a change, like an unreadable digest: the roll the caller used to do.
pub fn fit_of(container: &Inspected, rendered: Option<&str>) -> ContainerFit {
    match container {
        Inspected::Absent | Inspected::Seen { running: false, .. } => ContainerFit::NotRunning,
        Inspected::Unreadable => ContainerFit::Differs,
        Inspected::Seen {
            running: true,
            hash,
        } => match (hash.as_deref(), rendered) {
            (Some(have), Some(want)) if !have.is_empty() && have == want => ContainerFit::Matches,
            _ => ContainerFit::Differs,
        },
    }
}

/// The digest `docker compose config --hash <svc>` printed for `svc`: one
/// `<service> <hash>` line per service asked for.
pub fn parse_config_hash(stdout: &str, service: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next(), parts.next()) {
            (Some(svc), Some(hash), None) if svc == service && !hash.is_empty() => {
                Some(hash.to_string())
            }
            _ => None,
        }
    })
}

/// Ask compose and the container itself whether the render moved the
/// service under a running agent ([`ContainerFit`]). Compose's own digest is
/// the comparison because compose's own digest is what a recreate would be
/// decided by; nothing here re-derives it.
pub async fn container_fit(pg: &PgPool, department: &str) -> ContainerFit {
    let slot = active_slot(pg, department).await;
    let svc = slot_service(department, slot);
    let container = slot_container(department, slot);
    let answer = docker(
        &[
            "inspect",
            "-f",
            &format!("{{{{.State.Running}}}} {{{{index .Config.Labels \"{CONFIG_HASH_LABEL}\"}}}}"),
            &container,
        ],
        Duration::from_secs(10),
    )
    .await;
    let inspected = parse_inspect(&answer);
    match &inspected {
        Inspected::Absent | Inspected::Seen { running: false, .. } => {
            return ContainerFit::NotRunning;
        }
        Inspected::Unreadable => {
            tracing::warn!(
                "[fleet] {container}: could not inspect the container ({}) — treating it as changed, so the edit rolls it",
                answer.err().unwrap_or_default()
            );
            return ContainerFit::Differs;
        }
        Inspected::Seen { running: true, .. } => {}
    }
    let args = compose_args(&["config", "--hash", &svc]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let rendered = match docker(&refs, Duration::from_secs(30)).await {
        Ok((out, _)) => parse_config_hash(&out, &svc),
        Err(e) => {
            tracing::warn!(
                "[fleet] {svc}: could not read the rendered service's config hash ({e}) — treating it as changed"
            );
            None
        }
    };
    fit_of(&inspected, rendered.as_deref())
}

// ── What a container really did ─────────────────────────────────────────────
//
// A compose verb's exit is not the container's state. A CLI killed at its
// deadline leaves the daemon finishing what it had asked for — a stop runs
// out its grace period and lands, minutes after the caller was told it
// failed — and a compose that exits on one error can leave another of its
// requests running. Measured on docker 29.8.1 / compose v5.5.1: a
// `--force-recreate` killed mid-stop answered 137, and the old container
// exited at the end of its grace period all the same. So a caller with a
// failed verb in hand re-reads the container and answers by what it is.

/// What docker says one container is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observed {
    /// Running: its id and docker's own `StartedAt` stamp, which moves on
    /// every start — how a restart that did happen is told from one that
    /// did not.
    Running { id: String, started_at: String },
    /// It exists and is not running: created, exited, dead.
    Stopped { id: String },
    /// Docker says there is no such container.
    Absent,
    /// The inspect itself failed — a timeout, a daemon error — so nothing is
    /// known.
    Unreadable,
}

impl Observed {
    /// The sentence a log line carries.
    pub fn describe(&self) -> &'static str {
        match self {
            Observed::Running { .. } => "running",
            Observed::Stopped { .. } => "stopped",
            Observed::Absent => "gone",
            Observed::Unreadable => "unreadable (docker did not answer)",
        }
    }
}

/// The `docker inspect` format [`parse_observed`] reads.
const OBSERVE_FORMAT: &str = "{{.Id}} {{.State.Running}} {{.State.StartedAt}}";

/// Read a `docker inspect -f OBSERVE_FORMAT` result. Like [`parse_inspect`],
/// only docker's own "no such object" is an absent container.
pub fn parse_observed(result: &Result<(String, String), String>) -> Observed {
    match result {
        Ok((out, _)) => {
            let mut parts = out.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(id), Some("true")) => Observed::Running {
                    id: id.to_string(),
                    started_at: parts.next().unwrap_or_default().to_string(),
                },
                (Some(id), Some(_)) => Observed::Stopped { id: id.to_string() },
                _ => Observed::Unreadable,
            }
        }
        Err(_) => match parse_inspect(result) {
            Inspected::Absent => Observed::Absent,
            _ => Observed::Unreadable,
        },
    }
}

/// Patient on purpose: this read follows a verb that failed, very often
/// because the daemon is slow — an inspect took 35 s on this host while
/// another session's build saturated the disk, and a 10 s read would only
/// have said "unknown" for the whole window.
async fn observe_container(container: &str) -> Observed {
    parse_observed(
        &docker(
            &["inspect", "-f", OBSERVE_FORMAT, container],
            Duration::from_secs(30),
        )
        .await,
    )
}

/// What a department's managed container is doing right now.
pub async fn observe(pg: &PgPool, department: &str) -> Observed {
    observe_container(&managed_container(pg, department).await).await
}

/// How long to keep re-reading a container after a compose verb failed. A
/// verb that ran out its deadline was killed with the daemon still at work —
/// a stop can need its whole grace period after that — so it gets the grace
/// period and a margin; any other failure gets a few reads, for the request
/// compose may have left running when it exited.
pub fn settle_window(error: &str) -> Duration {
    if error.contains("timed out after") {
        Duration::from_secs(AGENT_STOP_GRACE_SECS + 5)
    } else {
        Duration::from_secs(6)
    }
}

/// After a compose verb on a department failed with `error`: re-read its
/// container every two seconds, for [`settle_window`], until `reached` says
/// it is what the verb asked for. Answers the last reading.
pub async fn settle(
    pg: &PgPool,
    department: &str,
    error: &str,
    reached: impl Fn(&Observed) -> bool,
) -> Observed {
    let deadline = tokio::time::Instant::now() + settle_window(error);
    loop {
        let seen = observe(pg, department).await;
        if reached(&seen) || tokio::time::Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// After an `up` failed with `error`: whether the department's agent is
/// running the current render all the same — its container
/// [`ContainerFit::Matches`] within [`settle_window`]: a create and start
/// the daemon finished for a CLI that did not wait.
pub async fn up_landed(pg: &PgPool, department: &str, error: &str) -> bool {
    let deadline = tokio::time::Instant::now() + settle_window(error);
    loop {
        if container_fit(pg, department).await == ContainerFit::Matches {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// ── A recreate a killed CLI left half done ──────────────────────────────────
//
// Compose recreates a container by creating the new one as
// `<first 12 of the old id>_<name>`, then stopping and removing the old one,
// then renaming the new one and starting it. A CLI killed between the create
// and the rename — a roll that ran out its time — leaves that new container
// `created`, never started. It is not harmless: the next recreate of the
// service treats it as the container to replace, creates yet another, asks
// the daemon to stop the RUNNING agent, and then fails on the name
// (measured on compose v5.5.1: "Conflict. The container name … is already
// in use"), leaving the agent stopped and one more stranded container.

/// Whether `name` is the temporary name compose gives a recreate's new
/// container for `container`.
pub fn is_stranded_name(name: &str, container: &str) -> bool {
    name.strip_suffix(container)
        .and_then(|prefix| prefix.strip_suffix('_'))
        .is_some_and(|id| id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Remove the containers a half-done recreate stranded for this service:
/// this fleet's compose project and this service by label, never started
/// (`status=created`), under the temporary name [`is_stranded_name`] reads.
/// Nothing that ever ran is touched, and `docker rm` without `-f` refuses a
/// running container anyway. Answers the names it removed.
async fn clear_stranded(svc: &str, container: &str) -> Vec<String> {
    let project = talaria_fleet_layout::fleet_project();
    let listed = docker(
        &[
            "ps",
            "-a",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
            "--filter",
            &format!("label=com.docker.compose.service={svc}"),
            "--filter",
            "status=created",
            "--format",
            "{{.ID}}\t{{.Names}}",
        ],
        Duration::from_secs(20),
    )
    .await;
    let Ok((out, _)) = listed else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for (id, name) in out.lines().filter_map(|l| l.split_once('\t')) {
        if !is_stranded_name(name.trim(), container) {
            continue;
        }
        match docker(&["rm", id.trim()], Duration::from_secs(20)).await {
            Ok(_) => {
                tracing::warn!(
                    "[fleet] {svc}: removed {} — a recreate that did not finish left it created and never started",
                    name.trim()
                );
                removed.push(name.trim().to_string());
            }
            Err(e) => tracing::warn!(
                "[fleet] {svc}: could not remove the stranded {} ({e})",
                name.trim()
            ),
        }
    }
    removed
}

/// One compose operation per service at a time, in this process: a cleanup
/// of stranded containers must never take the temporary container of a
/// recreate that is still running, and two verbs on one agent at once (a
/// stop during a roll) only race each other. The roll's own lock
/// (fleet-reconcile) covers rolls across api instances.
static SERVICE_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

async fn lock_service(svc: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = SERVICE_LOCKS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry(svc.to_string())
        .or_default()
        .clone();
    lock.lock_owned().await
}

/// An `up` that would replace a running container failed — compose's error,
/// or its deadline. What it leaves depends on how far it got. A stranded
/// replacement (`stranded`) means compose had created the new container, so
/// it may have asked the daemon to stop the old one, and that stop lands
/// whether or not anyone waits for it: wait it out (the grace period and a
/// margin). Then answer by what is there:
/// - the old container still running: it serves on its old render, and the
///   change did not land (`Err`; the next apply rolls it again);
/// - another container running under the name: compose got as far as the
///   start, so the render landed (`Ok`);
/// - nothing running: the stop went through — start the service on the
///   current render, which completes what was asked (`Ok`), or say that
///   failed too.
async fn recover_failed_recreate(
    svc: &str,
    container: &str,
    old_id: &str,
    stranded: bool,
    error: String,
) -> Result<String, String> {
    let wait = if stranded {
        Duration::from_secs(AGENT_STOP_GRACE_SECS + 10)
    } else {
        Duration::ZERO
    };
    let deadline = tokio::time::Instant::now() + wait;
    let settled = loop {
        let seen = observe_container(container).await;
        let still_old = match &seen {
            Observed::Running { id, .. } => id == old_id,
            Observed::Unreadable => true,
            Observed::Stopped { .. } | Observed::Absent => false,
        };
        if !still_old || tokio::time::Instant::now() >= deadline {
            break seen;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    match settled {
        Observed::Running { id, .. } if id != old_id => {
            tracing::warn!(
                "[fleet] {svc}: the recreate reported a failure ({error}), but its new container is running — the render landed"
            );
            Ok(String::new())
        }
        Observed::Running { .. } => Err(format!(
            "{error} — the agent kept running on its previous container"
        )),
        Observed::Unreadable => Err(format!(
            "{error} — and docker could not say whether the agent is still running"
        )),
        Observed::Stopped { .. } | Observed::Absent => {
            clear_stranded(svc, container).await;
            let args = compose_args(&["up", "-d", "--no-deps", svc]);
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            match docker(&refs, Duration::from_secs(120)).await {
                Ok((_, stderr)) => {
                    tracing::warn!(
                        "[fleet] {svc}: the recreate failed ({error}) after its stop went through — started the agent again on the current render"
                    );
                    Ok(stderr.trim().to_string())
                }
                Err(e) => {
                    clear_stranded(svc, container).await;
                    Err(format!(
                        "{error} — the agent stopped, and starting it again failed too: {e}"
                    ))
                }
            }
        }
    }
}

/// Stop the agent's gateway THROUGH ITS OWN SUPERVISOR, and wait for it to
/// finish, before anything stops its container. A plain `docker stop` cannot
/// drain a Hermes agent, whatever `stop_grace_period` says: the gateway is a
/// dynamic s6 service, so it gets its SIGTERM only in s6-overlay's final
/// shutdown phase, exits 1 ("so the service manager can revive" it), is
/// revived, and everything is SIGKILLed `S6_KILL_GRACETIME` (3s) later.
/// Measured on v0.22.4: `docker stop -t 45` returned in ~4s and cut an
/// in-flight `/v1/chat/completions` off. `s6-svc -d` marks the service down
/// first — s6 sends the SIGTERM and will not revive it — so Hermes runs its
/// own drain (cron and `/v1` runs held up to 30s, plus its cleanup reserve)
/// and exits; `s6-svwait -D` returns when it has. The same measurement with
/// this first: the turn finished (200, the whole reply) and the container
/// then stopped cleanly. Hermes reads a SIGTERM it did not plan as
/// "unexpected" and records the gateway as running, so the next container
/// on this volume starts it as usual. Best effort: a container that is not
/// running, or an image laid out differently, falls through to the plain
/// stop it would have had anyway.
async fn drain_gateway(container: &str) {
    let script = format!(
        "{S6_BIN}/s6-svc -d {GATEWAY_SERVICE_DIR} && \
         {S6_BIN}/s6-svwait -D -t {} {GATEWAY_SERVICE_DIR}",
        (AGENT_STOP_GRACE_SECS - 5) * 1000
    );
    if let Err(e) = docker_exec(
        container,
        &["sh", "-c", &script],
        AGENT_STOP_GRACE_SECS * 1000,
    )
    .await
    {
        tracing::debug!("[fleet] gateway drain in {container} skipped: {e}");
    }
}

/// Undo [`drain_gateway`] when the stop it cleared the way for did not
/// happen — a compose or `docker stop` that failed (or timed out) before it
/// reached the container. s6 will not revive a service marked down, so
/// without this the container keeps running with no gateway until someone
/// restarts it. `s6-svc -u` brings it back up. Best effort, and harmless
/// where there is nothing to undo: a container that did stop answers the
/// exec with an error, and a gateway that is already up stays up.
async fn undrain_gateway(container: &str) {
    let script = format!("{S6_BIN}/s6-svc -u {GATEWAY_SERVICE_DIR}");
    if let Err(e) = docker_exec(container, &["sh", "-c", &script], 15_000).await {
        tracing::debug!("[fleet] gateway undrain in {container} skipped: {e}");
    }
}

/// `compose up` for a department's service, with the network ensured and the
/// image pulled first. The up's own timeout covers the stop and the create;
/// the pull has its own, much longer one — a new Hermes pin is gigabytes.
async fn compose_up(
    pg: &PgPool,
    department: &str,
    verb: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    ensure_fleet_network().await?;
    let slot = active_slot(pg, department).await;
    let svc = slot_service(department, slot);
    let container = slot_container(department, slot);
    pull_missing(&svc).await?;
    let _held = lock_service(&svc).await;
    // A container an earlier recreate stranded would make this one stop the
    // agent and then fail on the name: clear it first, so the next apply
    // after a timed-out roll recovers instead of repeating it.
    clear_stranded(&svc, &container).await;
    // The container serving now, if any — what a failed `up` that got as
    // far as replacing it is measured against ([`recover_failed_recreate`]).
    let replaced = match observe_container(&container).await {
        Observed::Running { id, .. } => Some(id),
        _ => None,
    };
    // A recreate stops the running container: drain it first. A plain `up`
    // starts one and leaves a running one alone, so it has nothing to drain.
    let recreate = verb.contains(&"--force-recreate");
    if recreate {
        drain_gateway(&container).await;
    }
    let mut args: Vec<String> = compose_args(verb);
    args.push(svc.clone());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (_, stderr) = match docker(&refs, timeout).await {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!("[fleet] {svc}: compose {} failed: {e}", verb.join(" "));
            let stranded = !clear_stranded(&svc, &container).await.is_empty();
            if recreate {
                undrain_gateway(&container).await;
            }
            // Only a container that was serving is brought back: an `up` or
            // a roll of a stopped agent that failed leaves it stopped. (A
            // plain `up` replaces a running container too, when the render
            // moved its service.)
            match &replaced {
                Some(old_id) => {
                    recover_failed_recreate(&svc, &container, old_id, stranded, e).await?;
                }
                None => return Err(e),
            }
            (String::new(), String::new())
        }
    };
    // Bringing an agent up is the moment to ask whether it can reach us — from
    // the fleet network, not from here. Detached: a preflight must never be
    // able to fail a start, and its whole value is that it writes a verdict
    // somebody reads later (alerts) rather than that this caller waits for it.
    let pool = pg.clone();
    if let Some(pf) = PREFLIGHT.get() {
        pf(pool);
    }
    Ok(stderr.trim().to_string())
}

/// Pull the service's image if — and only if — the host does not have it.
/// The chassis pins Hermes by digest, so this is what moves a host onto a
/// new pin the first time an agent starts or rolls after the bump, and a
/// no-op every time after. A tag an operator set instead (`HERMES_IMAGE`)
/// is pulled once and then kept as cached: re-pull it yourself, or pin a
/// digest. Done as its own step, ahead of `up`, so a registry problem fails
/// here — with the running container untouched — and not halfway through a
/// recreate.
async fn pull_missing(svc: &str) -> Result<(), String> {
    let args = compose_args(&["pull", "--policy", "missing", "--quiet", svc]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match docker(&refs, Duration::from_secs(1_200)).await {
        Ok(_) => Ok(()),
        // A compose too old for `--policy` (before v2.22): `up` still pulls a
        // missing image itself, just without the early, clean failure.
        Err(e) if e.contains("unknown flag") => Ok(()),
        Err(e) => Err(format!("could not pull the image for {svc}: {e}")),
    }
}

/// [`pull_missing`] for a department's current service, on its own — for a
/// caller that must have the image on the host BEFORE it stops anything
/// compose does not stop for it: the roll that moves a legacy `-b` agent
/// home retires `-b` itself, so it pulls first, while `-b` still serves.
pub async fn pull_agent_image(pg: &PgPool, department: &str) -> Result<(), String> {
    pull_missing(&slot_service(department, active_slot(pg, department).await)).await
}

/// Whether the named container exists and is running.
pub async fn container_running(name: &str) -> bool {
    docker(
        &["inspect", "-f", "{{.State.Running}}", name],
        Duration::from_secs(10),
    )
    .await
    .is_ok_and(|(out, _)| out.trim() == "true")
}

/// Wait for the managed container to report healthy (or give up). `unhealthy`
/// is an answer, not an error — the run files it as a warning and the roster
/// knows how to show it. A docker error mid-wait means the container is not
/// created YET; the deadline, not the error, ends the wait.
pub async fn wait_healthy(pg: &PgPool, department: &str, timeout_ms: u64) -> bool {
    let name = managed_container(pg, department).await;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    while tokio::time::Instant::now() < deadline {
        if let Ok((stdout, _)) = docker(
            &["inspect", "-f", "{{.State.Health.Status}}", &name],
            Duration::from_secs(10),
        )
        .await
        {
            match stdout.trim() {
                "healthy" => return true,
                "unhealthy" => return false,
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    false
}

/// Which departments have a RUNNING agent container right now — the
/// running-state half of container_status's projection (either service
/// name counts, so an agent still in a legacy `-b` container is rolled
/// home rather than skipped). One `docker ps` sweep, never cached: the
/// roll loop wants this fresh.
pub async fn running_departments(departments: &[String]) -> Result<Vec<String>, String> {
    let (out, _) = docker(
        &["ps", "-a", "--format", "{{.Names}}\t{{.State}}"],
        Duration::from_secs(20),
    )
    .await?;
    let by_name: HashMap<&str, &str> = out.lines().filter_map(|l| l.split_once('\t')).collect();
    Ok(departments
        .iter()
        .filter(|d| {
            [Slot::A, Slot::B].iter().any(|slot| {
                by_name
                    .get(slot_container(d, *slot).as_str())
                    .is_some_and(|state| *state == "running")
            })
        })
        .cloned()
        .collect())
}

/// Retire a container compose no longer addresses (a legacy `-b` slot a
/// roll is moving home, or a deleted agent's leftover): stop it the graceful
/// way — the gateway drained through s6, then SIGTERM and
/// `AGENT_STOP_GRACE_SECS` before docker's KILL — and
/// remove it. Never `rm -f`, which is a SIGKILL that cuts off whatever the
/// agent had in flight. A container that does not exist is already retired.
/// A stop that fails brings the drained gateway back up, so the container
/// is left serving, not running without a gateway; a remove that fails
/// leaves it stopped.
pub async fn retire_container(name: &str) -> Result<(), String> {
    if docker(&["inspect", "-f", "{{.Id}}", name], Duration::from_secs(10))
        .await
        .is_err()
    {
        return Ok(());
    }
    drain_gateway(name).await;
    let grace = AGENT_STOP_GRACE_SECS.to_string();
    if let Err(e) = docker(
        &["stop", "-t", &grace, name],
        Duration::from_secs(AGENT_STOP_GRACE_SECS + 30),
    )
    .await
    {
        undrain_gateway(name).await;
        return Err(e);
    }
    docker(&["rm", name], Duration::from_secs(60))
        .await
        .map(|_| ())
}

/// `docker exec` into a container. Args never touch a shell (argv only —
/// what a caller passes is data, not a command line). On failure stderr is
/// the half a caller wants: it is what `docker exec` itself reported.
pub async fn docker_exec(
    container: &str,
    command: &[&str],
    timeout_ms: u64,
) -> Result<(String, String), String> {
    docker_exec_opt(container, command, None, timeout_ms).await
}

/// The full form: `-i` is added exactly when `input` is given (a write pipes
/// its payload through stdin), which is how the memory write reaches `cat >`.
pub async fn docker_exec_opt(
    container: &str,
    command: &[&str],
    input: Option<&str>,
    timeout_ms: u64,
) -> Result<(String, String), String> {
    docker_exec_as(container, None, command, input, timeout_ms).await
}

/// The same exec with a user: `-u <user>` is how a platform-side step runs
/// inside the sandbox with the AGENT's rights rather than the container's
/// root — a scan of files the agent controls, a write into the agent's own
/// workdir. The user must exist in the image, which the hermes-agent image
/// guarantees for `hermes` (its own init drops to it). `None` is the container's
/// default user, exactly `docker_exec_opt`.
pub async fn docker_exec_as(
    container: &str,
    user: Option<&str>,
    command: &[&str],
    input: Option<&str>,
    timeout_ms: u64,
) -> Result<(String, String), String> {
    let mut args: Vec<&str> = vec!["exec"];
    if input.is_some() {
        args.push("-i");
    }
    if let Some(user) = user {
        args.push("-u");
        args.push(user);
    }
    args.push(container);
    args.extend(command.iter().copied());
    let out = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let mut child = tokio::process::Command::new("docker")
            .args(&args)
            .stdin(if input.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("docker exec {container} …: {e}"))?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        child
            .wait_with_output()
            .await
            .map_err(|e| format!("docker exec {container} …: {e}"))
    })
    .await
    .map_err(|_| format!("docker exec {container} …: timed out"))??;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(stderr);
    }
    Ok((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// The managed container's CURRENT name — anything that docker-execs into an
/// agent must resolve through this, never hardcode the `-1` name (an agent
/// an old blue/green roll left behind lives in "-b-1" until its next roll).
/// Resolved per call, never cached: the roll can happen between any two
/// commands.
pub async fn managed_container(pg: &PgPool, department: &str) -> String {
    let slot = active_slot(pg, department).await;
    slot_container(department, slot)
}

/// Live-resize a running agent's memory. Compose's mem_limit is the boot
/// default; this is how a workbench grows with its jobs without a roll.
/// `--memory-swap` must be ≥ `--memory` or docker refuses the raise.
pub async fn update_container_memory(
    container: &str,
    limit: u64,
    reservation: u64,
) -> Result<(), String> {
    let limit_s = limit.to_string();
    let res_s = reservation.to_string();
    docker(
        &[
            "update",
            "--memory",
            &limit_s,
            "--memory-swap",
            &limit_s,
            "--memory-reservation",
            &res_s,
            container,
        ],
        Duration::from_secs(10),
    )
    .await
    .map(|_| ())
}

async fn compose_verb(
    pg: &PgPool,
    department: &str,
    verb_args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let slot = active_slot(pg, department).await;
    let container = slot_container(department, slot);
    let svc = slot_service(department, slot);
    let _held = lock_service(&svc).await;
    // A stranded recreate container is part of the service to compose: a
    // `restart` would START it beside the agent. Clear it first.
    clear_stranded(&svc, &container).await;
    // Every verb through here stops the container (stop, restart, rm -s):
    // let the gateway drain first — and bring it back if the verb failed
    // before it got that far.
    drain_gateway(&container).await;
    let mut full: Vec<String> = compose_args(verb_args);
    full.push(svc.clone());
    let refs: Vec<&str> = full.iter().map(String::as_str).collect();
    match docker(&refs, timeout).await {
        Ok((_, stderr)) => Ok(stderr.trim().to_string()),
        Err(e) => {
            tracing::warn!("[fleet] {svc}: compose {} failed: {e}", verb_args.join(" "));
            undrain_gateway(&container).await;
            Err(e)
        }
    }
}

/// `docker volume rm` — plain success/failure: a missing volume fails, and
/// delete_agent_forever reads that as a `false`.
pub async fn remove_volume(name: &str) -> bool {
    docker(&["volume", "rm", name], Duration::from_secs(20))
        .await
        .is_ok()
}

/// Stop a managed agent. The CLI timeout outlasts the service's
/// `stop_grace_period`, or a draining agent would read as a failed stop.
pub async fn fleet_stop(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_verb(
        pg,
        department,
        &["stop"],
        verb_timeout(AGENT_STOP_GRACE_SECS + 30),
    )
    .await
}

/// Restart a managed agent's process in its EXISTING container — the same
/// image, env and mounts. Applying a change is not this: fleet_reconcile's
/// `apply_agent_change` re-renders (config.yaml lands in place, and the keys
/// Hermes reads live take effect from it) and recreates the container only
/// when [`container_fit`] says the service moved — env, image, limits, or the
/// part of config.yaml Hermes reads at start.
pub async fn fleet_restart(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_verb(
        pg,
        department,
        &["restart"],
        verb_timeout(AGENT_STOP_GRACE_SECS + 90),
    )
    .await
}

pub async fn fleet_remove(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_verb(
        pg,
        department,
        &["rm", "-sf"],
        verb_timeout(AGENT_STOP_GRACE_SECS + 30),
    )
    .await
}

/// The parsed healthcheck phase — 'starting' is the warm-up window
/// (start_period 60s on the compose healthcheck). None = no health info.
/// docker ps folds health into the Status string; the order matters because
/// "(unhealthy)" contains "healthy": starting, then the PARENTHESIZED
/// healthy, then unhealthy (parse_health's order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Health {
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "healthy")]
    Healthy,
    #[serde(rename = "unhealthy")]
    Unhealthy,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ContainerState {
    pub name: String,
    /// running | exited | …
    pub state: String,
    /// Human string incl. health, e.g. "Up 2 hours (healthy)".
    pub status: String,
    pub health: Option<Health>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentContainers {
    pub department: String,
    pub managed: Option<ContainerState>,
}

fn parse_health(status: &str) -> Option<Health> {
    let lower = status.to_lowercase();
    if lower.contains("health: starting") {
        Some(Health::Starting)
    } else if lower.contains("(healthy)") {
        Some(Health::Healthy)
    } else if lower.contains("unhealthy") {
        Some(Health::Unhealthy)
    } else {
        None
    }
}

/// Container reality per department: the talaria-managed service, by name —
/// either service name counts, preferring the one that's running (an agent
/// an old blue/green roll left in `-b` has that one; a roll moving it home
/// briefly has both). Docker CLI costs ~0.5s per shell-out and Home + alerts + the
/// roster all ask within the same breath — a 5s cache dedupes them without
/// going stale for the 10s roster poll (container_status).
pub async fn container_status(departments: &[String]) -> Result<Vec<AgentContainers>, String> {
    let key = {
        let mut sorted = departments.to_vec();
        sorted.sort();
        sorted.join(",")
    };
    let cache = STATUS_CACHE.lock().await;
    if let Some((at, cached_key, value)) = cache.as_ref()
        && cached_key == &key
        && at.elapsed() < Duration::from_secs(5)
    {
        return Ok(value.clone());
    }
    drop(cache);

    let value = container_status_fresh(departments).await?;

    let mut cache = STATUS_CACHE.lock().await;
    *cache = Some((std::time::Instant::now(), key, value.clone()));
    Ok(value)
}

static STATUS_CACHE: std::sync::LazyLock<
    tokio::sync::Mutex<Option<(std::time::Instant, String, Vec<AgentContainers>)>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(None));

async fn container_status_fresh(departments: &[String]) -> Result<Vec<AgentContainers>, String> {
    let (out, _) = docker(
        &["ps", "-a", "--format", "{{json .}}"],
        Duration::from_secs(20),
    )
    .await?;
    // One JSON object per line; a line that does not parse is skipped —
    // docker never emits one, so this is shape armor rather than a real
    // failure mode.
    let by_name: HashMap<String, ContainerState> = out
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| {
            let name = v.get("Names")?.as_str()?.to_string();
            let state = v.get("State")?.as_str()?.to_string();
            let status = v.get("Status")?.as_str()?.to_string();
            Some((
                name.clone(),
                ContainerState {
                    name,
                    state,
                    health: parse_health(&status),
                    status,
                },
            ))
        })
        .collect();
    Ok(departments
        .iter()
        .map(|department| {
            let a = by_name.get(&slot_container(department, Slot::A));
            let b = by_name.get(&slot_container(department, Slot::B));
            // Either slot counts, preferring the one that's running.
            let managed = match (a, b) {
                (Some(x), _) if x.state == "running" => Some(x.clone()),
                (_, Some(y)) if y.state == "running" => Some(y.clone()),
                (x, y) => x.or(y).cloned(),
            };
            AgentContainers {
                department: department.clone(),
                managed,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_names_carry_the_project_and_only_slot_b_gets_a_suffix() {
        // fleet_project() reads the env at call time; pin the NAME SHAPE by
        // asserting against whatever project this test environment resolves.
        let project = talaria_fleet_layout::fleet_project();
        assert_eq!(slot_service("research", Slot::A), "agent-research");
        assert_eq!(slot_service("research", Slot::B), "agent-research-b");
        assert_eq!(
            slot_container("research", Slot::A),
            format!("{project}-agent-research-1")
        );
        assert_eq!(
            slot_container("research", Slot::B),
            format!("{project}-agent-research-b-1")
        );
    }

    #[test]
    fn the_gateway_drain_downs_the_s6_service_and_waits_inside_the_grace() {
        // The drain is `s6-svc -d` (down, never revived) then `s6-svwait -D`
        // (until it has exited), by absolute path — the image keeps s6's
        // tools off docker exec's PATH — bounded inside the stop grace.
        assert_eq!(GATEWAY_SERVICE_DIR, "/run/service/gateway-default");
        assert_eq!(S6_BIN, "/command");
        const { assert!((AGENT_STOP_GRACE_SECS - 5) * 1000 >= 40_000) };
    }

    #[test]
    fn the_stop_grace_outlasts_hermes_drain_floor() {
        // Hermes drains cron and /v1 runs for agent.cron_drain_timeout (30s
        // by default) plus a 10s cleanup reserve; a shorter grace SIGKILLs
        // the agent mid-drain on every roll.
        const { assert!(AGENT_STOP_GRACE_SECS >= 30 + 10) };
    }

    #[test]
    fn compose_args_address_the_rendered_file_and_the_fleet_env() {
        let args = compose_args(&["up", "-d", "agent-research"]);
        assert_eq!(args[0], "compose");
        assert_eq!(args[1], "-p");
        assert_eq!(args[2], talaria_fleet_layout::fleet_project());
        assert!(args[4].ends_with("docker-compose.yml"), "{}", args[4]);
        assert!(args[6].ends_with("/.env"), "{}", args[6]);
        assert_eq!(&args[7..], ["up", "-d", "agent-research"]);
    }

    #[test]
    fn the_hash_comparison_decides_the_recreate() {
        let h = "4392ddf6aaf63585ca9f4f21314a3463990d5a0259266354f5abdc66be12a91d";
        let running = |hash: Option<&str>| Inspected::Seen {
            running: true,
            hash: hash.map(str::to_string),
        };
        // Created from exactly the rendered service: nothing to recreate.
        assert_eq!(fit_of(&running(Some(h)), Some(h)), ContainerFit::Matches);
        // The render moved the service (env, an env_file value, the image, a
        // mount, a limit, the start-config label): recreate.
        assert_eq!(
            fit_of(&running(Some(h)), Some("0353ec3c")),
            ContainerFit::Differs
        );
        // A container compose did not label, or a digest compose would not
        // print, is read as moved — the roll the caller used to do anyway.
        assert_eq!(
            fit_of(&running(Some("<no value>")), Some(h)),
            ContainerFit::Differs
        );
        assert_eq!(fit_of(&running(None), Some(h)), ContainerFit::Differs);
        assert_eq!(fit_of(&running(Some("")), Some("")), ContainerFit::Differs);
        assert_eq!(fit_of(&running(Some(h)), None), ContainerFit::Differs);
        // An inspect that failed says nothing about the container: read as
        // moved too, never as stopped — a stopped verdict would answer the
        // edit "applied" and strand a start-only change on a running agent.
        assert_eq!(
            fit_of(&Inspected::Unreadable, Some(h)),
            ContainerFit::Differs
        );
        // Stopped or absent: the next start reads the render; nothing to do.
        assert_eq!(
            fit_of(
                &Inspected::Seen {
                    running: false,
                    hash: Some(h.into())
                },
                Some("x")
            ),
            ContainerFit::NotRunning
        );
        assert_eq!(
            fit_of(&Inspected::Absent, Some(h)),
            ContainerFit::NotRunning
        );
    }

    #[test]
    fn a_container_is_read_back_as_running_stopped_gone_or_unknown() {
        let ok = |s: &str| Ok((s.to_string(), String::new()));
        // `docker inspect -f OBSERVE_FORMAT`, as 29.8.1 prints it.
        assert_eq!(
            parse_observed(&ok(
                "6d3b3d0e4b73c0de true 2026-10-27T21:04:11.123456789Z\n"
            )),
            Observed::Running {
                id: "6d3b3d0e4b73c0de".into(),
                started_at: "2026-10-27T21:04:11.123456789Z".into()
            }
        );
        // An exited container keeps its last StartedAt; it is still stopped.
        assert_eq!(
            parse_observed(&ok("6d3b3d0e4b73c0de false 2026-10-27T21:04:11Z\n")),
            Observed::Stopped {
                id: "6d3b3d0e4b73c0de".into()
            }
        );
        assert_eq!(
            parse_observed(&Err(
                "docker inspect failed: Error: no such object: talaria-agent-ops".into()
            )),
            Observed::Absent
        );
        assert_eq!(
            parse_observed(&Err("docker inspect …: timed out after 10s".into())),
            Observed::Unreadable
        );
        assert_eq!(parse_observed(&ok("")), Observed::Unreadable);
    }

    #[test]
    fn only_compose_s_temporary_recreate_name_is_stranded() {
        let c = "talaria-wt-fu-rolls-fleet-agent-ops-1";
        // What a killed `up --force-recreate` left on this host (compose
        // v5.5.1): the old container's first 12 id characters, then `_`.
        assert!(is_stranded_name(&format!("6d3b3d0e4b73_{c}"), c));
        // The agent itself, another agent, and lookalikes are never touched.
        assert!(!is_stranded_name(c, c));
        assert!(!is_stranded_name(
            "6d3b3d0e4b73_talaria-wt-fu-rolls-fleet-agent-ops-b-1",
            c
        ));
        assert!(!is_stranded_name(&format!("6d3b3d0e4b7_{c}"), c));
        assert!(!is_stranded_name(&format!("6d3b3d0e4b73x_{c}"), c));
        assert!(!is_stranded_name(&format!("zzzzzzzzzzzz_{c}"), c));
        assert!(!is_stranded_name(&format!("x_6d3b3d0e4b73_{c}"), c));
        assert!(!is_stranded_name(&format!("{c}_6d3b3d0e4b73"), c));
    }

    #[test]
    fn the_compose_deadline_override_takes_a_positive_number_only() {
        assert_eq!(verb_timeout_secs(None, 120), 120);
        assert_eq!(verb_timeout_secs(Some("4"), 120), 4);
        assert_eq!(verb_timeout_secs(Some(" 600 "), 120), 600);
        for ignored in ["", "0", "-3", "soon", "1.5"] {
            assert_eq!(verb_timeout_secs(Some(ignored), 120), 120, "{ignored:?}");
        }
    }

    #[test]
    fn a_timed_out_verb_is_given_the_grace_period_to_land() {
        assert_eq!(
            settle_window("docker compose …: timed out after 75s"),
            Duration::from_secs(AGENT_STOP_GRACE_SECS + 5)
        );
        assert_eq!(
            settle_window("docker compose failed: Error response from daemon: conflict"),
            Duration::from_secs(6)
        );
    }

    #[test]
    fn only_docker_saying_no_such_container_reads_as_absent() {
        let ok = |s: &str| Ok((s.to_string(), String::new()));
        assert_eq!(
            parse_inspect(&ok("true 4392ddf6\n")),
            Inspected::Seen {
                running: true,
                hash: Some("4392ddf6".into())
            }
        );
        // A container compose did not label: docker prints `<no value>`,
        // which can never equal a rendered digest, so it reads as moved.
        let unlabelled = parse_inspect(&ok("true <no value>\n"));
        assert!(matches!(unlabelled, Inspected::Seen { running: true, .. }));
        assert_eq!(fit_of(&unlabelled, Some("4392ddf6")), ContainerFit::Differs);
        assert_eq!(
            parse_inspect(&ok("false 4392ddf6\n")),
            Inspected::Seen {
                running: false,
                hash: Some("4392ddf6".into())
            }
        );
        // What docker says of a container that does not exist, as `docker()`
        // frames it: 29.8.1's own words (captured), an older CLI's
        // capitalisation, and `docker container inspect`'s daemon answer.
        for absent in [
            "docker inspect failed: Error: no such object: talaria-agent-ops",
            "docker inspect failed: Error: No such object: talaria-agent-ops",
            "docker inspect failed: Error response from daemon: No such container: talaria-agent-ops",
        ] {
            assert_eq!(
                parse_inspect(&Err(absent.into())),
                Inspected::Absent,
                "{absent}"
            );
        }
        // Everything else is not knowing — the timeout `docker()` reports on
        // an IO-bound host above all.
        for unknown in [
            "docker inspect …: timed out after 10s",
            "docker inspect failed: Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
            "docker inspect …: No such file or directory (os error 2)",
        ] {
            assert_eq!(
                parse_inspect(&Err(unknown.into())),
                Inspected::Unreadable,
                "{unknown}"
            );
        }
    }

    #[test]
    fn the_config_hash_is_read_for_the_service_asked_about() {
        let out = "agent-research 0efc77db680582df\nagent-ops ef9fd7d574ec0e33\n";
        assert_eq!(
            parse_config_hash(out, "agent-ops").as_deref(),
            Some("ef9fd7d574ec0e33")
        );
        assert_eq!(
            parse_config_hash(out, "agent-research").as_deref(),
            Some("0efc77db680582df")
        );
        // A prefix of another service is not that service.
        assert_eq!(parse_config_hash(out, "agent-res"), None);
        assert_eq!(parse_config_hash("", "agent-ops"), None);
        assert_eq!(parse_config_hash("agent-ops\n", "agent-ops"), None);
    }

    // parse_health's order is the whole test: "(unhealthy)" CONTAINS
    // "healthy", so the parenthesized healthy pattern must run first.
    #[test]
    fn health_parses_the_ps_status_string_in_ts_order() {
        assert_eq!(
            parse_health("Up 30 seconds (health: starting)"),
            Some(Health::Starting)
        );
        assert_eq!(parse_health("Up 2 hours (healthy)"), Some(Health::Healthy));
        assert_eq!(
            parse_health("Up 2 hours (unhealthy)"),
            Some(Health::Unhealthy)
        );
        assert_eq!(parse_health("Up 2 hours"), None);
        assert_eq!(parse_health("Exited (0) 3 minutes ago"), None);
        // Case-insensitive.
        assert_eq!(parse_health("Up 2 hours (HEALTHY)"), Some(Health::Healthy));
    }

    #[test]
    fn container_state_serializes_health_null_when_absent() {
        let c = ContainerState {
            name: "n".into(),
            state: "running".into(),
            status: "Up 2 hours".into(),
            health: None,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({"name": "n", "state": "running", "status": "Up 2 hours", "health": null})
        );
    }
}
