// Docker verbs for the app's own roll — the fleet's docker module shapes
// (fleet/docker.rs owns the invocation helper this reuses), aimed at the
// updater-owned project instead of the fleet's.
//
// THE SPLIT THAT SHAPES THIS SET (mod.rs's "self" law): the process that
// orchestrates a roll IS the old container, and `docker stop <self>` kills
// the choreography. Every verb here must therefore be callable by EITHER
// side: blue pulls, renders, brings green up, gates on health, attaches the
// alias, records cutting-over, and stops ITSELF; green's boot reconcile
// then waits, verifies through the edge, and retires blue by name. Nothing
// in here assumes its caller survives the next line.

use std::time::Duration;

use serde_json::Value;

use super::layout::{
    Slot, compose_file, edge_container, slot_container, slot_service, update_project,
};
use crate::fleet::docker::docker;

/// The shared DNS alias agents dial the app by on the fleet network. One
/// constant: the attach that moves it is the agent-plane cutover, and the
/// name must never drift between the renderer's env lines (which carry
/// TALARIA_MCP_GW_URL through it) and the verb that moves it.
pub const FLEET_ALIAS: &str = "talaria";

/// This container's own inspect document — `docker inspect` of HOSTNAME,
/// which docker sets to the container id inside a container. The whole
/// render is derived from it (render.rs); outside a container the inspect
/// fails with docker's own "No such object" sentence, which is the honest
/// answer there.
pub async fn inspect_self() -> Result<Value, String> {
    let host = std::env::var("HOSTNAME").unwrap_or_default();
    if host.is_empty() {
        return Err("no HOSTNAME — this process is not running in a container".into());
    }
    let (out, _) = docker(&["inspect", &host], Duration::from_secs(20)).await?;
    let first = serde_json::from_str::<Value>(&out)
        .map_err(|e| format!("docker inspect of self is not JSON: {e}"))?;
    serde_json::from_value(
        first
            .as_array()
            .and_then(|a| a.first().cloned())
            .unwrap_or(first),
    )
    .map_err(|e| format!("docker inspect of self is unreadable: {e}"))
}

/// Compose argv for the updater-owned project: `-p <project> -f <compose>`
/// then the verb. No project-level `--env-file`: the renderer bakes values
/// (the fleet interpolates because operators edit its chassis; nobody edits
/// this file by hand — it is re-rendered from the live container on every
/// roll).
fn compose_args(extra: &[&str]) -> Vec<String> {
    let mut args = vec![
        "compose".to_string(),
        "-p".to_string(),
        update_project(),
        "-f".to_string(),
        compose_file().to_string_lossy().into_owned(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

/// Bring one slot up (`up -d --no-deps <service>`): the file is on disk,
/// the image pulled; this is the moment the incoming container starts
/// gating on its healthcheck. The service name is NOT optional — bare
/// `up -d` would address every service in the file, including the ACTIVE
/// slot and the edge, and a config drift on the active slot is exactly
/// what a roll must never touch mid-flight. `--no-deps` because the
/// sidecars are not this project's — they keep running where they are.
pub async fn slot_up(slot: Slot) -> Result<(), String> {
    service_up(&slot_service(slot)).await
}

/// Bring one service of the update project up (compose, scoped to the
/// service — a bare `up` would address the ACTIVE slot too). The edge is
/// the second caller: adoption brings it up on the port it will own.
pub async fn service_up(service: &str) -> Result<(), String> {
    let args = compose_args(&["up", "-d", "--no-deps", "--quiet-pull", service]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // First boot of a service can spend its time in image extraction; the
    // pull itself is a separate verb with its own patience.
    docker(&refs, Duration::from_secs(300)).await.map(|_| ())
}

/// Pull an image BY DIGEST (`repo@sha256:…` — the ref the roll engine
/// hands here is never a tag; mod.rs's order of trust). Long patience: a
/// full app image over a slow link is gigabytes, and the old container
/// keeps serving the whole time — there is no failure mode that hurrying
/// improves.
pub async fn pull_image(reference: &str) -> Result<(), String> {
    docker(&["pull", reference], Duration::from_secs(900))
        .await
        .map(|_| ())
}

/// The roll gate, fleet's spelling: poll the slot container's health until
/// `healthy` (true) or `unhealthy`/deadline (false). `starting` keeps
/// waiting — the compose healthcheck's start_period is 90s of boot
/// migrations the gate MUST tolerate.
pub async fn wait_healthy_slot(slot: Slot, timeout_ms: u64) -> bool {
    wait_healthy_container(&slot_container(slot), timeout_ms).await
}

/// The gate, by container name — the slots' spelling (see wait_healthy_slot)
/// and the adoption caller's (which gates a container it did not name
/// through a slot).
pub async fn wait_healthy_container(name: &str, timeout_ms: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    while tokio::time::Instant::now() < deadline {
        if let Ok((stdout, _)) = docker(
            &["inspect", "-f", "{{.State.Health.Status}}", name],
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

/// Is a container currently running? (Adoption's resume asks this about the
/// retired orchestrator container; an absent container is simply not
/// running.)
pub async fn container_running(name: &str) -> bool {
    matches!(
        docker(
            &["inspect", "-f", "{{.State.Running}}", name],
            Duration::from_secs(10),
        )
            .await
            .map(|(out, _)| out.trim().to_string()),
        Ok(r) if r == "true"
    )
}

/// The registry digest of a LOCAL image (`docker image inspect`'s
/// RepoDigests), if the image was pulled from a registry — adoption pins
/// the running image when the registry's tag has nothing newer to say.
pub async fn image_repo_digest(reference: &str) -> Option<String> {
    let (out, _) = docker(
        &[
            "image",
            "inspect",
            "-f",
            "{{range .RepoDigests}}{{.}}{{println}}{{end}}",
            reference,
        ],
        Duration::from_secs(15),
    )
    .await
    .ok()?;
    out.lines()
        .find_map(|l| l.split_once('@').map(|(_, d)| d.trim().to_string()))
        .filter(|d| super::registry::is_digest(d))
}

/// Stop a container with an explicit grace (`docker stop -t <secs>`): the
/// app's SIGTERM path drains in-flight requests, and the grace is how long
/// docker waits before SIGKILL. NOT removal — a stopped container is the
/// rollback's material.
pub async fn stop_container(name: &str, grace_ms: u64) -> Result<(), String> {
    let secs = (grace_ms / 1000).max(1).to_string();
    docker(&["stop", "-t", &secs, name], Duration::from_secs(300))
        .await
        .map(|_| ())
}

/// Start a stopped container by name — the rollback verb: the old slot
/// restarts with everything it had (its slot env, its networks-minus-the-
/// alias), and the caller re-attaches the alias.
pub async fn start_container(name: &str) -> Result<(), String> {
    docker(&["start", name], Duration::from_secs(120))
        .await
        .map(|_| ())
}

/// Remove a container by exact name — the post-keep-window retirement of
/// an old slot (the fleet's remove-by-name: by then it is not in the
/// compose file, so compose cannot address it).
pub async fn remove_container(name: &str) -> Result<(), String> {
    docker(&["rm", "-f", name], Duration::from_secs(60))
        .await
        .map(|_| ())
}

/// Move the shared `talaria` alias onto a container on the fleet network —
/// THE agent-plane cutover. Disconnect first, best-effort: a container
/// that already sits on the network (a rollback target that only stopped)
/// makes `connect` refuse, and the alias must move, not dangle. While both
/// slots hold the alias docker round-robins between two HEALTHY backends —
/// the same DB, the same wire — and the old one's stop ends the overlap.
pub async fn attach_fleet_alias(network: &str, container: &str) -> Result<(), String> {
    let _ = docker(
        &["network", "disconnect", network, container],
        Duration::from_secs(30),
    )
    .await;
    docker(
        &[
            "network",
            "connect",
            "--alias",
            FLEET_ALIAS,
            network,
            container,
        ],
        Duration::from_secs(30),
    )
    .await
    .map(|_| ())
}

/// Is the edge container up and reporting healthy? Green's boot reconcile
/// verifies itself THROUGH the edge before marking the run done — the port
/// the world dials is the port that must answer.
pub async fn edge_healthy(timeout_ms: u64) -> bool {
    let name = edge_container();
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
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

/// The service names for both slots — the renderer and the verbs agree on
/// them through layout; exposed for the render tests' round-trip.
pub fn service_name(slot: Slot) -> String {
    slot_service(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_args_address_the_updater_project() {
        let args = compose_args(&["up", "-d"]);
        assert_eq!(args[0], "compose");
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&update_project()));
        // The compose file rides as an absolute path next to the project
        // flag; the verb follows.
        let f = args.iter().position(|a| a == "-f").unwrap();
        assert!(args[f + 1].ends_with("compose.yml"));
        assert_eq!(args.last().unwrap(), "-d");
    }
}
