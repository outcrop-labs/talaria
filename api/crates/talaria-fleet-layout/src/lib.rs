// The fleet's LAYOUT — where the rendered fleet lives on disk and what every
// docker-level name resolves to. Its own module because every write-plane
// caller (create, render, docker) resolves through these and one path rule
// must have one definition.
//
// THE INVARIANT BEHIND `fleet_project`: one fleet per compose project is the
// identity the whole lifecycle assumes (container names, volume names,
// reconcile scope), so anything sharing a docker host with agents — a second
// instance, a devbox — must drive its own project or the two reconcile each
// other's containers. Everything that names the project resolves through here
// (same pattern as TALARIA_FLEET_NETWORK for the network).

use serde_json::Value;
use std::path::PathBuf;

/// The MCP gateway base as fleet containers reach it — the app server over
/// the docker host bridge (same pattern as the talaria-mcp fleet URL).
pub fn mcp_gw_base() -> String {
    match std::env::var("TALARIA_MCP_GW_URL") {
        Ok(url) => url,
        // A set-but-empty env var is used verbatim; var() errs only on
        // unset/invalid UTF-8, so the Err arm is exactly the unset case.
        Err(_) => format!("http://host.docker.internal:{}/api/mcp/gw", app_port()),
    }
}

/// The port the app server listens on (PORT env, 5273 in dev).
pub fn app_port() -> String {
    std::env::var("PORT").unwrap_or_else(|_| "5273".into())
}

/// The render output tree (FLEET_DIR) the renderer materializes and every
/// docker command addresses.
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
/// Talaria-owned.
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

/// TALARIA_AGENT_DIAL=container (docker/compose.yml deployments): the app runs
/// ON the fleet network and dials compose service names, so loopback port
/// publishing is skipped entirely — every install sharing one docker host
/// allocates identical ports from the same seed data and would collide.
pub fn dial_is_container() -> bool {
    matches!(std::env::var("TALARIA_AGENT_DIAL"), Ok(v) if v == "container")
}

/// The operator's search endpoint AS AN AGENT CONTAINER CAN REACH IT, or None.
/// The app's spelling is the app's view: on a host-run dev app a loopback
/// address means the laptop, which an agent reaches as host.docker.internal
/// (the chassis declares it); in a containerized stack a loopback address is
/// the app container itself and reaches nothing from an agent, so it is
/// withheld rather than rendered wrong. Any other host passes through as-is.
///
/// One rule for two readers: the fleet render hands the result to every agent
/// as SEARXNG_URL, and Admin → Search reads it back so the panel says what the
/// render does. It rewrites; it never probes — a URL it passes may still be
/// one the agents cannot connect to.
pub fn agent_search_url(app_url: &str, container_dial: bool) -> Option<String> {
    let url = app_url.trim();
    if url.is_empty() {
        return None;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return Some(url.to_string());
    };
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (authority, None),
    };
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "0.0.0.0") {
        return Some(url.to_string());
    }
    if container_dial {
        return None;
    }
    let port = port.map(|p| format!(":{p}")).unwrap_or_default();
    let path = if path.is_empty() {
        String::new()
    } else {
        format!("/{path}")
    };
    Some(format!("{scheme}://host.docker.internal{port}{path}"))
}

/// Docker-level names inherited from the pre-Talaria stack: imported agents'
/// state volumes (ai_hermes-<dept>) and the shared infra network (ai_default).
/// These are volume/network NAMES, not a code dependency on that repo.
pub const LEGACY_DOCKER_PROJECT: &str = "ai";

/// Where per-agent loopback gateway ports start (ensure_gateway_ports assigns
/// upward from here, persisted on the def row).
pub const GATEWAY_PORT_BASE: i64 = 8770;

/// The base THIS process allocates gateway ports from:
/// `TALARIA_GATEWAY_PORT_BASE` when it names a usable port, else
/// [`GATEWAY_PORT_BASE`]. Every stack keeps its ports in its own database, so
/// two stacks on one docker host both start at the same base, and the second
/// one's first agent cannot publish the port the first one's holds ("port is
/// already allocated"). `talaria worktree` writes a per-stack base for exactly
/// that.
pub fn gateway_port_base() -> i64 {
    parse_gateway_port_base(std::env::var("TALARIA_GATEWAY_PORT_BASE").ok().as_deref())
}

/// The pure half of [`gateway_port_base`]: an unset, unparseable or
/// unusable value falls back to the default rather than failing a render.
/// Usable leaves room above for a stack's agents (100) below 65535.
pub fn parse_gateway_port_base(raw: Option<&str>) -> i64 {
    raw.and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|p| (1024..=65_435).contains(p))
        .unwrap_or(GATEWAY_PORT_BASE)
}

/// The EXTERNAL docker network the whole fleet joins (compose never creates
/// external networks — the docker module ensures it exists before any `up`).
/// The name lives in the chassis; 'talaria' is the fresh-install default.
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
    fn the_gateway_port_base_is_the_env_when_usable_and_the_default_otherwise() {
        assert_eq!(parse_gateway_port_base(None), GATEWAY_PORT_BASE);
        assert_eq!(parse_gateway_port_base(Some("20300")), 20300);
        assert_eq!(parse_gateway_port_base(Some(" 20300\n")), 20300);
        for bad in ["", "abc", "80", "0", "-1", "65500", "99999"] {
            assert_eq!(
                parse_gateway_port_base(Some(bad)),
                GATEWAY_PORT_BASE,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn an_operators_search_url_reaches_an_agent_only_where_it_can() {
        // Nothing configured: nothing rendered — Hermes resolves its own floor.
        assert_eq!(agent_search_url("", false), None);
        assert_eq!(agent_search_url("   ", true), None);
        // A real host passes through untouched, container or not.
        assert_eq!(
            agent_search_url("https://search.corp.example/", true).as_deref(),
            Some("https://search.corp.example/")
        );
        // Loopback on a host-run app is the laptop, which an agent spells
        // host.docker.internal; the port and any path survive.
        assert_eq!(
            agent_search_url("http://127.0.0.1:8888", false).as_deref(),
            Some("http://host.docker.internal:8888")
        );
        assert_eq!(
            agent_search_url("http://localhost:8080/searx", false).as_deref(),
            Some("http://host.docker.internal:8080/searx")
        );
        // Loopback in a containerized stack is the app container itself —
        // withheld, never rendered wrong.
        assert_eq!(agent_search_url("http://127.0.0.1:8888", true), None);
    }

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

#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentModel {
    pub id: String,
    /// Display label — the leading segment of the id, capitalised.
    pub label: String,
    /// Role remainder of the id.
    pub role: String,
}

/// Split an agent id into its display halves: "<slug>-<role>". An empty
/// leading segment (an id like "-lead") keeps the WHOLE id as the label.
pub fn describe_agent(id: &str) -> AgentModel {
    let (first, rest) = match id.split_once('-') {
        Some((f, r)) => (f, r),
        None => (id, ""),
    };
    let label = if first.is_empty() {
        id.to_string()
    } else {
        let mut c = first.chars();
        match c.next() {
            Some(h) => h.to_uppercase().collect::<String>() + c.as_str(),
            None => first.to_string(),
        }
    };
    AgentModel {
        id: id.to_string(),
        label,
        role: rest.replace('-', " "),
    }
}

/// fleet.json — the render OUTPUT table of {model, url, key} per agent.
#[derive(serde::Deserialize)]
pub struct ManifestEntry {
    pub model: String,
    pub url: String,
    pub key: Option<String>,
}

/// A missing or unparseable manifest is an EMPTY fleet.
pub async fn read_manifest() -> Vec<ManifestEntry> {
    let Ok(raw) = tokio::fs::read_to_string(fleet_dir().join("fleet.json")).await else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<ManifestEntry>>(&raw).unwrap_or_default()
}
