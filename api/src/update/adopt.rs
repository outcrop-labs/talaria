// Adoption — the one-time handover from an orchestrator-deployed container
// (dokploy's, a plain `compose up`, anything that owns the port today) to
// the updater-owned slots + edge this engine rolls forever after. Everything
// else in api/src/update/ refuses to act until this has run (roll's
// acting_gate): the engine ships DORMANT, and adoption is the explicit,
// per-instance switch.
//
// THE SHAPE (two paths, one choreography):
//   gates → resolve the target pin (the registry's, else the running
//   image's own RepoDigest) → pull → render the fresh project (green =
//   slot a, the first updater-owned container this host has) → green up +
//   health gate → alias onto green → RECORD (migrated, pinned, edgePort,
//   retiredContainer — crash-safe BEFORE any port changes hands) → then:
//
//   FRESH-PORT (`edge_port` given): the edge binds the NEW port now —
//   nobody contends it — and BLUE KEEPS SERVING its own. The operator
//   repoints the proxy at the edge (the infra script drives the Cloudflare
//   API; a self-hoster, their own), and the SECOND adopt call — which can
//   only ARRIVE THROUGH THE EDGE, which is the proof the proxy moved —
//   stops blue and lands the run. Zero interruption at the origin.
//
//   INHERIT (`edge_port` omitted): the edge must own the port blue holds,
//   so blue must release it first — a one-time seconds-scale cut, named in
//   the panel's confirm dialog. Blue cannot run code after its own stop,
//   so it arms a host-side helper container (this very image, docker-cli
//   and compose baked in, the same sock) that polls blue every second and
//   raises the edge the moment the port frees; green's reconcile is the
//   crash fallback and the runbook's docker line the last resort.
//
// RESUME is adopt called again with `migrated` already true — by design,
// not an error: the fresh-port protocol is two calls BY CONSTRUCTION, and
// a crash at any step heals on the next one.

use redis::aio::ConnectionManager;
use serde_json::Value;
use sqlx::PgPool;

use super::docker::{
    attach_fleet_alias, container_running, edge_healthy, image_repo_digest, inspect_self,
    pull_image, remove_container, service_up, slot_up, stop_container, wait_healthy_slot,
};
use super::layout::{
    EDGE_SERVICE, Slot, compose_file, default_image_ref, roll_drain_ms, slot_container, update_dir,
    update_project,
};
use super::mode::install_mode;
use super::registry::{fetch_version_label, is_digest, parse_image_ref, resolve_latest};
use super::render::{digest_ref, repo_of, slot_spec_from_inspect};
use super::roll::{
    HEALTH_GATE_MS, ROLL_LOCK_TTL_MS, now_iso, reconcile_boot, roll_lease_key, run_in_flight,
    self_name, transition, write_project,
};
use super::state::{Pin, RunBy, RunRecord, RunState, UpdateState, load, patch, record_run};
use crate::fleet::docker::docker;
use crate::runs::lease::{AcquireResult, RedisLeases, acquire_lease, keep_lease_alive};

const LOG: &str = "[update]";

/// What one adopt call landed on — the route's answer and the migration
/// script's cue for what to do next.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AdoptStage {
    /// Green and the edge are up on a port nobody else held; blue still
    /// serves its own. Repoint the proxy, then call adopt AGAIN — the call
    /// must arrive through the edge, which is the proof the proxy moved.
    EdgeReady { edge_port: String },
    /// Traffic is green's through the edge; blue is stopped or stopping and
    /// the run has landed.
    Cutover { edge_port: String },
}

/// Which pin adoption lands on — pure, so the policy is pinnable by test.
/// The registry's resolved pin when it answered (adoption takes the newest
/// fully-published image, not merely the running one); else a re-pin of the
/// running image's own RepoDigest (a registry that will not answer must not
/// strand the install); else refusal — a local `docker build` image with no
/// RepoDigest is nothing the engine can re-deploy.
pub fn adopt_pin(
    resolved: Option<Pin>,
    running_digest: Option<&str>,
    running_version: Option<&str>,
) -> Result<Pin, String> {
    if let Some(pin) = resolved {
        return Ok(pin);
    }
    let digest = running_digest
        .filter(|d| is_digest(d))
        .ok_or("the registry did not answer and the running image carries no registry digest — there is nothing to re-deploy")?
        .to_string();
    Ok(Pin {
        digest,
        version: running_version.unwrap_or("adopted").to_string(),
    })
}

/// The moving-tag half of a reference, as a version LABEL — adoption's
/// re-pin fallback when the registry will not speak. A digest ref carries
/// none, and an implied `latest` says less than "adopted".
fn tag_of(reference: &str) -> Option<String> {
    if reference.contains('@') {
        return None;
    }
    parse_image_ref(reference)
        .map(|r| r.tag)
        .filter(|t| t != "latest")
}

/// This container's own inspect document, as a name (docker prefixes
/// inspect's Name with a slash; every verb here takes it without).
fn name_of(doc: &Value) -> Result<String, String> {
    doc.get("Name")
        .and_then(Value::as_str)
        .map(|n| n.trim_start_matches('/').to_string())
        .ok_or_else(|| "the self inspect document carries no name".into())
}

/// The image reference a container runs (`Config.Image`).
fn image_of(doc: &Value) -> Option<String> {
    doc.get("Config")
        .and_then(|c| c.get("Image"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The first host port this container publishes — render.rs's own read of
/// PortBindings, mirrored here for the resume path's inherit-or-fresh
/// question: does the recorded edge port equal the one blue is holding?
fn own_host_port(doc: &Value) -> Option<String> {
    doc.get("HostConfig")?
        .get("PortBindings")?
        .as_object()?
        .values()
        .find_map(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|b| b.get("HostPort"))
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

/// The edge-boot helper's container name — deterministic, so a stale armer
/// from a prior attempt is removable by name before re-arming.
fn edge_boot_container() -> String {
    edge_boot_container_in(&update_project())
}

/// The naming, pure in the project (layout's slot_container_in shape —
/// pinnable by test without touching the process env).
fn edge_boot_container_in(project: &str) -> String {
    format!("{project}-edge-boot")
}

/// The helper's whole job, one shell line: wait out blue's drain, then
/// raise the edge on the port that just freed. Pure so the shape is
/// pinnable by test.
///
/// A denied docker socket must read as DENIED, not as "not running": the
/// first shape swallowed inspect's stderr (`2>/dev/null | grep -q true`),
/// so a helper that could not speak to the daemon at all saw its own
/// permission error as blue already dead, skipped the wait, failed the
/// compose, and died — leaving the port freed and the edge never raised
/// (the bbills stranding). Now the daemon being unreachable is a loud
/// exit with a breadcrumb in the update dir — `--rm` erases the container,
/// the log is what remains — and the compose's own output lands there too,
/// success or failure.
fn edge_boot_script(retired: &str, compose: &std::path::Path, project: &str) -> String {
    format!(
        "log=$(dirname {compose})/edge-boot.log; i=0; while :; do \
         s=\"$(docker inspect -f '{{{{.State.Running}}}}' {retired} 2>&1)\" \
         || {{ echo \"$s\" >>\"$log\"; exit 1; }}; \
         [ \"$s\" = true ] || break; \
         sleep 1; i=$((i+1)); [ $i -gt 600 ] && exit 1; done; \
         docker compose -f {compose} -p {project} up -d edge >>\"$log\" 2>&1 \
         || {{ echo compose-failed >>\"$log\"; exit 1; }}",
        compose = compose.display()
    )
}

/// The host group ids this container carries (`HostConfig.GroupAdd`) — the
/// docker socket's gid among them. Pure, pinnable by test.
fn group_adds_of(doc: &Value) -> Vec<String> {
    doc.get("HostConfig")
        .and_then(|h| h.get("GroupAdd"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|g| g.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Arm the host-side helper: a detached container of the RUNNING app image
/// (definitionally local, docker-cli and compose baked in — the same
/// premise every verb here stands on), the sock and the update dir mounted
/// same-path, polling blue's state every second. The inherit path's only
/// way to raise the edge after its own container dies — nothing inside
/// blue outlives blue.
///
/// The sock is 0660 root:docker and the app image runs as its own non-root
/// user — the very reason the slot compose renders `group_add`. The helper
/// needs the same gids or it is blind: every docker call denied from the
/// first inspect on (verified on a live host: no `--group-add`, the daemon
/// is unreachable; with it, it answers).
async fn arm_edge_boot(image: &str, retired: &str, group_add: &[String]) -> Result<(), String> {
    let _ = remove_container(&edge_boot_container()).await; // a stale armer blocks the name
    let script = edge_boot_script(retired, &compose_file(), &update_project());
    let dir = update_dir().display().to_string();
    let bind = format!("{dir}:{dir}");
    let mut argv: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--rm".into(),
        "--name".into(),
        edge_boot_container(),
        "-v".into(),
        "/var/run/docker.sock:/var/run/docker.sock".into(),
        "-v".into(),
        bind,
        "--entrypoint".into(),
        "sh".into(),
    ];
    for gid in group_add {
        argv.push("--group-add".into());
        argv.push(gid.clone());
    }
    argv.push(image.to_string());
    argv.push("-c".into());
    argv.push(script);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    docker(&args, std::time::Duration::from_secs(60))
        .await
        .map(|_| ())
        .map_err(|e| format!("the edge-boot helper would not arm: {e}"))
}

/// The handover itself. The FIRST call is long (a pull can run 15 minutes)
/// — the route runs it detached and the caller polls; the RESUME call is
/// fast and synchronous.
pub async fn adopt(
    pg: &PgPool,
    conn: ConnectionManager,
    edge_port: Option<String>,
) -> Result<AdoptStage, String> {
    if let Some(sentence) = install_mode().refusal() {
        return Err(format!("adoption is for image installs — {sentence}"));
    }
    let state = load(pg).await;
    if state.migrated {
        return resume(pg, &state).await;
    }
    if state.last_run.as_ref().is_some_and(run_in_flight) {
        return Err("an update is in flight — let it land before adopting".into());
    }

    // The lock: adoption moves the same two containers a roll does, and the
    // same never-release law applies (the holder's last act may be its own
    // container's stop). The RESUME path takes no lock — see its comment.
    let mut backend = RedisLeases::new(conn.clone());
    let token = match acquire_lease(&mut backend, &roll_lease_key(), ROLL_LOCK_TTL_MS).await {
        AcquireResult::Acquired(t) => t,
        AcquireResult::Held => {
            return Err("another update holds the roll lock".into());
        }
        AcquireResult::Unavailable(e) => {
            return Err(format!("the roll lock could not be taken (redis): {e}"));
        }
    };
    let _beat = keep_lease_alive(conn, token, ROLL_LOCK_TTL_MS, Default::default());

    // Who and where am I: the orchestrator's own app container.
    let self_doc = inspect_self().await?;
    let blue = name_of(&self_doc)?;
    let running_ref = image_of(&self_doc).ok_or("the self inspect document names no image")?;
    let running_digest = image_repo_digest(&running_ref).await;

    // What we adopt ONTO: the registry's newest when it speaks, else the
    // running image's own digest. The label beside a re-pin is best-effort
    // — in exactly that branch the registry is down — with the running
    // ref's tag as the fallback word.
    let resolved = resolve_latest().await.ok();
    let version_hint = if resolved.is_none()
        && let Some(digest) = running_digest.clone()
        && let Some(image) = parse_image_ref(&default_image_ref())
    {
        fetch_version_label(&image, &digest).await.ok()
    } else {
        None
    }
    .or_else(|| tag_of(&running_ref));
    // The pull repo follows the pin's ORIGIN: the tracked repo when the
    // registry answered, else the repo the running image was pulled from
    // (a re-pin's digest lives THERE — the tracked repo may not even carry
    // it). Resolved before adopt_pin consumes the answer.
    let pull_repo = if resolved.is_some() {
        repo_of(&default_image_ref())
    } else {
        repo_of(&running_ref)
    };
    let pin = adopt_pin(resolved, running_digest.as_deref(), version_hint.as_deref())?;
    let reference = digest_ref(&pull_repo, &pin.digest);

    // The spec off the LIVE container — env, binds, networks — with the
    // edge's port decided: an explicit one wins (fresh-port path, a port
    // nobody holds); else blue's own binding is the port the world already
    // dials (inherit path).
    let mut spec = slot_spec_from_inspect(&self_doc, edge_port.as_deref())?;
    let port = match &edge_port {
        Some(p) => {
            spec.host_port = p.clone();
            p.clone()
        }
        None => spec.host_port.clone(),
    };

    let from = running_digest
        .map(|digest| Pin {
            digest,
            version: version_hint.clone().unwrap_or_else(|| "adopted".into()),
        })
        .unwrap_or_else(|| Pin {
            digest: String::new(),
            version: String::new(),
        });
    let started = now_iso();
    let fresh = || RunRecord {
        state: RunState::Pulling,
        from: from.clone(),
        to: pin.clone(),
        by: RunBy::Manual,
        started_at: started.clone(),
        finished_at: None,
        error: None,
    };

    // Pull, recorded, before any container exists — the panel can watch.
    transition(pg, Some(fresh()), |r| r.state = RunState::Pulling).await?;
    if let Err(e) = pull_image(&reference).await {
        transition(pg, Some(fresh()), |r| {
            r.state = RunState::Failed;
            r.finished_at = Some(now_iso());
            r.error = Some(e.clone());
        })
        .await
        .ok();
        return Err(format!("the pull failed: {e}"));
    }

    // The fresh project: green is slot A — the FIRST updater-owned
    // container this host sees — and slot B is declared at the same pin so
    // the file is steady-state correct the moment adoption lands.
    write_project(&spec, Slot::B, &pin.digest, Slot::A, &pin.digest).await?;
    transition(pg, Some(fresh()), |r| r.state = RunState::Starting).await?;
    if let Err(e) = slot_up(Slot::A).await {
        let _ = remove_container(&slot_container(Slot::A)).await;
        transition(pg, Some(fresh()), |r| {
            r.state = RunState::Failed;
            r.finished_at = Some(now_iso());
            r.error = Some(e.clone());
        })
        .await
        .ok();
        return Err(format!("the first updater-owned slot did not start: {e}"));
    }
    if !wait_healthy_slot(Slot::A, HEALTH_GATE_MS).await {
        let _ = remove_container(&slot_container(Slot::A)).await;
        let sentence =
            "the new slot never became healthy — the orchestrator's container kept serving"
                .to_string();
        transition(pg, Some(fresh()), |r| {
            r.state = RunState::Failed;
            r.finished_at = Some(now_iso());
            r.error = Some(sentence.clone());
        })
        .await
        .ok();
        return Err(sentence);
    }

    // The agent-plane cutover: the shared alias moves to green. Blue keeps
    // the network and the alias until its stop — two HEALTHY backends on
    // the same DB, the same overlap a roll accepts.
    if let Err(e) = attach_fleet_alias(&spec.fleet_network, &slot_container(Slot::A)).await {
        let _ = remove_container(&slot_container(Slot::A)).await;
        let sentence = format!("the fleet alias did not move: {e}");
        transition(pg, Some(fresh()), |r| {
            r.state = RunState::Failed;
            r.finished_at = Some(now_iso());
            r.error = Some(sentence.clone());
        })
        .await
        .ok();
        return Err(sentence);
    }

    // THE RECORD — crash-safe before any port changes hands. From here on,
    // any reader sees an adopted instance with an open run; a crash at any
    // later step heals on the next adopt call (resume, below).
    let port_for_record = port.clone();
    let blue_for_record = blue.clone();
    patch(pg, move |mut s| {
        s.migrated = true;
        s.pinned = Some(pin.clone());
        s.edge_port = Some(port_for_record.clone());
        s.retired_container = Some(blue_for_record.clone());
        record_run(
            &mut s,
            RunRecord {
                state: RunState::CuttingOver,
                from: from.clone(),
                to: pin.clone(),
                by: RunBy::Manual,
                started_at: started.clone(),
                finished_at: None,
                error: None,
            },
        );
        s
    })
    .await
    .map_err(|e| format!("the adoption record did not write: {e}"))?;

    if edge_port.is_some() {
        // FRESH-PORT: the edge binds a port nobody holds; blue keeps serving
        // its own until the proxy repoints and the second call — through
        // the edge — finishes the handover.
        if let Err(e) = service_up(EDGE_SERVICE).await {
            return Err(format!(
                "the edge did not start on {port}: {e} — the orchestrator's container still serves; call adopt again to retry"
            ));
        }
        if !edge_healthy(60_000).await {
            return Err(format!(
                "the edge did not come up healthy on {port} — the orchestrator's container still serves; call adopt again to retry"
            ));
        }
        tracing::info!(
            "{LOG} adoption: green and the edge are up on {port}; the orchestrator's container serves on until the proxy repoints"
        );
        Ok(AdoptStage::EdgeReady { edge_port: port })
    } else {
        // INHERIT: arm the helper, then blue's last act. The drain carries
        // whatever in-flight work this container still holds; the helper
        // raises the edge the second the port frees.
        arm_edge_boot(&running_ref, &blue, &group_adds_of(&self_doc)).await?;
        tracing::info!(
            "{LOG} adoption: green healthy, edge armed, stopping {blue} — the one-time cut"
        );
        let _ = stop_container(&blue, roll_drain_ms()).await;
        Ok(AdoptStage::Cutover { edge_port: port })
    }
}

/// The second-and-later calls: `migrated` is already true. Takes NO lock,
/// deliberately: the roll lock's holder is the FIRST call's process (blue,
/// alive and renewing on the fresh-port path — precisely so nothing else
/// moves containers mid-handover), the run-in-flight gate already bars
/// every other actor from touching anything, and the resume's whole job is
/// to finish exactly that run. Idempotent by construction: called on blue
/// it answers the hold again, called on green it stops a possibly-stopped
/// container and reconciles.
async fn resume(pg: &PgPool, state: &UpdateState) -> Result<AdoptStage, String> {
    let port = state.edge_port.clone().ok_or(
        "the state row is migrated but records no edge port — a broken adoption; inspect the update dir by hand",
    )?;
    let me = self_name().await.ok();
    let retired = state.retired_container.clone();

    if me.as_deref() == retired.as_deref() {
        // On BLUE. If the edge is up, this call did not come through it —
        // the proxy has not repointed — and the honest answer is the hold.
        if edge_healthy(5_000).await {
            return Ok(AdoptStage::EdgeReady { edge_port: port });
        }
        // The edge is down: a first call that died before its port work.
        // Which port work, though — blue's own binding inherited, or a
        // fresh one nobody held?
        let doc = inspect_self().await?;
        let image = image_of(&doc).unwrap_or_default();
        let Some(me) = me else {
            return Err(
                "this container's name would not resolve — finish the adoption by hand".into(),
            );
        };
        if own_host_port(&doc).as_deref() == Some(port.as_str()) {
            // INHERIT, died before the stop: arm and cut now.
            arm_edge_boot(&image, &me, &group_adds_of(&doc)).await?;
            tracing::info!("{LOG} adoption resume: re-armed, stopping this container");
            let _ = stop_container(&me, roll_drain_ms()).await;
            return Ok(AdoptStage::Cutover { edge_port: port });
        }
        // FRESH-PORT, died before the edge came up: the port is free.
        service_up(EDGE_SERVICE).await?;
        if !edge_healthy(60_000).await {
            return Err(format!(
                "the edge would not come up on {port} — inspect the update dir; the orchestrator's container still serves"
            ));
        }
        return Ok(AdoptStage::EdgeReady { edge_port: port });
    }

    // On GREEN (the call arrived through the edge — the proof the proxy
    // moved) or after blue is already gone: stop the retired if it lives,
    // then land the run exactly as the reconcile would. Ok(None) means the
    // run already landed (green's own tick, a prior call) — cutover either
    // way, not an error.
    if let Some(retired) = retired.filter(|r| Some(r.as_str()) != me.as_deref())
        && container_running(&retired).await
    {
        stop_container(&retired, roll_drain_ms())
            .await
            .map_err(|e| format!("the retired container did not stop: {e}"))?;
    }
    let _ = reconcile_boot(pg).await?;
    Ok(AdoptStage::Cutover { edge_port: port })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(v: &str) -> Pin {
        Pin {
            digest: format!("sha256:{:0>64}", v),
            version: v.into(),
        }
    }

    #[test]
    fn adopt_pin_takes_the_registrys_answer_when_it_has_one() {
        let got = adopt_pin(Some(pin("new")), Some(&pin("old").digest), Some("old-tag")).unwrap();
        assert_eq!(got, pin("new"), "adoption takes the newest image");
    }

    #[test]
    fn adopt_pin_repins_the_running_image_when_the_registry_is_silent() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let got = adopt_pin(None, Some(&digest), Some("sha-abc123")).unwrap();
        assert_eq!(got.digest, digest);
        assert_eq!(got.version, "sha-abc123");
        // No word for it → the honest placeholder.
        let bare = adopt_pin(None, Some(&digest), None).unwrap();
        assert_eq!(bare.version, "adopted");
    }

    #[test]
    fn adopt_pin_refuses_an_image_with_nothing_redeployable() {
        let err = adopt_pin(None, None, Some("v1")).unwrap_err();
        assert!(err.contains("no registry digest"), "{err}");
        // A non-digest string in the slot is not a pin either.
        assert!(adopt_pin(None, Some("main"), None).is_err());
    }

    #[test]
    fn tags_read_the_moving_half_only() {
        assert_eq!(
            tag_of("ghcr.io/outcrop-labs/talaria:main"),
            Some("main".into())
        );
        assert_eq!(tag_of("localhost:5000/talaria:v2"), Some("v2".into()));
        // A digest ref has no tag; an implied latest says nothing.
        assert_eq!(tag_of("ghcr.io/x/y@sha256:aa"), None);
        assert_eq!(tag_of("ghcr.io/x/y"), None);
    }

    #[test]
    fn the_edge_boot_helper_waits_out_the_drain_then_raises_the_edge() {
        let script = edge_boot_script(
            "dokploy-app-1",
            std::path::Path::new("/var/lib/talaria/update/compose.yml"),
            "talaria-update",
        );
        assert!(
            script.contains("'{{.State.Running}}' dokploy-app-1"),
            "{script}"
        );
        assert!(
            script.contains(
                "docker compose -f /var/lib/talaria/update/compose.yml -p talaria-update up -d edge"
            ),
            "{script}"
        );
        assert!(
            script.contains("[ $i -gt 600 ] && exit 1"),
            "bounded: {script}"
        );
        // A denied daemon reads as DENIED — the wait must not mistake its
        // own permission error for blue already dead.
        assert!(
            script.contains("|| { echo \"$s") && script.contains("edge-boot.log"),
            "denial is loud and leaves a breadcrumb: {script}"
        );
    }

    #[test]
    fn group_adds_read_the_host_config_array() {
        let doc: Value = serde_json::json!({
            "HostConfig": { "GroupAdd": ["993", "1001"] }
        });
        assert_eq!(group_adds_of(&doc), vec!["993", "1001"]);
        let bare: Value = serde_json::json!({ "HostConfig": {} });
        assert!(group_adds_of(&bare).is_empty());
        assert!(group_adds_of(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn own_host_port_reads_the_first_binding() {
        let doc: Value = serde_json::json!({
            "HostConfig": { "PortBindings": {
                "5273/tcp": [{ "HostPort": "5273" }]
            }}
        });
        assert_eq!(own_host_port(&doc).as_deref(), Some("5273"));
        let bare: Value = serde_json::json!({ "HostConfig": { "PortBindings": {} } });
        assert_eq!(own_host_port(&bare), None);
    }

    #[test]
    fn the_helper_names_after_the_project() {
        assert_eq!(
            edge_boot_container_in("talaria-update"),
            "talaria-update-edge-boot"
        );
    }
}
