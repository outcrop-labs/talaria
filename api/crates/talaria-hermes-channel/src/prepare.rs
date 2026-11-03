// THE PREPARATION — a candidate image pulled and read before anything runs it.
//
// The pull is the agents' own shared pull (talaria-fleet-docker's pull.rs:
// one per key in the process, on a task of its own, killed only for silence
// or its ceiling), addressed through the canary's compose project so it
// never touches the fleet's. It gives way: while any other pull runs in the
// process (a hire's, a roll's), the preparation waits for the next tick
// rather than start a second 1 GB download beside it. Docker has no pull
// priority to set; waiting is the low priority.
//
// The inspection reads the image with networking off — `docker run --rm
// --network none` of the image's own Python — for what the render needs from
// it: `DEFAULT_CONFIG["_config_version"]`, the env names its Hermes strips
// from an agent's commands, its own version, and whether the s6 tools the
// drain relies on are where Talaria calls them. Its output is untrusted:
// parsed as one JSON object, every field checked and bounded.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
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

/// How many times an inspection that failed for docker's own reasons (a
/// timeout, an unreachable daemon) is tried before the image is given up on.
const INSPECT_TRIES: u32 = 3;

/// The Python the image ships Hermes in.
const IMAGE_PYTHON: &str = "/opt/hermes/.venv/bin/python";

/// What the inspection runs inside the image. Prints one JSON object.
const INSPECT_SCRIPT: &str = r#"
import json, os, sys
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
}

impl Inspected {
    pub fn to_json(&self) -> Value {
        json!({
            "imageVersion": self.image_version,
            "strippedEnv": self.stripped_env,
            "s6": self.s6,
        })
    }
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
    Ok(Inspected {
        config_version,
        image_version,
        stripped_env,
        s6,
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

/// Whether each digest was on the host before this process first asked to
/// prepare it (so this instance did not pull it, and must not prune it).
static PRESENT_BEFORE: LazyLock<Mutex<HashMap<String, bool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Inspections that failed for docker's own reasons, per digest.
static INSPECT_FAILURES: LazyLock<Mutex<HashMap<String, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Run the inspection.
pub async fn inspect(image: &str) -> Result<Inspected, (String, bool)> {
    let ran = docker(
        &[
            "run",
            "--rm",
            "--network",
            "none",
            "--entrypoint",
            IMAGE_PYTHON,
            "-w",
            "/opt/hermes",
            image,
            "-I",
            "-c",
            INSPECT_SCRIPT,
        ],
        INSPECT_TIMEOUT,
    )
    .await;
    match ran {
        Ok((stdout, _)) => parse_inspection(&stdout).map_err(|e| (e, true)),
        Err(e) => {
            // The image's Python failing (a traceback, no such interpreter)
            // is the image's answer; anything else is docker's trouble.
            let definitive = e.contains("Traceback")
                || e.contains("no such file or directory")
                || e.contains("executable file not found");
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

/// Bring a candidate to runnable: pull it (giving way to other pulls), then
/// inspect it and record what it said.
pub async fn prepare(pg: &PgPool, record: &ImageRecord) -> Prepared {
    if record.runnable() {
        return Prepared::Ready;
    }
    let image = record.image_ref();
    let key = format!("hermes-image:{}", record.digest);
    let known_presence = PRESENT_BEFORE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&record.digest)
        .copied();
    let present = image_present(&image).await;
    let present_before = match known_presence {
        Some(p) => p,
        None => {
            PRESENT_BEFORE
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(record.digest.clone(), present);
            present
        }
    };
    if !present {
        // Our own pull in flight is joined; someone else's is given way to.
        if known_presence.is_none() && talaria_fleet_docker::pulls_running() > 0 {
            PRESENT_BEFORE
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&record.digest);
            return Prepared::Waiting(
                "waits for another agent image download to finish first".into(),
            );
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
    match inspect(&image).await {
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
                !present_before,
                own_version.as_deref(),
            )
            .await
            {
                return Prepared::Failed {
                    why: format!("the inspection could not be recorded: {e}"),
                    definitive: false,
                };
            }
            talaria_agent_secrets::adopt_image_stripped(found.stripped_env.iter().cloned());
            INSPECT_FAILURES
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&record.digest);
            tracing::info!(
                "[hermes] {} inspected: config version {}, {} stripped env names",
                record.label(),
                found.config_version,
                found.stripped_env.len()
            );
            Prepared::Ready
        }
        Err((why, true)) => Prepared::Failed {
            why,
            definitive: true,
        },
        Err((why, false)) => {
            let tries = {
                let mut failures = INSPECT_FAILURES.lock().unwrap_or_else(|p| p.into_inner());
                let n = failures.entry(record.digest.clone()).or_insert(0);
                *n += 1;
                *n
            };
            Prepared::Failed {
                why: if tries >= INSPECT_TRIES {
                    format!("{why} ({tries} tries)")
                } else {
                    why
                },
                definitive: tries >= INSPECT_TRIES,
            }
        }
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
    fn an_image_version_it_reports_is_kept_only_when_it_reads_as_one() {
        let got = parse_inspection("{\"configVersion\": 52, \"imageVersion\": \"0.22.6\\n$(x)\"}")
            .unwrap();
        assert_eq!(got.image_version, None);
        assert!(!got.s6, "an absent s6 answer is a no");
    }
}
