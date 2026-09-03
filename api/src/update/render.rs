// The slot render — what the updater-owned compose project looks like,
// derived from the LIVE container on every roll. The fleet renders agents
// from a chassis file an operator edits; this renders from what's running,
// because post-adoption there is no orchestrator env to consult — the
// container's own env, binds, and networks ARE the source of truth, and
// every roll re-reads them so drift cannot accumulate across rolls.
//
// THE SLOT CONTRACT (docker/compose.yml's, carried over unchanged):
//   - env: the live container's, verbatim except the denylist below —
//     secrets ride in the slot env file the way the fleet's .env carries
//     agent keys: 0600, beside the compose file.
//   - binds: same-path state + the docker socket, verbatim — the fleet
//     renderer's absolute-host-path rule depends on the state bind staying
//     same-path, and the socket is how the app spawns anything at all.
//   - dns / group_add / security_opt: verbatim (the stub-resolver and
//     docker-gid problems the base compose solves don't vanish).
//   - healthcheck: byte-identical to the base compose's — /api/healthz,
//     which flips ok only when the DB answers AND migrations passed. The
//     edge's docker provider refuses starting/unhealthy containers, so
//     THIS healthcheck, verbatim, is the roll gate.
//   - NO published ports on the slots. The edge owns the host port; two
//     slots both publishing it is the port war this shape exists to avoid.
//
// THE DENYLIST — env that must NOT survive the copy, each for its reason:
//   TALARIA_VERSION   the new image's label/env must win, or the healthz
//                     version gate reads the OLD version on the NEW
//                     container (the freshness signal would lie).
//   TALARIA_INSTALL   the image's own statement; copied stale from an
//                     older container it could only contradict the image.
//   TALARIA_UPDATER   deployments that supervise the app set it `off` (the
//                     image itself stopped baking that when this engine
//                     landed — a baked off would have leaked through the
//                     image's ENV into every adopted slot). Adoption is the
//                     flip of this switch: the render drops it, the state
//                     row's `migrated` becomes the dormancy gate, and an
//                     operator who wants the engine off again sets it in
//                     the slot env file (the documented door).
//   HOSTNAME/PATH/HOME container-runtime injections, not configuration.

use serde_json::{Value, json};

use super::docker::FLEET_ALIAS;
use super::layout::{Slot, edge_image, slot_env_file, slot_service};

/// Env names the render drops from the live container's environment — see
/// the header for each one's reason.
pub const ENV_DENYLIST: [&str; 6] = [
    "TALARIA_VERSION",
    "TALARIA_INSTALL",
    "TALARIA_UPDATER",
    "HOSTNAME",
    "PATH",
    "HOME",
];

/// The docker-gid group (base compose's DOCKER_GID) rides the env in some
/// installs; the bind for the socket itself is in `binds`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SlotSpec {
    /// The host port the world dials (from the live container's port
    /// bindings) — the EDGE publishes it; the slots never do.
    pub host_port: String,
    /// Verbatim `HostConfig.Binds` (same-path state bind + docker.sock).
    pub binds: Vec<String>,
    /// Verbatim `HostConfig.GroupAdd` (the docker socket's host gid).
    pub group_add: Vec<String>,
    /// Verbatim `HostConfig.Dns`.
    pub dns: Vec<String>,
    /// Verbatim `HostConfig.SecurityOpt`.
    pub security_opt: Vec<String>,
    /// The sidecar network (`<project>_internal`) — slots and edge join it
    /// as an EXTERNAL network; it stays owned by the original project.
    pub internal_network: String,
    /// The shared fleet network the `talaria` alias lives on (the live
    /// container's TALARIA_FLEET_NETWORK, base default "talaria").
    pub fleet_network: String,
    /// The live container's env as KEY=VALUE lines, denylist applied.
    pub env: Vec<String>,
}

/// Read the spec out of this container's inspect document. Pure — the
/// docker call is `inspect_self` (docker.rs), and the pure half is what
/// the tests pin against a fixture.
pub fn slot_spec_from_inspect(doc: &Value) -> Result<SlotSpec, String> {
    let host = doc
        .get("HostConfig")
        .ok_or("the inspect document has no HostConfig")?;
    let config = doc
        .get("Config")
        .ok_or("the inspect document has no Config")?;

    // The published host port: PortBindings maps "5273/tcp" → host ports.
    let host_port = host
        .get("PortBindings")
        .and_then(|b| b.as_object())
        .and_then(|m| m.values().find_map(|v| v.as_array()))
        .and_then(|a| a.first())
        .and_then(|b| b.get("HostPort"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or("the live container publishes no host port — nothing for the edge to own")?;

    let lines = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    let raw_env = lines(config.get("Env"));
    if raw_env.is_empty() {
        return Err("the live container has no environment to inherit".into());
    }
    let env: Vec<String> = raw_env
        .into_iter()
        .filter(|l| {
            let name = l.split('=').next().unwrap_or("");
            !ENV_DENYLIST.contains(&name)
        })
        .collect();
    let env_value = |key: &str| -> Option<String> {
        env.iter()
            .find_map(|l| l.split_once(&format!("{key}=")).map(|(_, v)| v.to_string()))
    };

    // The two networks, each by an explicit signal rather than position:
    // the sidecar net by its name (`<project>_internal` — the compose
    // convention both dokploy and the public compose follow), the fleet
    // net by the env that names it (TALARIA_FLEET_NETWORK, the very knob
    // the public compose documents for multi-instance hosts).
    let networks = doc
        .get("NetworkSettings")
        .and_then(|n| n.get("Networks"))
        .and_then(Value::as_object)
        .ok_or("the inspect document names no networks")?;
    let internal_network = networks
        .keys()
        .find(|n| n.as_str() == "internal" || n.ends_with("_internal"))
        .cloned()
        .ok_or("the live container is on no <project>_internal network — the sidecars would be unreachable")?;
    let fleet_network =
        env_value("TALARIA_FLEET_NETWORK").unwrap_or_else(|| FLEET_ALIAS.to_string());

    Ok(SlotSpec {
        host_port,
        binds: lines(host.get("Binds")),
        group_add: lines(host.get("GroupAdd")),
        dns: lines(host.get("Dns")),
        security_opt: lines(host.get("SecurityOpt")),
        internal_network,
        fleet_network,
        env,
    })
}

/// The image reference for a digest in this install's tracked repo —
/// `repo@sha256:…`, digest-pinned. The repo comes from the tracked ref
/// (private forks and the E2E registry included); only the digest varies.
pub fn digest_ref(repo_without_tag: &str, digest: &str) -> String {
    format!("{repo_without_tag}@{digest}")
}

/// Strip the tag (or digest) off a reference, leaving registry/repository.
pub fn repo_of(reference: &str) -> String {
    // Everything before the last ':' that follows the final '/' is the
    // repo; a digest separator behaves the same.
    let no_digest = reference.split('@').next().unwrap_or(reference);
    match no_digest.rsplit_once(':') {
        Some((repo, tag)) if !repo.is_empty() && !tag.contains('/') => repo.to_string(),
        _ => no_digest.to_string(),
    }
}

/// Render the updater-owned compose file as a JSON tree (yaml11_emit turns
/// it into YAML a 1.1 reader resolves identically). Both slots always
/// present, flip-render: `active` carries the digest it already runs (so
/// `compose up` leaves it alone), the incoming slot carries `incoming`.
/// The edge carries the host port and a healthcheck of its own (traefik's
/// ping), so boot reconcile can gate on it.
pub fn render_update_compose(
    spec: &SlotSpec,
    repo_without_tag: &str,
    active: Slot,
    active_digest: &str,
    incoming_digest: &str,
) -> Value {
    let slot = |slot: Slot, digest: &str| {
        let mut service = json!({
            "image": digest_ref(repo_without_tag, digest),
            "restart": "unless-stopped",
            // No ports — see the header. The edge owns the host port.
            "env_file": format!("{}.env", slot_service(slot)),
            "healthcheck": {
                // Byte-identical to docker/compose.yml's app healthcheck —
                // the roll gate IS this check.
                "test": ["CMD-SHELL", "wget -qO- http://127.0.0.1:5273/api/healthz >/dev/null 2>&1 || exit 1"],
                "interval": "10s",
                "timeout": "5s",
                "retries": 30,
                "start_period": "90s",
            },
            // Identical labels on both slots: one traefik service, two
            // backends, health-gated — the edge balances between them and
            // refuses the one that isn't healthy.
            "labels": [
                "traefik.enable=true",
                "traefik.http.routers.talaria.rule=PathPrefix(`/`)",
                "traefik.http.routers.talaria.entrypoints=talaria",
                "traefik.http.services.talaria.loadbalancer.server.port=5273",
            ],
            "networks": ["internal"],
        });
        let obj = service.as_object_mut().expect("the service literal");
        if !spec.binds.is_empty() {
            obj.insert("volumes".into(), json!(spec.binds));
        }
        if !spec.group_add.is_empty() {
            obj.insert("group_add".into(), json!(spec.group_add));
        }
        if !spec.dns.is_empty() {
            obj.insert("dns".into(), json!(spec.dns));
        }
        if !spec.security_opt.is_empty() {
            obj.insert("security_opt".into(), json!(spec.security_opt));
        }
        service
    };

    json!({
        "services": {
            slot_service(Slot::A): slot(Slot::A, if active == Slot::A { active_digest } else { incoming_digest }),
            slot_service(Slot::B): slot(Slot::B, if active == Slot::B { active_digest } else { incoming_digest }),
            "edge": {
                "image": edge_image(),
                "command": [
                    "--providers.docker",
                    "--providers.docker.exposedbydefault=false",
                    // Scoped to OUR internal network: the socket sees every
                    // container on the host, and only ours may ever route.
                    format!("--providers.docker.network={}", spec.internal_network),
                    "--providers.docker.endpoint=unix:///var/run/docker.sock",
                    "--entrypoints.talaria.address=:80",
                    // The internal entrypoint ping answers the healthcheck
                    // below; it publishes nothing.
                    "--entrypoints.traefik.address=:8080",
                    "--ping=true",
                ],
                "ports": [format!("{}:80", spec.host_port)],
                "volumes": ["/var/run/docker.sock:/var/run/docker.sock:ro"],
                "healthcheck": {
                    "test": ["CMD-SHELL", "wget -qO- http://127.0.0.1:8080/ping >/dev/null 2>&1 || exit 1"],
                    "interval": "10s",
                    "timeout": "5s",
                    "retries": 12,
                    "start_period": "10s",
                },
                "restart": "unless-stopped",
                "networks": ["internal"],
            },
        },
        "networks": {
            // External on purpose: the sidecars keep running in the project
            // that owns this network; the slots join it, never recreate it.
            "internal": {
                "name": spec.internal_network,
                "external": true,
            },
        },
    })
}

/// Write one slot's env file at 0600 beside the compose (fleet .env
/// discipline: plaintext credentials, owner-only mode, chmod after write
/// so an existing world-readable file is fixed too).
pub async fn write_slot_env(slot: Slot, lines: &[String]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;
    let path = slot_env_file(slot);
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("update dir unwritable ({}): {e}", dir.display()))?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .await
        .map_err(|e| format!("slot env unwritable ({}): {e}", path.display()))?;
    file.write_all(lines.join("\n").as_bytes())
        .await
        .map_err(|e| format!("slot env write failed ({}): {e}", path.display()))?;
    file.write_all(b"\n")
        .await
        .map_err(|e| format!("slot env write failed ({}): {e}", path.display()))?;
    if let Ok(meta) = tokio::fs::metadata(&path).await {
        let mut p = meta.permissions();
        p.set_mode(0o600);
        let _ = tokio::fs::set_permissions(&path, p).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::render::yaml11_emit;
    use serde_yaml_ng::Value as Yaml;

    /// A live fleet container's inspect shape, trimmed to what the spec
    /// reads (the real document is hundreds of keys; the fixture carries
    /// exactly the load-bearing ones, values included for the assertions).
    fn inspect_fixture() -> Value {
        json!({
            "Config": {
                "Env": [
                    "PORT=5273",
                    "DATABASE_URL=postgres://talaria:sekrit@postgres:5432/talaria",
                    "TALARIA_FLEET_PROJECT=talaria-fleet-outcrop-labs",
                    "TALARIA_FLEET_NETWORK=talaria-net-outcrop",
                    "TALARIA_VERSION=sha-old",
                    "TALARIA_INSTALL=image",
                    "TALARIA_UPDATER=off",
                    "HOSTNAME=abc123",
                    "PATH=/usr/bin",
                    "HOME=/home/talaria",
                ],
            },
            "HostConfig": {
                "PortBindings": {
                    "5273/tcp": [{"HostIp": "", "HostPort": "5302"}]
                },
                "Binds": [
                    "/var/lib/talaria:/var/lib/talaria",
                    "/var/run/docker.sock:/var/run/docker.sock"
                ],
                "GroupAdd": ["994"],
                "Dns": ["1.1.1.1", "1.0.0.1"],
                "SecurityOpt": ["no-new-privileges:true"],
            },
            "NetworkSettings": {
                "Networks": {
                    "compose-hack-bluetooth-application-uqkmlh_internal": {},
                    "talaria-net-outcrop": {},
                    "dokploy-network": {},
                }
            },
        })
    }

    fn spec() -> SlotSpec {
        slot_spec_from_inspect(&inspect_fixture()).expect("the fixture reads")
    }

    #[test]
    fn the_spec_reads_the_live_truth_and_denies_the_baked_switches() {
        let spec = spec();
        assert_eq!(spec.host_port, "5302");
        assert_eq!(
            spec.internal_network, "compose-hack-bluetooth-application-uqkmlh_internal",
            "the sidecar net by its name, not its position among three networks"
        );
        assert_eq!(
            spec.fleet_network, "talaria-net-outcrop",
            "the fleet net by the env that names it"
        );
        assert_eq!(spec.binds.len(), 2);
        assert_eq!(spec.group_add, vec!["994"]);
        assert_eq!(spec.dns, vec!["1.1.1.1", "1.0.0.1"]);
        // Everything operational survives verbatim — secrets included…
        assert!(
            spec.env
                .iter()
                .any(|l| l.starts_with("DATABASE_URL=postgres://talaria:sekrit"))
        );
        assert!(
            spec.env
                .iter()
                .any(|l| l == "TALARIA_FLEET_PROJECT=talaria-fleet-outcrop-labs")
        );
        // …and every denylisted name is gone.
        for gone in [
            "TALARIA_VERSION",
            "TALARIA_INSTALL",
            "TALARIA_UPDATER",
            "HOSTNAME",
            "PATH",
            "HOME",
        ] {
            assert!(
                !spec.env.iter().any(|l| l.starts_with(&format!("{gone}="))),
                "{gone} must not survive the copy"
            );
        }
    }

    #[test]
    fn an_inspect_without_a_published_port_refuses_to_render() {
        let mut doc = inspect_fixture();
        doc["HostConfig"]["PortBindings"] = json!({});
        assert!(slot_spec_from_inspect(&doc).is_err());
    }

    #[test]
    fn repo_stripping_handles_tags_and_digests() {
        assert_eq!(
            repo_of("ghcr.io/outcrop-labs/talaria:main"),
            "ghcr.io/outcrop-labs/talaria"
        );
        assert_eq!(
            repo_of("ghcr.io/outcrop-labs/talaria@sha256:aa"),
            "ghcr.io/outcrop-labs/talaria"
        );
        assert_eq!(
            repo_of("localhost:5000/talaria:main"),
            "localhost:5000/talaria"
        );
        assert_eq!(
            digest_ref("ghcr.io/outcrop-labs/talaria", "sha256:bb"),
            "ghcr.io/outcrop-labs/talaria@sha256:bb"
        );
    }

    /// The render, emitted and parsed back — the same round trip the file
    /// on disk goes through. Every slot contract line asserts here.
    #[test]
    fn the_compose_round_trips_every_slot_contract() {
        let spec = spec();
        let tree = render_update_compose(
            &spec,
            "ghcr.io/outcrop-labs/talaria",
            Slot::A,
            "sha256:old",
            "sha256:new",
        );
        let text = yaml11_emit(&tree);
        let yaml: Yaml = serde_yaml_ng::from_str(&text).expect("the render is YAML");
        let services = yaml.get("services").expect("services block");

        // Both slots, digest-pinned, flip-rendered: A holds what it runs,
        // B carries the incoming digest.
        let a = services.get("app").expect("slot a");
        let b = services.get("app-b").expect("slot b");
        assert_eq!(a["image"], "ghcr.io/outcrop-labs/talaria@sha256:old");
        assert_eq!(b["image"], "ghcr.io/outcrop-labs/talaria@sha256:new");
        // No ports on either slot — the edge owns the host port.
        assert!(a.get("ports").is_none() && b.get("ports").is_none());
        // The healthcheck is byte-identical to the base compose's.
        for slot in [a, b] {
            assert_eq!(
                slot["healthcheck"]["test"][1],
                "wget -qO- http://127.0.0.1:5273/api/healthz >/dev/null 2>&1 || exit 1"
            );
            assert_eq!(slot["healthcheck"]["retries"], 30);
            assert_eq!(slot["healthcheck"]["start_period"], "90s");
            // Identical traefik labels — one service, two backends.
            assert_eq!(a["labels"], slot["labels"]);
            // Binds and docker-gid survive verbatim.
            assert_eq!(slot["volumes"][0], "/var/lib/talaria:/var/lib/talaria");
            assert_eq!(
                slot["volumes"][1],
                "/var/run/docker.sock:/var/run/docker.sock"
            );
            assert_eq!(slot["group_add"][0], "994");
            assert_eq!(slot["dns"][0], "1.1.1.1");
            // Its own env file, per slot.
            assert!(slot["env_file"].as_str().unwrap().ends_with(".env"));
            // The internal network only — the fleet network is a runtime
            // alias attach, never in the file.
            assert_eq!(slot["networks"][0], "internal");
        }

        // The edge: the ONLY thing publishing, on the live host port, into
        // the talaria entrypoint; provider scoped to our internal network.
        let edge = services.get("edge").expect("edge");
        assert_eq!(edge["ports"][0], "5302:80");
        let command: Vec<&str> = edge["command"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(command.contains(&"--providers.docker.exposedbydefault=false"));
        assert!(command.contains(
            &"--providers.docker.network=compose-hack-bluetooth-application-uqkmlh_internal"
        ));
        assert!(command.contains(&"--entrypoints.talaria.address=:80"));
        assert!(
            edge["volumes"][0].as_str().unwrap().ends_with(":ro"),
            "the socket the provider reads is read-only"
        );
        assert!(
            edge["healthcheck"]["test"][1]
                .as_str()
                .unwrap()
                .contains(":8080/ping")
        );

        // The network is external — the sidecars' project owns it.
        let internal = yaml.get("networks").unwrap().get("internal").unwrap();
        assert_eq!(internal["external"], true);
        assert_eq!(
            internal["name"],
            "compose-hack-bluetooth-application-uqkmlh_internal"
        );
    }

    /// The flip: render with B active and A's digest is the incoming one —
    /// same contract, mirrored.
    #[test]
    fn the_render_flips() {
        let tree = render_update_compose(
            &spec(),
            "ghcr.io/outcrop-labs/talaria",
            Slot::B,
            "sha256:old",
            "sha256:new",
        );
        let services = tree.get("services").unwrap();
        assert_eq!(
            services["app-b"]["image"],
            "ghcr.io/outcrop-labs/talaria@sha256:old"
        );
        assert_eq!(
            services["app"]["image"],
            "ghcr.io/outcrop-labs/talaria@sha256:new"
        );
    }
}
