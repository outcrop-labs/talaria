// THE HEALTH QUESTIONS — asked of the canary before any agent runs a new
// image, and of each agent the rollout moves onto it. None costs a token:
// docker's own health, the API server's `/health` and `/v1/models` (which
// Hermes answers from its own config), the container's logs, and the s6
// tools every roll's drain relies on. Each is asked the way the gateway
// asks — over the dial the persona door uses — so "the gateway can reach
// it" is the probe itself.

use std::time::Duration;

use talaria_fleet_docker::docker;
use talaria_fleet_render::canary::{CanaryFiles, render_canary};
use talaria_hermes_images::ImageRecord;

/// How long the canary may take to report healthy. The chassis gives a boot
/// 60 s before its health check counts and five checks after; a loaded host
/// boots slower than that, so `unhealthy` before this is still booting.
const CANARY_HEALTH_WAIT: Duration = Duration::from_secs(8 * 60);

/// Its `compose up`'s bound: the image is already on the host.
const CANARY_UP: Duration = Duration::from_secs(5 * 60);

/// The log lines that say a Hermes boot went wrong with its config: the
/// migration Talaria's version stamp exists to prevent (it fails on the
/// read-only bind), and a config Hermes refused. Lowercase; matched on a
/// lowercased log.
const LOG_FAULTS: &[&str] = &[
    "read-only file system",
    "config migration failed",
    "migration failed",
    "failed to migrate",
    "could not migrate",
    "error loading config",
    "failed to load config",
    "could not load config",
    "invalid config",
    "configerror",
    "config validation",
];

/// The first log line that names a config or migration fault, or None.
pub fn log_fault(logs: &str) -> Option<String> {
    logs.lines().find_map(|line| {
        let lower = line.to_lowercase();
        LOG_FAULTS
            .iter()
            .any(|f| lower.contains(f))
            .then(|| line.trim().chars().take(300).collect())
    })
}

/// A container's logs (stdout and stderr), its last `tail` lines.
pub async fn logs(container: &str, tail: u32) -> String {
    match docker(
        &["logs", "--tail", &tail.to_string(), container],
        Duration::from_secs(30),
    )
    .await
    {
        Ok((out, err)) => format!("{out}\n{err}"),
        Err(e) => format!("(logs unavailable: {e})"),
    }
}

/// Whether the API server answers, the way the gateway reaches it: its
/// `/health`, then — with the agent's bearer — `/v1/models`, which Hermes
/// answers from its own config without calling a model.
pub async fn api_answers(base_url: &str, bearer: Option<&str>) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("probe client would not build: {e}"))?;
    let base = base_url.trim_end_matches('/');
    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .map_err(|e| format!("the gateway could not reach its API server at {base}: {e}"))?;
    if !health.status().is_success() {
        return Err(format!(
            "its API server answered {} to /health",
            health.status()
        ));
    }
    if let Some(key) = bearer {
        let models = client
            .get(format!("{base}/v1/models"))
            .bearer_auth(key)
            .send()
            .await
            .map_err(|e| format!("its API server did not answer /v1/models: {e}"))?;
        if !models.status().is_success() {
            return Err(format!(
                "its API server answered {} to /v1/models",
                models.status()
            ));
        }
    }
    Ok(())
}

/// Whether the container runs `image`: the image it was created from is the
/// one the reference resolves to on this host.
pub async fn runs_image(container: &str, image: &str) -> Result<bool, String> {
    let (ran, _) = docker(
        &["inspect", "--format", "{{.Image}}", container],
        Duration::from_secs(30),
    )
    .await?;
    let (want, _) = docker(
        &["image", "inspect", "--format", "{{.Id}}", image],
        Duration::from_secs(30),
    )
    .await?;
    Ok(ran.trim() == want.trim())
}

/// Whether the drain's tools are where every roll and stop calls them, in a
/// RUNNING container: the gateway's s6 service and `s6-svc`/`s6-svwait`.
pub async fn drain_ready(container: &str) -> Result<(), String> {
    docker(
        &[
            "exec",
            container,
            "sh",
            "-c",
            "test -d /run/service/gateway-default && test -x /command/s6-svc && test -x /command/s6-svwait",
        ],
        Duration::from_secs(30),
    )
    .await
    .map(|_| ())
    .map_err(|_| {
        "its gateway is not the s6 service Talaria drains (/run/service/gateway-default with /command/s6-svc and s6-svwait) — every roll and stop would hard-kill it".to_string()
    })
}

/// Where docker's health check stands for a container.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Boot {
    Healthy,
    Booting,
    Exited(String),
}

async fn boot_state(container: &str) -> Boot {
    match docker(
        &[
            "inspect",
            "--format",
            "{{.State.Status}}|{{if .State.Health}}{{.State.Health.Status}}{{end}}|{{.State.ExitCode}}",
            container,
        ],
        Duration::from_secs(15),
    )
    .await
    {
        Ok((out, _)) => {
            let mut parts = out.trim().split('|');
            let (status, health, code) = (
                parts.next().unwrap_or(""),
                parts.next().unwrap_or(""),
                parts.next().unwrap_or(""),
            );
            match (status, health) {
                ("running", "healthy") => Boot::Healthy,
                ("exited" | "dead", _) => Boot::Exited(format!("exited (code {code})")),
                _ => Boot::Booting,
            }
        }
        Err(_) => Boot::Booting,
    }
}

/// The canary's URL as the gateway dials an agent: by container name on the
/// fleet network, or its published loopback port.
async fn canary_url(files: &CanaryFiles) -> Result<String, String> {
    if talaria_fleet_layout::dial_is_container() {
        return Ok(format!("http://{}:8642", files.container));
    }
    let (out, _) = docker(
        &["port", &files.container, "8642/tcp"],
        Duration::from_secs(15),
    )
    .await?;
    let addr = out
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("127.0.0.1:"))
        .ok_or_else(|| format!("the canary published no loopback port ({})", out.trim()))?;
    Ok(format!("http://{addr}"))
}

/// Take the canary down, its volume with it.
pub async fn canary_down(files: &CanaryFiles) {
    let args = files.compose_args(&["down", "-v", "--remove-orphans", "-t", "10"]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Err(e) = docker(&refs, Duration::from_secs(180)).await {
        tracing::warn!("[hermes] the canary did not come down cleanly: {e}");
    }
}

/// Boot `record`'s image as the canary, with `base` (an agent's rendered
/// config.yaml) shaping its config, and ask it every health question. The
/// canary is taken down whatever the answer. `Ok` carries what passed.
pub async fn run_canary(record: &ImageRecord, base: Option<&str>) -> Result<String, String> {
    let config_version = record
        .config_version
        .ok_or_else(|| "the image's config version is not known".to_string())?;
    let image = record.image_ref();
    let files = render_canary(&image, config_version, base).await?;
    // A canary an interrupted step left behind goes first.
    canary_down(&files).await;
    let verdict = canary_checks(&files, &image).await;
    if verdict.is_err() {
        let tail = logs(&files.container, 40).await;
        tracing::warn!("[hermes] canary of {image} failed; its last log lines:\n{tail}");
    }
    canary_down(&files).await;
    verdict
}

async fn canary_checks(files: &CanaryFiles, image: &str) -> Result<String, String> {
    let up = files.compose_args(&["up", "-d", "--no-build", "--pull", "never"]);
    let refs: Vec<&str> = up.iter().map(String::as_str).collect();
    docker(&refs, CANARY_UP).await.map_err(|e| {
        format!(
            "the canary would not start: {}",
            e.chars().take(400).collect::<String>()
        )
    })?;
    let started = tokio::time::Instant::now();
    loop {
        match boot_state(&files.container).await {
            Boot::Healthy => break,
            Boot::Exited(why) => {
                let log = logs(&files.container, 400).await;
                // The config fault if the log names one, else its last word.
                let said = log_fault(&log).or_else(|| {
                    log.lines()
                        .map(str::trim)
                        .rfind(|l| !l.is_empty())
                        .map(|l| l.chars().take(200).collect())
                });
                return Err(format!(
                    "the canary {why} before it reported healthy{}",
                    said.map(|f| format!(" — its log: {f}")).unwrap_or_default()
                ));
            }
            Boot::Booting if started.elapsed() > CANARY_HEALTH_WAIT => {
                return Err(format!(
                    "the canary did not report healthy within {} min",
                    CANARY_HEALTH_WAIT.as_secs() / 60
                ));
            }
            Boot::Booting => tokio::time::sleep(Duration::from_secs(3)).await,
        }
    }
    let booted = started.elapsed().as_secs();
    let url = canary_url(files).await?;
    api_answers(&url, Some(&files.api_key))
        .await
        .map_err(|e| format!("the canary is healthy but {e}"))?;
    if !runs_image(&files.container, image).await.unwrap_or(false) {
        return Err(format!("the canary does not run {image}"));
    }
    drain_ready(&files.container)
        .await
        .map_err(|e| format!("the canary booted, but {e}"))?;
    if let Some(fault) = log_fault(&logs(&files.container, 2000).await) {
        return Err(format!("the canary booted, but its log says: {fault}"));
    }
    Ok(format!(
        "the canary reported healthy in {booted}s; its API server answered the gateway; its log names no config or migration fault; its drain tools are in place"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_or_migration_fault_in_the_log_is_named_and_a_clean_boot_is_not() {
        let clean = "[2026-11-03 01:02:03] gateway starting\nAPI server listening on 0.0.0.0:8642\nplugins loaded: talaria-events\n";
        assert_eq!(log_fault(clean), None);
        let migrating = "boot\nconfig migration failed: [Errno 30] Read-only file system: '/opt/data/config.yaml'\n";
        assert_eq!(
            log_fault(migrating).as_deref(),
            Some(
                "config migration failed: [Errno 30] Read-only file system: '/opt/data/config.yaml'"
            )
        );
        assert!(log_fault("hermes_cli.config.ConfigError: unknown key").is_some());
        assert!(log_fault("PermissionError: [Errno 30] Read-only file system").is_some());
    }
}
