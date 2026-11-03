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
if isinstance(cv, int) and cv > want:
    try:
        with open("/probe/configs.json") as f:
            agents = json.load(f)
        from hermes_cli.config import load_config, migrate_config
        import copy
        missing = object()
        def at(d, path):
            for k in path:
                if not isinstance(d, dict) or k not in d:
                    return missing
                d = d[k]
            return d
        changed = []
        for agent in agents:
            name, written = agent["name"], agent["config"]
            written["_config_version"] = want
            path = os.path.join(home, "config.yaml")
            for stale in ("config.yaml", ".env"):
                try:
                    os.remove(os.path.join(home, stale))
                except FileNotFoundError:
                    pass
            with open(path, "w") as f:
                json.dump(written, f)
            # What Hermes would run on Talaria's config stamped as current (its
            # migration skipped), against what it runs once it has migrated it.
            skipped = copy.deepcopy(load_config())
            migrate_config(interactive=False, quiet=True)
            migrated = copy.deepcopy(load_config())
            def walk(keys, w):
                if isinstance(w, dict) and w:
                    for k, v in w.items():
                        if not keys and k == "_config_version":
                            continue
                        walk(keys + [k], v)
                elif at(skipped, keys) != at(migrated, keys):
                    gone = " (removed)" if at(migrated, keys) is missing else ""
                    changed.append(name + ": " + (".".join(str(p) for p in keys) or "(the whole config)") + gone)
            walk([], written)
            contract["migratedTo"] = migrated.get("_config_version") if isinstance(migrated, dict) else None
        contract["migrationChecked"] = len(agents)
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
                let n = contract
                    .get("migrationChecked")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                checked.push(format!(
                    "its migration from schema {ours} to {image_schema} leaves every key Talaria writes as it is, in the rendered config of {n} agent{}",
                    if n == 1 { "" } else { "s" }
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

/// The probe's stand-in for a literal credential in a rendered config.
const REDACTED: &str = "${TALARIA_PROBE_REDACTED}";

/// Whether a config key names a credential.
fn credential_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "key",
        "token",
        "secret",
        "password",
        "passwd",
        "credential",
        "auth",
        "cookie",
        "bearer",
        "private",
        "session",
    ]
    .iter()
    .any(|w| k.contains(w))
}

/// Whether a string is only `${NAME}` handles (what Talaria writes for every
/// sealed or gateway credential): nothing secret in it.
fn only_handles(v: &str) -> bool {
    let mut rest = v.trim();
    if rest.is_empty() {
        return true;
    }
    while let Some(i) = rest.find("${") {
        if !rest[..i].trim().is_empty() && !rest[..i].trim().eq_ignore_ascii_case("bearer") {
            return false;
        }
        let Some(j) = rest[i..].find('}') else {
            return false;
        };
        rest = &rest[i + j + 1..];
    }
    rest.trim().is_empty()
}

/// An agent's config with every literal credential replaced by a stand-in:
/// any string under a credential-shaped key that is not just `${…}`
/// handles, and any `Bearer …` value. The shape — every key Talaria writes —
/// is kept, which is what the schema check compares.
pub fn redact(v: &Value) -> Value {
    fn walk(v: &Value, under_secret: bool) -> Value {
        match v {
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| (k.clone(), walk(v, under_secret || credential_key(k))))
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.iter().map(|x| walk(x, under_secret)).collect()),
            Value::String(s)
                if (under_secret && !only_handles(s))
                    || (s.to_ascii_lowercase().starts_with("bearer ") && !only_handles(s)) =>
            {
                Value::String(REDACTED.into())
            }
            other => other.clone(),
        }
    }
    walk(v, false)
}

/// The configs the schema check runs Hermes' migration on: every agent's
/// rendered config as it is on disk, redacted, each once — or, with no
/// agent yet, the default config the canary would run.
pub fn probe_configs(configs: &[(String, String)]) -> Value {
    let mut out: Vec<Value> = Vec::new();
    let mut seen: Vec<Value> = Vec::new();
    for (name, text) in configs {
        let Some(config) = talaria_fleet_render::canary::rendered_config(text) else {
            continue;
        };
        let config = redact(&Value::Object(config));
        if seen.contains(&config) {
            continue;
        }
        seen.push(config.clone());
        out.push(json!({ "name": name, "config": config }));
    }
    if out.is_empty() {
        out.push(json!({
            "name": "the default config",
            "config": Value::Object(talaria_fleet_render::canary::canary_routed(
                None,
                talaria_fleet_render::HERMES_CONFIG_VERSION,
            )),
        }));
    }
    Value::Array(out)
}

/// What the inspection hands the image in its environment: the schema this
/// release renders and the sealed manifest declaring one probe name. Names
/// only, never a secret; the configs go in a read-only file.
fn probe_env() -> Vec<String> {
    let b64 = |s: &str| {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(s)
    };
    let manifest = b64(&talaria_agent_secrets::sealed_manifest(&[
        PROBE_SECRET.to_string()
    ]));
    vec![
        "HERMES_HOME=/tmp/talaria-probe-home".to_string(),
        format!(
            "TALARIA_SCHEMA={}",
            talaria_fleet_render::HERMES_CONFIG_VERSION
        ),
        format!("TALARIA_PROBE_SECRET={PROBE_SECRET}"),
        format!("TALARIA_PROBE_MANIFEST={manifest}"),
    ]
}

/// Run the inspection: a named container (so one a timeout left behind is
/// removed), no network, bounded memory, processes and CPU, a read-only root
/// with a small tmpfs, every capability dropped, no privilege gain, as
/// nobody — and the configs it checks mounted read-only.
pub async fn inspect(
    image: &str,
    digest: &str,
    configs: &[(String, String)],
) -> Result<Inspected, (String, bool)> {
    let host_err = |e: String| (e, false);
    let name = format!(
        "talaria-inspect-{}",
        digest
            .trim_start_matches("sha256:")
            .chars()
            .take(12)
            .collect::<String>()
    );
    let dir = talaria_fleet_render::canary::canary_dir();
    let file = dir.join(format!("{name}.json"));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| host_err(format!("{}: {e}", dir.display())))?;
    tokio::fs::write(&file, probe_configs(configs).to_string())
        .await
        .map_err(|e| host_err(format!("{}: {e}", file.display())))?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = tokio::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).await;
    }
    let _ = docker(&["rm", "-f", &name], Duration::from_secs(60)).await;
    let mount = format!("{}:/probe/configs.json:ro", file.display());
    let env = probe_env();
    let mut args: Vec<&str> = vec![
        "run",
        "--rm",
        "--name",
        &name,
        "--network",
        "none",
        "--memory",
        "1g",
        "--pids-limit",
        "256",
        "--cpus",
        "1",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,size=256m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "65534:65534",
        "-v",
        &mount,
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
    // A timeout kills the CLI, not the container: it goes too.
    let _ = docker(&["rm", "-f", &name], Duration::from_secs(60)).await;
    let _ = tokio::fs::remove_file(&file).await;
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

/// How much room a download must leave: an upgrade keeps a tenth of the
/// disk free beside the image plus a quarter; a rollback — leaving something
/// worse — needs only the image itself and a fiftieth. `forced` is an
/// admin's Roll now or Pin: on a host whose disk cannot be read, it is the
/// consent to download anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Room {
    pub urgent: bool,
    pub forced: bool,
}

/// Free bytes and size of the filesystem the docker root (and so every
/// volume, Postgres's among them) lives on, or None when it cannot be read:
/// through the host root when the api has one (`/host`), the docker root
/// itself on a host-run api, the container's own `/` when the api runs in a
/// container on the same daemon (an overlay reports its upper directory's
/// filesystem). A daemon elsewhere (`DOCKER_HOST` over tcp/ssh) cannot be
/// read from here.
async fn docker_disk() -> Option<(u64, u64, String)> {
    if std::env::var("DOCKER_HOST").is_ok_and(|h| !h.trim().is_empty() && !h.starts_with("unix://"))
    {
        return None;
    }
    let (root, _) = docker(
        &["info", "--format", "{{.DockerRootDir}}"],
        Duration::from_secs(30),
    )
    .await
    .ok()?;
    let root = root.trim().to_string();
    let path = std::path::Path::new(&root);
    if let Some((free, total)) = talaria_host_metrics::mount_space(path) {
        return Some((free, total, root));
    }
    if path.exists() {
        return talaria_host_metrics::local_space(path).map(|(f, t)| (f, t, root));
    }
    if std::path::Path::new("/.dockerenv").exists() {
        return talaria_host_metrics::local_space(std::path::Path::new("/")).map(|(f, t)| {
            (
                f,
                t,
                format!("{root}, read through this container's own root"),
            )
        });
    }
    None
}

/// The image's own size: the one the fleet runs now (or a guess when that
/// cannot be read).
async fn image_size(pg: &PgPool) -> u64 {
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
    .unwrap_or(IMAGE_SIZE_GUESS)
}

/// The room a download needs, and what is kept free beside it.
pub fn room_rule(size: u64, total: u64, room: Room) -> (u64, u64) {
    if room.urgent {
        (size, total / 50)
    } else {
        (size + size / 4, (total as f64 * KEEP_FREE_SHARE) as u64)
    }
}

/// Whether a pull of `need` bytes fits beside `keep` kept free.
pub fn room_for(need: u64, keep: u64, free: u64) -> bool {
    free > need.saturating_add(keep)
}

fn gib(n: u64) -> String {
    format!("{:.1} GB", n as f64 / 1e9)
}

/// Bring a rollout's image to ready: on the host (pulled if it is not,
/// giving way to other pulls and never past the disk's room), then — once —
/// inspected, with what it said recorded.
pub async fn prepare(
    pg: &PgPool,
    record: &ImageRecord,
    configs: &[(String, String)],
    room: Room,
) -> Prepared {
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
            let size = image_size(pg).await;
            match docker_disk().await {
                Some((free, total, root)) => {
                    let (need, keep) = room_rule(size, total, room);
                    if !room_for(need, keep, free) {
                        return Prepared::NoSpace(format!(
                            "downloading {} needs about {} free on the docker disk ({root}) and {} kept free; it has {} of {}",
                            record.label(),
                            gib(need),
                            gib(keep),
                            gib(free),
                            gib(total)
                        ));
                    }
                    tracing::info!(
                        "[hermes] downloading {}: about {} needed on the docker disk ({root}), {} kept free, {} free of {}",
                        record.label(),
                        gib(need),
                        gib(keep),
                        gib(free),
                        gib(total)
                    );
                }
                // A disk this api cannot read is not one it may fill: held,
                // unless an admin pressed Roll now (or Pin) for this rollout.
                None if !room.forced => {
                    return Prepared::NoSpace(format!(
                        "the docker disk's free space cannot be read from this api (no /host mount, or a remote daemon), so {} is not downloaded on its own — Roll now downloads it anyway",
                        record.label()
                    ));
                }
                None => tracing::warn!(
                    "[hermes] the docker disk's free space could not be read; downloading {} on Roll now",
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
    match inspect(&image, &record.digest, configs).await {
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
    fn a_pull_keeps_a_tenth_of_the_disk_free_and_a_rollback_needs_less() {
        let gb = 1_000_000_000u64;
        let upgrade = Room::default();
        let (need, keep) = room_rule(4 * gb, 100 * gb, upgrade);
        assert_eq!((need, keep), (5 * gb, 10 * gb));
        assert!(room_for(need, keep, 20 * gb));
        assert!(!room_for(need, keep, 14 * gb), "5 + 10 > 14");
        // A nearly full host can still roll back: the image and a fiftieth.
        let rollback = Room {
            urgent: true,
            forced: true,
        };
        let (need, keep) = room_rule(4 * gb, 100 * gb, rollback);
        assert_eq!((need, keep), (4 * gb, 2 * gb));
        assert!(room_for(need, keep, 7 * gb));
        assert!(!room_for(need, keep, 5 * gb));
    }

    #[test]
    fn a_literal_credential_never_reaches_the_inspection_and_the_shape_does() {
        let config = json!({
            "model": { "api_key": "${LLM_API_KEY}", "base_url": "${LLM_BASE_URL}" },
            "mcp_servers": {
                "talaria": { "url": "http://x/mcp", "headers": { "X-Api-Key": "${TALARIA_AGENT_KEY}" } },
                "vendor": { "headers": { "Authorization": "Bearer sk-live-123" }, "env": { "VENDOR_TOKEN": "tok-abc" } }
            },
            "platforms": { "slack": { "bot_token": "xoxb-1", "channel": "general" } },
            "platform_toolsets": { "slack": ["web"] }
        });
        let r = redact(&config);
        assert_eq!(r["model"]["api_key"], "${LLM_API_KEY}", "a handle stays");
        assert_eq!(
            r["mcp_servers"]["talaria"]["headers"]["X-Api-Key"],
            "${TALARIA_AGENT_KEY}"
        );
        assert_eq!(
            r["mcp_servers"]["vendor"]["headers"]["Authorization"],
            REDACTED
        );
        assert_eq!(r["mcp_servers"]["vendor"]["env"]["VENDOR_TOKEN"], REDACTED);
        assert_eq!(r["platforms"]["slack"]["bot_token"], REDACTED);
        assert_eq!(
            r["platforms"]["slack"]["channel"], "general",
            "not a credential"
        );
        assert_eq!(r["platform_toolsets"], config["platform_toolsets"]);
        assert_eq!(r["mcp_servers"]["talaria"]["url"], "http://x/mcp");
        assert!(only_handles("Bearer ${X}") && !only_handles("${X}tail") && only_handles(""));
    }

    #[test]
    fn every_agents_config_is_checked_once_and_the_default_when_there_are_none() {
        let a =
            "model:\n  model: a\nmcp_servers:\n  talaria:\n    url: http://x\n...\n".to_string();
        let b = "model:\n  model: b\n".to_string();
        let set = probe_configs(&[
            ("ada".into(), a.clone()),
            ("bo".into(), b),
            ("cy".into(), a),
        ]);
        let names: Vec<&str> = set
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["ada", "bo"],
            "an identical config is checked once"
        );
        assert_eq!(
            set[0]["config"]["mcp_servers"]["talaria"]["url"],
            "http://x"
        );
        let none = probe_configs(&[]);
        assert_eq!(none[0]["name"], "the default config");
    }

    #[test]
    fn an_image_version_it_reports_is_kept_only_when_it_reads_as_one() {
        let got = parse_inspection("{\"configVersion\": 52, \"imageVersion\": \"0.22.6\\n$(x)\"}")
            .unwrap();
        assert_eq!(got.image_version, None);
        assert!(!got.s6, "an absent s6 answer is a no");
    }
}
