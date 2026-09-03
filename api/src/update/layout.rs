// The update engine's LAYOUT — where the updater's compose project lives and
// what every docker-level name resolves to (mirrors fleet/layout.rs, whose
// invariant this inherits: one path rule, one definition, because every
// write-plane caller resolves through these).
//
// THE PROJECT INVARIANT, the fleet's carried over: one updater-owned compose
// project per docker host is the identity the whole lifecycle assumes —
// anything sharing a host with an adopted instance must drive its own
// project or the two reconcile each other's containers. The project is
// OUTSIDE the fleet's (agents reconcile by the fleet project's prefix; the
// app slots reconcile by this one's) but the two share the fleet NETWORK at
// runtime: agents dial the app by its `talaria` alias on that network, and
// the roll moves the alias rather than the DNS the fleet bakes.

use std::path::PathBuf;

/// The flip-render slots, same shape as the fleet's — one active, one
/// incoming, and the roll renders the incoming beside the active one
/// (fleet/docker.rs owns the enum; the app slots are its second user).
pub use crate::fleet::docker::Slot;

/// The update tree (rendered compose, slot env files, project state) —
/// Talaria-owned, host-real because the rendered compose's binds must be
/// host paths. Containers default this to /var/lib/talaria/update; dev and
/// the E2E point it somewhere disposable.
pub fn update_dir() -> PathBuf {
    match std::env::var("TALARIA_UPDATE_DIR") {
        Ok(d) => PathBuf::from(d),
        // A set-but-empty env var is used verbatim; var() errs only on
        // unset/invalid UTF-8, so the Err arm is exactly the unset case —
        // the state root the image's other subtree envs (FLEET_DIR,
        // UPLOADS_DIR, APPS_DIR) already name.
        Err(_) => PathBuf::from("/var/lib/talaria/update"),
    }
}

/// The updater-owned compose project — see the header invariant.
pub fn update_project() -> String {
    std::env::var("TALARIA_UPDATE_PROJECT").unwrap_or_else(|_| "talaria-update".into())
}

/// The compose file the renderer materializes and every docker verb
/// addresses (fleet's docker-compose.yml convention, same layout).
pub fn compose_file() -> PathBuf {
    update_dir().join("compose.yml")
}

/// The compose service name for an app slot (`app` / `app-b`, the fleet's
/// agent-<dept>[-b] shape).
pub fn slot_service(slot: Slot) -> String {
    match slot {
        Slot::A => "app".into(),
        Slot::B => "app-b".into(),
    }
}

/// The container docker actually names for that slot (compose project +
/// service + replica index — compose's naming, not a choice).
pub fn slot_container(slot: Slot) -> String {
    slot_container_in(&update_project(), slot)
}

/// The naming, as a pure function of the project — compose's rule, pinned
/// by test without reaching for the process env.
pub fn slot_container_in(project: &str, slot: Slot) -> String {
    format!("{}-{}-1", project, slot_service(slot))
}

/// The slot's env file (rendered from the live container on every roll —
/// post-adoption env edits go through these files, never through the
/// orchestrator that no longer owns the app).
pub fn slot_env_file(slot: Slot) -> PathBuf {
    update_dir().join(format!("{}.env", slot_service(slot)))
}

/// The edge service — the per-VM traefik that owns the host port after
/// adoption and routes to whichever slot is healthy (its docker provider
/// refuses `starting`/`unhealthy` containers, so the slots' compose
/// healthcheck IS the roll gate).
pub const EDGE_SERVICE: &str = "edge";

/// The edge's container name (compose project + service + replica index).
pub fn edge_container() -> String {
    edge_container_in(&update_project())
}

/// The naming, pure in the project (slot_container_in's twin).
pub fn edge_container_in(project: &str) -> String {
    format!("{}-{}-1", project, EDGE_SERVICE)
}

/// The edge image — pinned, never `:latest`, for the same reason every
/// dependency in this tree is pinned: the edge is on the request path, and
/// an unpinned traefik upgrade is an unreviewed change to routing itself.
pub fn edge_image() -> String {
    std::env::var("TALARIA_EDGE_IMAGE")
        .unwrap_or_else(|_| "docker.io/library/traefik:v3.6.7".into())
}

/// The app image the engine tracks — the trunk feed CI publishes from main
/// (app-image.yml). One override serves private forks (their own published
/// image) and the devbox E2E (a localhost registry); the tag here is the
/// only moving ref the engine ever READS, and only to resolve a digest.
pub fn default_image_ref() -> String {
    std::env::var("TALARIA_UPDATE_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/outcrop-labs/talaria:main".into())
}

/// How long the old slot keeps serving after cutover so in-flight requests
/// drain — the SAME knob the fleet's agent rolls use (one drain policy per
/// host; two policies would let the app and its agents disagree about what
/// a graceful stop means).
pub fn roll_drain_ms() -> u64 {
    crate::fleet::reconcile::roll_drain_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_name_the_app_services_the_fleet_shape() {
        assert_eq!(slot_service(Slot::A), "app");
        assert_eq!(slot_service(Slot::B), "app-b");
        // Compose's container naming, pinned against the default project.
        assert_eq!(
            slot_container_in("talaria-update", Slot::A),
            "talaria-update-app-1"
        );
        assert_eq!(
            slot_container_in("talaria-update", Slot::B),
            "talaria-update-app-b-1"
        );
        assert_eq!(edge_container_in("talaria-update"), "talaria-update-edge-1");
    }
}
