// The fleet's LAYOUT — where the rendered fleet lives on disk and what every
// docker-level name resolves to. Port of the constant block at the top of
// ui/src/server/fleet-render.ts (lines 32-63, 171, 205-235), lifted into its
// own module because the write plane (fleet-create, fleet-render,
// fleet-docker) all resolves through these and one path rule must have one
// definition — gateway/provider.rs already carried a private copy of
// `fleet_dir` from the read side, and it delegates here now.
//
// THE INVARIANT BEHIND `fleet_project`: one fleet per compose project is the
// identity the whole lifecycle assumes (container names, volume names,
// reconcile scope), so anything sharing a docker host with agents — a second
// instance, a devbox — must drive its own project or the two reconcile each
// other's containers. Everything that names the project resolves through here
// (same pattern as TALARIA_FLEET_NETWORK for the network).

use serde_json::Value;
use std::path::PathBuf;

/// The MCP gateway base as fleet containers reach it — the UI server over the
/// docker host bridge (same pattern as the talaria-mcp fleet URL).
pub fn mcp_gw_base() -> String {
    match std::env::var("TALARIA_MCP_GW_URL") {
        Ok(url) => url,
        // TS `??` — an env var set but EMPTY is not nullish and would be
        // used verbatim; var() only errors on unset/invalid UTF-8, so this
        // arm is exactly the unset case.
        Err(_) => format!("http://host.docker.internal:{}/api/mcp/gw", app_port()),
    }
}

/// The port the app server listens on (vite/prod PORT, 5273 in dev).
pub(crate) fn app_port() -> String {
    std::env::var("PORT").unwrap_or_else(|_| "5273".into())
}

/// FLEET_DIR (fleet-render.ts:38) — the render output tree the renderer
/// materializes and every docker command addresses.
pub fn fleet_dir() -> PathBuf {
    if let Ok(d) = std::env::var("TALARIA_FLEET_DIR") {
        return PathBuf::from(d);
    }
    std::env::current_dir()
        .map(|c| c.join("../fleet"))
        .unwrap_or_else(|_| PathBuf::from("../fleet"))
}

/// The fleet's compose project — see the header invariant.
pub fn fleet_project() -> String {
    std::env::var("TALARIA_FLEET_PROJECT").unwrap_or_else(|_| "talaria-fleet".into())
}

/// The fleet's env file (agent keys + compose interpolation) — Talaria-owned.
pub fn fleet_env() -> PathBuf {
    fleet_dir().join(".env")
}

/// The chassis every agent renders from: one service block + per-slug extras.
/// Talaria-owned (extracted once at cutover from the legacy stack).
pub fn chassis_file() -> PathBuf {
    match std::env::var("TALARIA_CHASSIS_FILE") {
        Ok(f) => PathBuf::from(f),
        Err(_) => fleet_dir().join("chassis.yml"),
    }
}

/// Repo-shipped fleet skills (scripts/skills/*) seed into the fleet's shared
/// skills root on render.
pub fn seed_skills_dir() -> PathBuf {
    std::env::current_dir()
        .map(|c| c.join("../scripts/skills"))
        .unwrap_or_else(|_| PathBuf::from("../scripts/skills"))
}

/// Compose-interpolation name for an agent's own credential.
pub fn agent_key_var(slug: &str) -> String {
    format!("TALARIA_AGENT_KEY_{}", slug.to_uppercase())
}

/// The host the app reaches agents on. Loopback in dev (host app + published
/// ports); set to a service DNS name for a fully containerized deployment.
pub fn agent_host() -> String {
    std::env::var("TALARIA_AGENT_HOST").unwrap_or_else(|_| "127.0.0.1".into())
}

/// Docker-level names inherited from the pre-Talaria stack: imported agents'
/// state volumes (ai_hermes-<dept>) and the shared infra network (ai_default).
/// These are volume/network NAMES, not a code dependency on that repo.
pub const LEGACY_DOCKER_PROJECT: &str = "ai";

/// Where per-agent loopback gateway ports start (ensure_gateway_ports assigns
/// upward from here, persisted on the def row).
pub const GATEWAY_PORT_BASE: i64 = 8770;

/// The EXTERNAL docker network the whole fleet joins (compose never creates
/// external networks — fleet-docker ensures it exists before any `up`). The
/// name lives in the chassis; 'talaria' is the fresh-install default.
pub async fn fleet_network_name() -> String {
    let text = match tokio::fs::read_to_string(chassis_file()).await {
        Ok(t) => t,
        Err(_) => return "talaria".into(),
    };
    // The chassis is data, not a typed contract: the renderer walks its
    // service block as a tree, so parse it as one tree here too.
    match serde_yaml_ng::from_str::<Value>(&text) {
        Ok(chassis) => chassis
            .get("network")
            .and_then(|n| n.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "talaria".into()),
        Err(_) => "talaria".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_key_var_uppercases_the_slug() {
        assert_eq!(agent_key_var("sloane"), "TALARIA_AGENT_KEY_SLOANE");
        assert_eq!(agent_key_var("dot.name"), "TALARIA_AGENT_KEY_DOT.NAME");
    }

    #[test]
    fn a_chassis_without_a_network_block_defaults_to_talaria() {
        // Parse-level test of the same walk fleet_network_name does, against
        // inline chassis text: the FILE READ resolves through the env, so it
        // is not unit-testable here, but the TREE WALK is the part that can
        // go wrong.
        for text in [
            "service:\n  image: x\n",
            "network: {}\n",
            "network:\n  other: y\n",
        ] {
            let chassis: Value = serde_yaml_ng::from_str(text).expect("the minimal chassis parses");
            let name = chassis
                .get("network")
                .and_then(|n| n.get("name"))
                .and_then(Value::as_str);
            assert_eq!(name, None, "no name in: {text}");
        }
        let chassis: Value = serde_yaml_ng::from_str("network:\n  name: ai_default\n").unwrap();
        assert_eq!(
            chassis
                .get("network")
                .and_then(|n| n.get("name"))
                .and_then(Value::as_str),
            Some("ai_default")
        );
    }
}
