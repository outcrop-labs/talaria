// Docker control for the managed fleet — Talaria drives `docker compose` on
// the rendered fleet/docker-compose.yml (interpolation env = the fleet's own
// .env, so per-agent keys/secrets stay in one Talaria-owned, gitignored
// place). Port of the LIFECYCLE half of ui/src/server/fleet-docker.ts.
//
// SCOPE, deliberately: only what the agent-hire run's boot stage reaches —
// slot naming, active-slot resolution, the external network guarantee, `up`,
// the post-up reachability kick, and the healthcheck wait. The rest of the TS
// module (stop/restart/remove, the 5s-cached `docker ps` status the roster
// and Home poll, bundled-skill pruning) is read-plane surface for routes that
// still serve from TS; each crosses with its caller.
//
// Rolling slots: each agent runs as `agent-<dept>` (slot a) or
// `agent-<dept>-b` (slot b). Callers pass a department as always — the
// active slot resolves here — and only the roll orchestration addresses a
// slot explicitly.

use std::time::Duration;

use sqlx::PgPool;

use crate::fleet_layout;

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
        fleet_layout::fleet_project(),
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
        fleet_layout::fleet_project(),
        "-f".to_string(),
        fleet_layout::fleet_dir()
            .join("docker-compose.yml")
            .to_string_lossy()
            .into_owned(),
        "--env-file".to_string(),
        fleet_layout::fleet_env().to_string_lossy().into_owned(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

/// The fleet joins an EXTERNAL network (shared with app/bridge/MCP
/// containers), which compose declares but never creates. Create it here when
/// missing so a fresh install works without any setup script. Idempotent,
/// race-safe.
async fn ensure_fleet_network() -> Result<(), String> {
    let name = fleet_layout::fleet_network_name().await;
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
pub async fn fleet_up_slot(
    pg: &PgPool,
    department: &str,
    slot: Slot,
) -> Result<String, String> {
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
        let _ = crate::fleet_preflight::run_fleet_preflight(&pool).await;
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

/// Remove a container by exact name — used to retire the old slot after a
/// roll: it's no longer in the compose file by then, so compose can't address
/// it (fleet-docker.ts removeContainerByName).
pub async fn remove_container_by_name(name: &str) -> Result<(), String> {
    docker(&["rm", "-f", name], Duration::from_secs(60))
        .await
        .map(|_| ())
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
    "email", // draft_email/read_recent_email govern mail through Talaria
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
        let project = fleet_layout::fleet_project();
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
        assert_eq!(args[2], fleet_layout::fleet_project());
        assert!(args[4].ends_with("docker-compose.yml"), "{}", args[4]);
        assert!(args[6].ends_with("/.env"), "{}", args[6]);
        assert_eq!(&args[7..], ["up", "-d", "agent-research"]);
    }
}
