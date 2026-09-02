// Docker control for the managed fleet — Talaria drives `docker compose` on
// the rendered fleet/docker-compose.yml (interpolation env = the fleet's own
// .env, so per-agent keys/secrets stay in one Talaria-owned, gitignored
// place). Port of the LIFECYCLE half of ui/src/server/fleet-docker.ts.
//
// The whole TS module crossed with the fleet tail: slot naming, active-slot
// resolution, the external network guarantee, the lifecycle verbs (up, stop,
// restart, remove), the healthcheck wait, bundled-skill pruning, the 5s-cached
// `docker ps` status the roster and Home poll, and the shared `docker exec`
// wrapper the cron and memory surfaces reach an agent's own filesystem
// through.
//
// Rolling slots: each agent runs as `agent-<dept>` (slot a) or
// `agent-<dept>-b` (slot b). Callers pass a department as always — the
// active slot resolves here — and only the roll orchestration addresses a
// slot explicitly.

use std::collections::HashMap;
use std::time::Duration;

use sqlx::PgPool;

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
        crate::fleet::layout::fleet_project(),
        slot_service(department, slot)
    )
}

/// Which slot a department's managed agent currently runs in, from the def
/// row the roll orchestration updates. Missing row / null reads as 'a' —
/// TS's `rows[0]?.s === 'b' ? 'b' : 'a'`.
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

/// One docker CLI invocation with execFile's contract: non-zero exit is an
/// error carrying stderr, success returns (stdout, stderr) trimmed as written.
async fn docker(args: &[&str], timeout: Duration) -> Result<(String, String), String> {
    let out = tokio::time::timeout(timeout, async {
        tokio::process::Command::new("docker")
            .args(args)
            .stdin(std::process::Stdio::null())
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
        crate::fleet::layout::fleet_project(),
        "-f".to_string(),
        crate::fleet::layout::fleet_dir()
            .join("docker-compose.yml")
            .to_string_lossy()
            .into_owned(),
        "--env-file".to_string(),
        crate::fleet::layout::fleet_env()
            .to_string_lossy()
            .into_owned(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

/// The fleet joins an EXTERNAL network (shared with app/bridge/MCP
/// containers), which compose declares but never creates. Create it here when
/// missing so a fresh install works without any setup script. Idempotent,
/// race-safe.
async fn ensure_fleet_network() -> Result<(), String> {
    let name = crate::fleet::layout::fleet_network_name().await;
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
/// nothing of it, but the shape is what TS handed back.
pub async fn fleet_up(pg: &PgPool, department: &str) -> Result<String, String> {
    let slot = active_slot(pg, department).await;
    fleet_up_slot(pg, department, slot).await
}

/// The roll's spelling: address a SPECIFIC slot (the incoming one), not the
/// active one (fleet-docker.ts fleetUp(department, slot)).
pub async fn fleet_up_slot(pg: &PgPool, department: &str, slot: Slot) -> Result<String, String> {
    ensure_fleet_network().await?;
    let svc = slot_service(department, slot);
    let args: Vec<String> = compose_args(&["up", "-d", &svc]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (_, stderr) = docker(&refs, Duration::from_secs(120)).await?;
    // Bringing an agent up is the moment to ask whether it can reach us — from
    // the fleet network, not from here. Detached: a preflight must never be
    // able to fail a start, and its whole value is that it writes a verdict
    // somebody reads later (alerts) rather than that this caller waits for it.
    let pool = pg.clone();
    tokio::spawn(async move {
        let _ = crate::fleet::preflight::run_fleet_preflight(&pool).await;
    });
    Ok(stderr.trim().to_string())
}

/// Wait for the managed container to report healthy (or give up). `unhealthy`
/// is an answer, not an error — the run files it as a warning and the roster
/// knows how to show it. A docker error mid-wait means the container is not
/// created YET; the deadline, not the error, ends the wait.
pub async fn wait_healthy(pg: &PgPool, department: &str, timeout_ms: u64) -> bool {
    let slot = active_slot(pg, department).await;
    wait_healthy_slot(department, slot, timeout_ms).await
}

/// The roll's spelling: wait on a SPECIFIC slot (waitHealthy(department, slot)).
pub async fn wait_healthy_slot(department: &str, slot: Slot, timeout_ms: u64) -> bool {
    let name = slot_container(department, slot);
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

/// Which departments have a RUNNING slot container right now — the
/// `managed?.state === 'running'` half of containerStatus (the full
/// projection crosses with the fleet-status routes). One `docker ps` sweep,
/// never cached: the roll loop wants this fresh.
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

/// Remove a container by exact name — used to retire the old slot after a
/// roll: it's no longer in the compose file by then, so compose can't address
/// it (fleet-docker.ts removeContainerByName).
pub async fn remove_container_by_name(name: &str) -> Result<(), String> {
    docker(&["rm", "-f", name], Duration::from_secs(60))
        .await
        .map(|_| ())
}

/// `docker exec` into a container — port of docker-exec.ts dockerExec. Args
/// never touch a shell (argv only — cron prompts are data, not command
/// lines). On failure stderr is the half a caller wants: it is what
/// `docker exec` itself reported, and it beats the node error text when both
/// exist (TS: `new Error(String(stderr).trim() || err.message)`).
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
    let mut args: Vec<&str> = vec!["exec"];
    if input.is_some() {
        args.push("-i");
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
/// agent must resolve through this, never hardcode the slot-a name (a rolled
/// agent lives in "-b-1" until its next roll). Slot-aware per call, never
/// cached: the roll can happen between any two commands.
pub async fn managed_container(pg: &PgPool, department: &str) -> String {
    let slot = active_slot(pg, department).await;
    slot_container(department, slot)
}

/// One lifecycle verb against the department's ACTIVE slot's compose service
/// (fleet-docker.ts fleetStop / fleetRestart / fleetRemove). All three answer
/// with compose's stderr trimmed — the shape TS handed back.
async fn compose_verb(
    pg: &PgPool,
    department: &str,
    verb_args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let slot = active_slot(pg, department).await;
    let svc = slot_service(department, slot);
    let mut full: Vec<String> = compose_args(verb_args);
    full.push(svc);
    let refs: Vec<&str> = full.iter().map(String::as_str).collect();
    let (_, stderr) = docker(&refs, timeout).await?;
    Ok(stderr.trim().to_string())
}

/// `docker volume rm` — plain success/failure, the way deleteAgentForever's
/// execFile callback reads it (a missing volume fails; that's a "false").
pub async fn remove_volume(name: &str) -> bool {
    docker(&["volume", "rm", name], Duration::from_secs(20))
        .await
        .is_ok()
}

pub async fn fleet_stop(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_verb(pg, department, &["stop"], Duration::from_secs(60)).await
}

/// Restart a managed agent so a re-rendered config.yaml takes effect. Prefer
/// roll_agent (fleet_reconcile) for user-facing changes — a restart has a
/// downtime window; a roll doesn't.
pub async fn fleet_restart(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_verb(pg, department, &["restart"], Duration::from_secs(120)).await
}

pub async fn fleet_remove(pg: &PgPool, department: &str) -> Result<String, String> {
    compose_verb(pg, department, &["rm", "-sf"], Duration::from_secs(60)).await
}

/// The parsed healthcheck phase — 'starting' is the warm-up window
/// (start_period 60s on the compose healthcheck). None = no health info.
/// docker ps folds health into the Status string; the order matters because
/// "(unhealthy)" contains "healthy": starting, then the PARENTHESIZED
/// healthy, then unhealthy (parseHealth's regex order).
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
/// either slot counts, preferring the one that's running (mid-roll both
/// exist). Docker CLI costs ~0.5s per shell-out and Home + alerts + the
/// roster all ask within the same breath — a 5s cache dedupes them without
/// going stale for the 10s roster poll (containerStatus).
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
    // One JSON object per line; a line that does not parse is skipped the way
    // TS's JSON.parse throw would abort — but docker never emits one, so the
    // unwrap mirrors the TS shape rather than a real failure mode.
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

/// Bundled skill packs that CONFLICT with the Talaria toolkit — they pitch a
/// parallel system of record (external note vaults, ungoverned email) and
/// send agents flailing. Removed explicitly because the seed marks packs
/// "user-modified", which opt-out --remove preserves.
const CONFLICTING_SKILL_PACKS: [&str; 5] = [
    "note-taking", // obsidian — Talaria KB is the knowledgebase
    "productivity/notion",
    "productivity/airtable",
    "productivity/google-workspace", // Talaria's Google integration is confirm-send governed
    "email",                         // draft_email/read_recent_email govern mail through Talaria
];

/// Strip the image's conflicting bundled skills from a slot's container
/// (pruneBundledSkills). Surgical — only the conflict list goes; the rest of
/// the bundled packs are genuinely useful and stay. Best-effort by contract:
/// the caller treats false as "skip", never as failure.
pub async fn prune_bundled_skills(department: &str, slot: Slot) -> bool {
    let name = slot_container(department, slot);
    let paths = CONFLICTING_SKILL_PACKS
        .iter()
        .map(|p| format!("/opt/data/skills/{p}"))
        .collect::<Vec<_>>()
        .join(" ");
    docker(
        &["exec", &name, "sh", "-c", &format!("rm -rf {paths}")],
        Duration::from_secs(30),
    )
    .await
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_names_carry_the_project_and_only_slot_b_gets_a_suffix() {
        // fleet_project() reads the env at call time; pin the NAME SHAPE by
        // asserting against whatever project this test environment resolves.
        let project = crate::fleet::layout::fleet_project();
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
    fn compose_args_address_the_rendered_file_and_the_fleet_env() {
        let args = compose_args(&["up", "-d", "agent-research"]);
        assert_eq!(args[0], "compose");
        assert_eq!(args[1], "-p");
        assert_eq!(args[2], crate::fleet::layout::fleet_project());
        assert!(args[4].ends_with("docker-compose.yml"), "{}", args[4]);
        assert!(args[6].ends_with("/.env"), "{}", args[6]);
        assert_eq!(&args[7..], ["up", "-d", "agent-research"]);
    }

    // parseHealth's order is the whole test: "(unhealthy)" CONTAINS
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
        // Case-insensitive, as the TS /i flags insist.
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
