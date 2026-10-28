// Docker control for the managed fleet — Talaria drives `docker compose` on
// the rendered fleet/docker-compose.yml (interpolation env = the fleet's own
// .env, so per-agent keys/secrets stay in one Talaria-owned, gitignored
// place).
//
// Covers service naming, the external network guarantee, the lifecycle verbs
// (up, recreate, stop, restart, remove), the image pull, the healthcheck
// wait, the 5s-cached `docker ps` status the roster and Home poll, and the
// shared `docker exec` wrapper the cron and memory surfaces use to reach an
// agent's own filesystem. Nothing here execs into an agent after it comes up:
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
/// blue/green roll left behind reads 'b', until its next roll. Where two
/// agents hold one department (a pair that predates the one-agent rule), the
/// row read is the one the render runs there: enabled with a current version
/// first (the render's own conditions), then the oldest.
pub async fn active_slot(pg: &PgPool, department: &str) -> Slot {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "select d.active_slot from agent_defs d where d.department = $1 and d.managed \
         order by (d.enabled and exists (select 1 from agent_versions v \
             where v.agent_id = d.id and v.version = d.current_version)) desc, \
           d.created_at, d.id limit 1",
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
    compose_up(pg, department, &["up", "-d"], Duration::from_secs(120)).await
}

/// Recreate a department's managed agent IN PLACE — the roll. Compose stops
/// the running container (SIGTERM, then `stop_grace_period` before a KILL),
/// then creates and starts a fresh one from the current render on the same
/// state volume. `--no-deps`: an agent has none, and a roll must never touch
/// another service. The image is pulled first when the host lacks it, so a
/// pull that fails leaves the running container exactly as it was.
pub async fn fleet_recreate(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_up(
        pg,
        department,
        &["up", "-d", "--force-recreate", "--no-deps"],
        Duration::from_secs(AGENT_STOP_GRACE_SECS + 120),
    )
    .await
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
    pull_missing(&svc).await?;
    // A recreate stops the running container: drain it first. A plain `up`
    // starts one and leaves a running one alone, so it has nothing to drain.
    let drained = verb
        .contains(&"--force-recreate")
        .then(|| slot_container(department, slot));
    if let Some(container) = &drained {
        drain_gateway(container).await;
    }
    let mut args: Vec<String> = compose_args(verb);
    args.push(svc);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (_, stderr) = match docker(&refs, timeout).await {
        Ok(out) => out,
        Err(e) => {
            if let Some(container) = &drained {
                undrain_gateway(container).await;
            }
            return Err(e);
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
    // Every verb through here stops the container (stop, restart, rm -s):
    // let the gateway drain first — and bring it back if the verb failed
    // before it got that far.
    let container = slot_container(department, slot);
    drain_gateway(&container).await;
    let svc = slot_service(department, slot);
    let mut full: Vec<String> = compose_args(verb_args);
    full.push(svc);
    let refs: Vec<&str> = full.iter().map(String::as_str).collect();
    match docker(&refs, timeout).await {
        Ok((_, stderr)) => Ok(stderr.trim().to_string()),
        Err(e) => {
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
        Duration::from_secs(AGENT_STOP_GRACE_SECS + 30),
    )
    .await
}

/// Restart a managed agent's process in its EXISTING container — the same
/// image, env and mounts. A config.yaml change needs neither this nor a roll
/// (the render rewrites it in place and Hermes reloads it); an env, image or
/// limit change needs a roll (`roll_agent`, fleet_reconcile), which
/// recreates the container.
pub async fn fleet_restart(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_verb(
        pg,
        department,
        &["restart"],
        Duration::from_secs(AGENT_STOP_GRACE_SECS + 90),
    )
    .await
}

pub async fn fleet_remove(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_verb(
        pg,
        department,
        &["rm", "-sf"],
        Duration::from_secs(AGENT_STOP_GRACE_SECS + 30),
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
