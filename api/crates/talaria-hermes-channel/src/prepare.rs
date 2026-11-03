// THE PREPARATION — a candidate image on the host and read before anything
// runs it, and every rollout's image checked to be on the host before it
// moves an agent (an image that left — an operator's prune — is pulled
// again, never assumed).
//
// The pull is the agents' own shared pull (talaria-fleet-docker's pull.rs:
// one per key in the process, on a task of its own, killed only for silence
// or its ceiling), addressed through the canary's compose project so it
// never touches the fleet's. It gives way: while any other pull runs in the
// process (a hire's, a roll's), the preparation waits for the next tick
// rather than start a second 1 GB download beside it. Docker has no pull
// priority to set; waiting is the low priority.
//
// A PULL NEVER FILLS THE DISK. The image store sits on the filesystem docker
// keeps its volumes on — Postgres's among them — so before a pull starts the
// docker root must have room for the image (sized as the one the fleet runs
// now, a quarter again) and still keep a tenth of the filesystem free. A
// host without the room holds the rollout and tells the admins (engine.rs
// backs off and says it once).
//
// The inspection reads the image with networking off — `docker run --rm
// --network none` of the image's own Python, bounded in memory, processes and
// CPU, with nothing mounted and no secret in its environment — for what the
// render needs from it: `DEFAULT_CONFIG["_config_version"]`, the env names
// its Hermes strips from an agent's commands, its own version, and whether
// the s6 tools the drain relies on are where Talaria calls them. It also
// checks the Hermes internals Talaria relies on, so an upstream change that
// matters holds the release for an admin instead of passing silently:
//
//   schema    Talaria stamps a config with the image's own version, so
//             Hermes never migrates it. On Talaria's rendered config stamped
//             with the schema this release renders, what Hermes runs with
//             its migration skipped (its own `load_config`) must equal what
//             it runs once its own `migrate_config` has migrated it, on every
//             key Talaria writes — a migration that matters is one the stamp
//             would skip. (A rename Hermes' loader reads either way, as
//             v0.22.6 does `model.model` → `model.default`, passes; a key it
//             drops, as it does `browser.backend`, does not.)
//   secrets   a platform plugin declaring a `CONFIG_*` name (the sealed
//             manifest Talaria mounts) must put it in the set Hermes keeps
//             out of every command an agent runs.
//
// Its output is untrusted: parsed as one JSON object, every field checked
// and bounded.

use std::time::Duration;

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_fleet_docker::{PullStatus, docker};
use talaria_hermes_images::{self as images, ImageRecord};

/// The inspection's outer bound. The run is a second of Python, but on a
/// saturated disk docker took minutes to create the container (measured
/// 2026-11-03: five minutes at 60–80 % IO pressure).
const INSPECT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How long one step waits on the pull before it reports progress and
/// leaves the pull running for the next.
const PULL_SLICE: Duration = Duration::from_secs(20);

/// The room a pull must leave free on the docker root's filesystem.
const KEEP_FREE_SHARE: f64 = 0.10;

/// An image's unpacked size when the fleet's own cannot be read.
const IMAGE_SIZE_GUESS: u64 = 6 * 1024 * 1024 * 1024;

/// The env name the inspection's sealed-manifest check declares.
const PROBE_SECRET: &str = "CONFIG_TALARIA_PROBE";

/// The Python the image ships Hermes in.
const IMAGE_PYTHON: &str = "/opt/hermes/.venv/bin/python";

/// What the inspection runs inside the image. Prints one JSON object.
const INSPECT_SCRIPT: &str = r#"
import base64, json, os, sys
sys.path.insert(0, "/opt/hermes")
out = {}
try:
    from hermes_cli.config_defaults import DEFAULT_CONFIG
    out["configVersion"] = DEFAULT_CONFIG.get("_config_version")
except Exception as e:
    out["configVersionError"] = type(e).__name__
try:
    import hermes_cli
    out["imageVersion"] = getattr(hermes_cli, "__version__", None)
except Exception:
    pass
try:
    from tools.environments.local_env_policy import _HERMES_PROVIDER_ENV_BLOCKLIST as B, _ALWAYS_STRIP_KEYS as A
    out["strippedEnv"] = sorted(set(B) | set(A))
except Exception as e:
    out["strippedEnvError"] = type(e).__name__
out["s6"] = all(os.access(p, os.X_OK) for p in ("/command/s6-svc", "/command/s6-svwait"))
contract = {}
home = os.environ.get("HERMES_HOME", "")
try:
    d = os.path.join(home, "plugins", "platforms", "talaria-sealed")
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "plugin.yaml"), "w") as f:
        f.write(base64.b64decode(os.environ["TALARIA_PROBE_MANIFEST"]).decode())
    from tools.environments.local_env_policy import _home_adapter_secret_env
    contract["sealedHidden"] = os.environ["TALARIA_PROBE_SECRET"] in set(_home_adapter_secret_env())
except Exception as e:
    contract["sealedError"] = (type(e).__name__ + ": " + str(e))[:300]
want = int(os.environ.get("TALARIA_SCHEMA", "0"))
cv = out.get("configVersion")
if isinstance(cv, int) and cv > want and os.environ.get("TALARIA_PROBE_CONFIG"):
    try:
        written = json.loads(base64.b64decode(os.environ["TALARIA_PROBE_CONFIG"]))
        written["_config_version"] = want
        with open(os.path.join(home, "config.yaml"), "w") as f:
            json.dump(written, f)
        from hermes_cli.config import load_config, migrate_config
        import copy
        # What Hermes would run on Talaria's config stamped as current (its
        # migration skipped), against what it runs once it has migrated it.
        skipped = copy.deepcopy(load_config())
        migrate_config(interactive=False, quiet=True)
        migrated = copy.deepcopy(load_config())
        missing = object()
        def at(d, path):
            for k in path:
                if not isinstance(d, dict) or k not in d:
                    return missing
                d = d[k]
            return d
        changed = []
        def walk(path, w):
            if isinstance(w, dict) and w:
                for k, v in w.items():
                    if not path and k == "_config_version":
                        continue
                    walk(path + [k], v)
            elif at(skipped, path) != at(migrated, path):
                changed.append(".".join(str(p) for p in path) or "(the whole config)")
        walk([], written)
        contract["migratedTo"] = migrated.get("_config_version") if isinstance(migrated, dict) else None
        contract["migrationChanged"] = changed[:40]
    except Exception as e:
        contract["migrationError"] = (type(e).__name__ + ": " + str(e))[:300]
out["contract"] = contract
print(json.dumps(out))
"#;

/// Where a preparation stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// Pulled and inspected; the record is runnable.
    Ready,
    /// Still downloading: the pull's own words.
    Pulling(String),
    /// Giving way to another download.
    Waiting(String),
    /// The docker disk has no room for the download: held, and said.
    NoSpace(String),
    /// It failed. `definitive` is the image's fault (it cannot be run on
    /// this fleet); otherwise the registry or docker, tried again later.
    Failed { why: String, definitive: bool },
}

/// What the inspection read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspected {
    pub config_version: u64,
    pub image_version: Option<String>,
    pub stripped_env: Vec<String>,
    pub s6: bool,
    /// What changed in the Hermes internals Talaria relies on (none: all
    /// held), each a sentence for the admin.
    pub problems: Vec<String>,
    /// What the contract checks covered.
    pub checked: Vec<String>,
}

impl Inspected {
    pub fn to_json(&self) -> Value {
        json!({
            "imageVersion": self.image_version,
            "strippedEnv": self.stripped_env,
            "s6": self.s6,
            "contract": { "problems": self.problems, "checked": self.checked },
        })
    }
}

/// The contract checks' verdict, from the inspection's `contract` object and
/// the image's schema version against the one this release renders.
pub fn contract_problems(
    contract: &Value,
    image_schema: u64,
    ours: u64,
) -> (Vec<String>, Vec<String>) {
    let text = |k: &str| {
        contract
            .get(k)
            .and_then(Value::as_str)
            .map(|e| e.chars().take(300).collect::<String>())
    };
    let mut problems = Vec::new();
    let mut checked = Vec::new();
    match (contract.get("sealedHidden").and_then(Value::as_bool), text("sealedError")) {
        (Some(true), _) => checked.push("sealed CONFIG_* credentials stay out of commands".into()),
        (_, Some(e)) => problems.push(format!(
            "Talaria could not check that Hermes keeps sealed CONFIG_* credentials out of an agent's commands ({e})"
        )),
        _ => problems.push(
            "Hermes no longer keeps a platform plugin's declared secrets out of the commands an agent runs (tools/environments/local_env_policy.py _home_adapter_secret_env): every sealed CONFIG_* credential would reach them".into(),
        ),
    }
    if image_schema > ours {
        if let Some(e) = text("migrationError") {
            problems.push(format!(
                "Hermes' own config migration from schema {ours} to {image_schema} could not be run on Talaria's config ({e})"
            ));
        } else {
            let changed: Vec<String> = contract
                .get("migrationChanged")
                .and_then(Value::as_array)
                .map(|ks| {
                    ks.iter()
                        .filter_map(Value::as_str)
                        .map(|k| k.chars().take(120).collect())
                        .take(40)
                        .collect()
                })
                .unwrap_or_default();
            if contract.get("migrationChanged").is_none() {
                problems.push(format!(
                    "the image did not report its config migration from schema {ours} to {image_schema}"
                ));
            } else if changed.is_empty() {
                checked.push(format!(
                    "its migration from schema {ours} to {image_schema} leaves every key Talaria writes as it is"
                ));
            } else {
                problems.push(format!(
                    "Hermes' migration from schema {ours} to {image_schema} changes keys Talaria writes ({}): stamping the config {image_schema} would skip that migration",
                    changed.join(", ")
                ));
            }
        }
    }
    (problems, checked)
}

/// Read the inspection's output: the last line that is a JSON object.
pub fn parse_inspection(stdout: &str) -> Result<Inspected, String> {
    let obj = stdout
        .lines()
        .rev()
        .take(20)
        .find_map(|l| match serde_json::from_str::<Value>(l.trim()) {
            Ok(Value::Object(m)) => Some(m),
            _ => None,
        })
        .ok_or_else(|| "the image printed no inspection".to_string())?;
    let config_version = obj
        .get("configVersion")
        .and_then(Value::as_i64)
        .filter(|n| images::is_config_version(*n))
        .ok_or_else(|| {
            format!(
                "the image did not say its config schema version ({})",
                obj.get("configVersionError")
                    .and_then(Value::as_str)
                    .map(|e| e.chars().take(80).collect::<String>())
                    .unwrap_or_else(|| {
                        "no hermes_cli.config_defaults.DEFAULT_CONFIG[\"_config_version\"]".into()
                    })
            )
        })? as u64;
    let image_version = obj
        .get("imageVersion")
        .and_then(Value::as_str)
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 32
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        })
        .map(str::to_string);
    let stripped_env: Vec<String> = obj
        .get("strippedEnv")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .filter(|n| images::is_env_name(n))
                .take(4096)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let s6 = obj.get("s6").and_then(Value::as_bool).unwrap_or(false);
    let (problems, checked) = contract_problems(
        obj.get("contract").unwrap_or(&Value::Null),
        config_version,
        talaria_fleet_render::HERMES_CONFIG_VERSION,
    );
    Ok(Inspected {
        config_version,
        image_version,
        stripped_env,
        s6,
        problems,
        checked,
    })
}

/// Whether the image is on the host.
pub async fn image_present(image: &str) -> bool {
    docker(
        &["image", "inspect", "--format", "{{.Id}}", image],
        Duration::from_secs(30),
    )
    .await
    .is_ok()
}

/// What the inspection hands the image: the schema this release renders,
/// Talaria's config as the canary has it (no tool servers, no platforms but
/// the API server — no credential), and the sealed manifest declaring one
/// probe name. Names and placeholders only, never a secret.
fn probe_env(base: Option<&str>) -> Vec<String> {
    let config = serde_json::Value::Object(talaria_fleet_render::canary::canary_routed(
        base,
        talaria_fleet_render::HERMES_CONFIG_VERSION,
    ));
    let b64 = |s: &str| {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(s)
    };
    let config = b64(&config.to_string());
    let manifest = b64(&talaria_agent_secrets::sealed_manifest(&[
        PROBE_SECRET.to_string()
    ]));
    let mut env = vec![
        "HERMES_HOME=/tmp/talaria-probe-home".to_string(),
        format!(
            "TALARIA_SCHEMA={}",
            talaria_fleet_render::HERMES_CONFIG_VERSION
        ),
        format!("TALARIA_PROBE_SECRET={PROBE_SECRET}"),
        format!("TALARIA_PROBE_MANIFEST={manifest}"),
    ];
    // An argument longer than the kernel takes is left out: the schema check
    // then reports that it did not run, and the release is held.
    if config.len() < 100_000 {
        env.push(format!("TALARIA_PROBE_CONFIG={config}"));
    }
    env
}

/// Run the inspection.
pub async fn inspect(image: &str, base: Option<&str>) -> Result<Inspected, (String, bool)> {
    let env = probe_env(base);
    let mut args: Vec<&str> = vec![
        "run",
        "--rm",
        "--network",
        "none",
        "--memory",
        "1g",
        "--pids-limit",
        "256",
        "--cpus",
        "1",
        "--entrypoint",
        IMAGE_PYTHON,
        "-w",
        "/opt/hermes",
    ];
    for e in &env {
        args.push("-e");
        args.push(e);
    }
    args.extend([image, "-I", "-c", INSPECT_SCRIPT]);
    let ran = docker(&args, INSPECT_TIMEOUT).await;
    match ran {
        Ok((stdout, _)) => parse_inspection(&stdout).map_err(|e| (e, true)),
        Err(e) => {
            // The image's own Python failing (a traceback) is the image's
            // answer — unless it is the disk filling under it. Anything else
            // (docker slow, refusing, out of room) is the host's: tried again
            // later, never a ban.
            let definitive = e.contains("Traceback") && !e.to_lowercase().contains("no space left");
            Err((
                format!(
                    "the image could not be inspected: {}",
                    e.chars().take(400).collect::<String>()
                ),
                definitive,
            ))
        }
    }
}

/// Free bytes and size of the filesystem the docker root (and so every
/// volume, Postgres's among them) lives on, or None when it cannot be read.
async fn docker_disk() -> Option<(u64, u64, String)> {
    let (root, _) = docker(
        &["info", "--format", "{{.DockerRootDir}}"],
        Duration::from_secs(30),
    )
    .await
    .ok()?;
    let root = root.trim().to_string();
    let (free, total) = talaria_host_metrics::mount_space(std::path::Path::new(&root))
        .or_else(|| talaria_host_metrics::mount_space(std::path::Path::new("/")))?;
    Some((free, total, root))
}

/// The room a download needs: the image the fleet runs now, a quarter
/// again (or a guess when that cannot be read).
async fn room_needed(pg: &PgPool) -> u64 {
    let state = images::load_state(pg).await.ok();
    let current = state
        .and_then(|s| s.current)
        .unwrap_or_else(|| crate::compiled_pin().0);
    let image = match images::record(pg, &current).await.ok().flatten() {
        Some(r) => r.image_ref(),
        None => return IMAGE_SIZE_GUESS,
    };
    docker(
        &["image", "inspect", "--format", "{{.Size}}", &image],
        Duration::from_secs(30),
    )
    .await
    .ok()
    .and_then(|(out, _)| out.trim().parse::<u64>().ok())
    .map(|size| size + size / 4)
    .unwrap_or(IMAGE_SIZE_GUESS)
}

/// Whether a pull of `need` bytes fits beside the share kept free.
pub fn room_for(need: u64, free: u64, total: u64) -> bool {
    let keep = (total as f64 * KEEP_FREE_SHARE) as u64;
    free > need.saturating_add(keep)
}

fn gib(n: u64) -> String {
    format!("{:.1} GB", n as f64 / 1e9)
}

/// Bring a rollout's image to ready: on the host (pulled if it is not,
/// giving way to other pulls and never past the disk's room), then — once —
/// inspected, with what it said recorded.
pub async fn prepare(pg: &PgPool, record: &ImageRecord, base: Option<&str>) -> Prepared {
    let image = record.image_ref();
    let key = format!("hermes-image:{}", record.digest);
    let ours = talaria_fleet_docker::pull_running(&key);
    if ours || !image_present(&image).await {
        if !ours {
            // Someone else's download goes first.
            if talaria_fleet_docker::pulls_running() > 0 {
                return Prepared::Waiting(
                    "waits for another agent image download to finish first".into(),
                );
            }
            let need = room_needed(pg).await;
            match docker_disk().await {
                Some((free, total, root)) if !room_for(need, free, total) => {
                    return Prepared::NoSpace(format!(
                        "downloading {} needs about {} free on the docker disk ({root}) and a tenth of it kept free; it has {} of {}",
                        record.label(),
                        gib(need),
                        gib(free),
                        gib(total)
                    ));
                }
                Some((free, total, root)) => tracing::info!(
                    "[hermes] downloading {}: about {} needed on the docker disk ({root}), {} free of {}",
                    record.label(),
                    gib(need),
                    gib(free),
                    gib(total)
                ),
                None => tracing::warn!(
                    "[hermes] the docker disk's free space could not be read; downloading {} without the check",
                    record.label()
                ),
            }
            // This instance starts the download, so the image is its to
            // prune — recorded now: the pull outlives this step, and the step
            // that finds it finished sees only the image on the host.
            if let Err(e) = images::note_pulled(pg, &record.digest).await {
                tracing::warn!(
                    "[hermes] {}: the pull could not be recorded: {e}",
                    record.label()
                );
            }
        }
        let files = match talaria_fleet_render::canary::render_canary(
            &image,
            talaria_fleet_render::HERMES_CONFIG_VERSION,
            None,
        )
        .await
        {
            Ok(f) => f,
            Err(e) => {
                return Prepared::Failed {
                    why: format!("the canary could not be rendered for the download: {e}"),
                    definitive: false,
                };
            }
        };
        match talaria_fleet_docker::pull_project_image_within(
            &key,
            &files.compose_args(&[]),
            talaria_fleet_render::canary::CANARY_SERVICE,
            PULL_SLICE,
        )
        .await
        {
            PullStatus::Running(p) => return Prepared::Pulling(p.phase()),
            PullStatus::Failed(e) => {
                return Prepared::Failed {
                    why: e,
                    definitive: false,
                };
            }
            PullStatus::Done => {}
        }
    }
    // Inspected once: a digest's content never changes.
    if record.runnable() {
        return Prepared::Ready;
    }
    match inspect(&image, base).await {
        Ok(found) => {
            if !found.s6 {
                return Prepared::Failed {
                    why: "the image has no s6-svc and s6-svwait under /command — Talaria drains an agent's gateway with them before every roll and stop, so this image would be hard-killed on each".into(),
                    definitive: true,
                };
            }
            let own_version = found.image_version.as_ref().map(|v| format!("v{v}"));
            if let Err(e) = images::set_prepared(
                pg,
                &record.digest,
                found.config_version,
                &found.to_json(),
                false,
                own_version.as_deref(),
            )
            .await
            {
                return Prepared::Failed {
                    why: format!("the inspection could not be recorded: {e}"),
                    definitive: false,
                };
            }
            tracing::info!(
                "[hermes] {} inspected: config version {}, {} stripped env names, contract {}",
                record.label(),
                found.config_version,
                found.stripped_env.len(),
                if found.problems.is_empty() {
                    "intact".to_string()
                } else {
                    format!("problems: {}", found.problems.join("; "))
                }
            );
            Prepared::Ready
        }
        Err((why, definitive)) => Prepared::Failed { why, definitive },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inspection_is_read_as_one_checked_object() {
        // The v0.22.4 image's own answer (2026-11-03), trimmed.
        let out = "noise\n{\"configVersion\": 51, \"imageVersion\": \"0.22.4\", \"strippedEnv\": [\"OPENAI_API_KEY\", \"bad name\", \"TELEGRAM_BOT_TOKEN\"], \"s6\": true}\n";
        let got = parse_inspection(out).unwrap();
        assert_eq!(got.config_version, 51);
        assert_eq!(got.image_version.as_deref(), Some("0.22.4"));
        assert_eq!(
            got.stripped_env,
            vec!["OPENAI_API_KEY", "TELEGRAM_BOT_TOKEN"]
        );
        assert!(got.s6);
    }

    #[test]
    fn an_image_that_will_not_say_its_config_version_is_not_runnable() {
        for out in [
            "",
            "not json",
            "{\"configVersionError\": \"ModuleNotFoundError\", \"s6\": true}",
            "{\"configVersion\": \"51\"}",
            "{\"configVersion\": 0}",
            "{\"configVersion\": 999999}",
            "[51]",
        ] {
            assert!(parse_inspection(out).is_err(), "{out}");
        }
        let e = parse_inspection("{\"configVersionError\": \"ModuleNotFoundError\"}").unwrap_err();
        assert!(e.contains("ModuleNotFoundError"), "{e}");
    }

    #[test]
    fn the_contract_holds_a_release_whose_migration_moves_talarias_keys() {
        let ok = json!({ "sealedHidden": true, "migrationChanged": [] });
        let (problems, checked) = contract_problems(&ok, 52, 51);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(checked.len(), 2);
        // Same schema: nothing to migrate, nothing to check.
        let (problems, checked) = contract_problems(&json!({ "sealedHidden": true }), 51, 51);
        assert!(problems.is_empty() && checked.len() == 1);
        let moved = json!({ "sealedHidden": true, "migrationChanged": ["agent.disabled_toolsets (removed)", "terminal.env_passthrough"] });
        let (problems, _) = contract_problems(&moved, 53, 51);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("agent.disabled_toolsets (removed), terminal.env_passthrough"),
            "{problems:?}"
        );
        let failed = json!({ "sealedHidden": true, "migrationError": "KeyError: 'x'" });
        assert!(contract_problems(&failed, 53, 51).0[0].contains("could not be run"));
        let unreported = json!({ "sealedHidden": true });
        assert!(contract_problems(&unreported, 53, 51).0[0].contains("did not report"));
    }

    #[test]
    fn the_contract_holds_a_release_that_lets_sealed_secrets_reach_commands() {
        for c in [
            json!({ "sealedHidden": false }),
            json!({}),
            json!({ "sealedError": "ImportError: x" }),
        ] {
            let (problems, _) = contract_problems(&c, 51, 51);
            assert_eq!(problems.len(), 1, "{c}");
            assert!(problems[0].contains("CONFIG_*"), "{problems:?}");
        }
    }

    #[test]
    fn a_pull_keeps_a_tenth_of_the_disk_free() {
        let gb = 1_000_000_000u64;
        assert!(room_for(6 * gb, 20 * gb, 100 * gb));
        assert!(!room_for(6 * gb, 15 * gb, 100 * gb), "6 + 10 > 15");
        assert!(!room_for(6 * gb, 5 * gb, 100 * gb));
    }

    #[test]
    fn an_image_version_it_reports_is_kept_only_when_it_reads_as_one() {
        let got = parse_inspection("{\"configVersion\": 52, \"imageVersion\": \"0.22.6\\n$(x)\"}")
            .unwrap();
        assert_eq!(got.image_version, None);
        assert!(!got.s6, "an absent s6 answer is a no");
    }
}
