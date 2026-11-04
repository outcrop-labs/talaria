// THE HEALTH QUESTIONS — asked of the canary before any agent runs a new
// image, and of each agent the rollout moves onto it, again and again while
// it soaks. None costs a token: docker's own health and restart count, the
// API server's `/health` and `/v1/models` (which Hermes answers from its own
// config), the container's logs, and the s6 tools every roll's drain relies
// on. Each is asked the way the gateway asks — over the dial the persona door
// uses — so "the gateway can reach it" is the probe itself.

use std::time::Duration;

use talaria_fleet_docker::docker;
use talaria_fleet_render::canary::{CanaryFiles, render_canary};
use talaria_hermes_images::ImageRecord;

use crate::ops::{Fault, Snapshot};

/// How long the canary may take to report healthy. The chassis gives a boot
/// 60 s before its health check counts and five checks after; a loaded host
/// boots slower than that, so `unhealthy` before this is still booting.
const CANARY_HEALTH_WAIT: Duration = Duration::from_secs(8 * 60);

/// Its `compose up`'s bound: the image is already on the host, but docker
/// has been measured taking five minutes to create a container on a
/// saturated disk.
const CANARY_UP: Duration = Duration::from_secs(10 * 60);

/// How often a soaking canary is asked again.
const CANARY_SOAK_EVERY: Duration = Duration::from_secs(20);

/// How long the canary soaks once healthy: it stays up and is asked again
/// and again, so an image that dies a minute or two after boot (on its first
/// cron tick, a platform thread, a refresh) fails here rather than on the
/// fleet. `TALARIA_HERMES_CANARY_SOAK_SECS` sets it.
pub fn canary_soak() -> Duration {
    Duration::from_secs(crate::env_secs("TALARIA_HERMES_CANARY_SOAK_SECS", 5 * 60))
}

/// The log lines that say a Hermes boot went wrong with its config: the
/// migration Talaria's version stamp exists to prevent (it fails on the
/// read-only bind), and a config Hermes refused. Lowercase; matched on a
/// lowercased log. An agent's are counted only when new against the image it
/// left ([`log_faults`] normalizes them for that comparison).
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

/// A fault line as the soak compares it: lowercased, without its digits
/// (timestamps, pids, counts) and with its whitespace collapsed, so the same
/// complaint on two boots reads the same.
pub fn normalize_fault(line: &str) -> String {
    line.to_lowercase()
        .chars()
        .filter(|c| !c.is_ascii_digit())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect()
}

/// Every fault line in a log, normalized and deduplicated.
pub fn log_faults(logs: &str) -> Vec<String> {
    let mut out: Vec<String> = logs
        .lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            LOG_FAULTS.iter().any(|f| lower.contains(f))
        })
        .map(normalize_fault)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A container's logs (stdout and stderr), its last `tail` lines.
/// Scrubbed at the source ([`talaria_agent_secrets::scrub_text`]): every
/// secret the agents were given ([`known_secrets`]) and every
/// credential-shaped token is taken out before any of it is read, so no
/// fault, reason, notification or log line Talaria makes of it carries one.
pub async fn logs(container: &str, tail: u32) -> String {
    let raw = match docker(
        &["logs", "--tail", &tail.to_string(), container],
        Duration::from_secs(30),
    )
    .await
    {
        Ok((out, err)) => format!("{out}\n{err}"),
        Err(e) => format!("(logs unavailable: {e})"),
    };
    talaria_agent_secrets::scrub_text(&raw, &known_secrets())
}

/// Every secret value the agents were given: each credential-named value in
/// the fleet's `.env` (the gateway keys, each agent's own keys) and every
/// value in an agent's `secrets.env`.
pub fn known_secrets() -> Vec<String> {
    let dir = talaria_fleet_layout::fleet_dir();
    let mut out = Vec::new();
    let mut read = |path: std::path::PathBuf, all: bool| {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let Some((k, v)) = line.trim().split_once('=') else {
                continue;
            };
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            if k.trim_start().starts_with('#') || v.chars().count() < 3 {
                continue;
            }
            if all || talaria_agent_secrets::may_hold_credential(k.trim(), "x") {
                out.push(v.to_string());
            }
        }
    };
    read(dir.join(".env"), false);
    if let Ok(agents) = std::fs::read_dir(dir.join("agents")) {
        for a in agents.flatten() {
            read(a.path().join("secrets.env"), true);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A request that never got an answer: no connection, no name, no reply in
/// time. Docker's own health check (inside the container) already said the
/// API server answers, so not reaching it from here is the network between
/// the api and the container — the host's, never the image's.
fn unreached(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout() || e.is_request()
}

/// Whether the API server answers, the way the gateway reaches it: its
/// `/health`, then — with the agent's bearer — `/v1/models`, which Hermes
/// answers from its own config without calling a model. Not reaching it is
/// [`Fault::Infra`]; a wrong answer is [`Fault::Image`].
pub async fn api_answers(base_url: &str, bearer: Option<&str>) -> Result<(), Fault> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| Fault::Infra(format!("probe client would not build: {e}")))?;
    let base = base_url.trim_end_matches('/');
    let sent = |e: reqwest::Error, what: &str| {
        let text = format!("the gateway could not reach its API server at {base} ({what}): {e}");
        if unreached(&e) {
            Fault::Infra(text)
        } else {
            Fault::Image(text)
        }
    };
    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .map_err(|e| sent(e, "/health"))?;
    if !health.status().is_success() {
        return Err(Fault::Image(format!(
            "its API server answered {} to /health",
            health.status()
        )));
    }
    if let Some(key) = bearer.filter(|k| !k.is_empty()) {
        let models = client
            .get(format!("{base}/v1/models"))
            .bearer_auth(key)
            .send()
            .await
            .map_err(|e| sent(e, "/v1/models"))?;
        if !models.status().is_success() {
            return Err(Fault::Image(format!(
                "its API server answered {} to /v1/models",
                models.status()
            )));
        }
    }
    Ok(())
}

/// GET a JSON body with a bearer: the status and the body, or why there is
/// none.
pub async fn get_json(url: &str, bearer: Option<&str>) -> Result<(u16, serde_json::Value), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("probe client would not build: {e}"))?;
    let mut req = client.get(url);
    if let Some(key) = bearer.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.map_err(|e| format!("no answer: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(serde_json::Value::Null);
    Ok((status, body))
}

/// What docker says of a container, as a [`Snapshot`]. A container that does
/// not exist is a snapshot with `exists: false`; docker failing to answer is
/// an error.
pub async fn snapshot(container: &str) -> Result<Snapshot, String> {
    let format = "{{.Id}}|{{.Image}}|{{.State.Status}}|{{.State.Running}}|{{.RestartCount}}|{{if .State.Health}}{{.State.Health.Status}}{{end}}|{{.State.StartedAt}}";
    match docker(
        &["inspect", "--format", format, container],
        Duration::from_secs(30),
    )
    .await
    {
        Ok((out, _)) => Ok(parse_snapshot(&out)),
        Err(e) if e.to_lowercase().contains("no such") => Ok(Snapshot::default()),
        Err(e) => Err(e),
    }
}

/// Read [`snapshot`]'s format line.
pub fn parse_snapshot(out: &str) -> Snapshot {
    let mut p = out.trim().split('|');
    let mut next = || p.next().unwrap_or("").trim().to_string();
    let (container, image, status, running, restarts, health, started_at) =
        (next(), next(), next(), next(), next(), next(), next());
    Snapshot {
        exists: !container.is_empty(),
        running: running == "true",
        status,
        health: (!health.is_empty()).then_some(health),
        restart_count: restarts.parse().unwrap_or(0),
        container,
        image,
        started_at,
    }
}

/// The id `image` resolves to on this host.
pub async fn image_id(image: &str) -> Result<String, String> {
    docker(
        &["image", "inspect", "--format", "{{.Id}}", image],
        Duration::from_secs(30),
    )
    .await
    .map(|(out, _)| out.trim().to_string())
}

/// `docker image rm` (never forced), and whether the image is then gone from
/// the host — checked by its id, since removing a digest reference while a
/// tag still names the image (docker's classic store) only untags it and
/// frees nothing.
pub async fn remove_image_verified(image: &str) -> Result<bool, String> {
    let id = image_id(image).await?;
    docker(&["image", "rm", image], Duration::from_secs(120)).await?;
    Ok(image_id(&id).await.is_err())
}

/// The `repository@sha256:…` an image was pulled as: `prefer`'s first, else
/// the first. None when docker has none (built here, never pulled) or will
/// not answer.
pub async fn repo_digest(image_id: &str, prefer: &str) -> Option<String> {
    let (out, _) = docker(
        &[
            "image",
            "inspect",
            "--format",
            "{{json .RepoDigests}}",
            image_id,
        ],
        Duration::from_secs(30),
    )
    .await
    .ok()?;
    let digests: Vec<String> = serde_json::from_str(out.trim()).ok()?;
    let ok = |d: &&String| {
        d.split_once('@')
            .is_some_and(|(_, dg)| talaria_hermes_images::is_digest(dg))
    };
    digests
        .iter()
        .filter(ok)
        .find(|d| d.split_once('@').is_some_and(|(r, _)| r == prefer))
        .or_else(|| digests.iter().find(ok))
        .cloned()
}

/// Whether the drain's tools are where every roll and stop calls them, in a
/// RUNNING container: the gateway's s6 service and `s6-svc`/`s6-svwait`. The
/// test failing (an exit with nothing on stderr) is the image's; docker not
/// running the exec is the host's.
pub async fn drain_ready(container: &str) -> Result<(), Fault> {
    match docker(
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
    {
        Ok(_) => Ok(()),
        Err(e) if e.trim_end().ends_with("failed:") => Err(Fault::Image(
            "its gateway is not the s6 service Talaria drains (/run/service/gateway-default with /command/s6-svc and s6-svwait) — every roll and stop would hard-kill it".into(),
        )),
        Err(e) => Err(Fault::Infra(format!(
            "docker would not run the drain check in it: {e}"
        ))),
    }
}

/// Where docker's health check stands for a container.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Boot {
    Healthy,
    Booting,
    Unhealthy,
    Exited(String),
    /// The container is not there at all: something removed it (an
    /// operator, another stack's cleanup), which an image cannot do to its
    /// own container.
    Gone,
    Unreadable(String),
}

async fn boot_state(container: &str) -> Boot {
    match snapshot(container).await {
        Ok(s) if !s.exists => Boot::Gone,
        Ok(s) => match (s.status.as_str(), s.health.as_deref()) {
            ("running", Some("healthy")) => Boot::Healthy,
            ("running", Some("unhealthy")) => Boot::Unhealthy,
            ("exited" | "dead", _) => Boot::Exited("exited".into()),
            _ => Boot::Booting,
        },
        Err(e) => Boot::Unreadable(e),
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
/// config.yaml) shaping its config, ask it every health question, then keep
/// it up and ask again for [`canary_soak`]. The canary is taken down whatever
/// the answer. `Ok` carries what passed; a failure says whose it is. `known`
/// are the fault lines the agent `base` came from already logs on the image
/// the fleet runs: the canary is held to the same standard, not a stricter
/// one.
pub async fn run_canary(
    record: &ImageRecord,
    base: Option<&str>,
    known: &[String],
) -> Result<String, Fault> {
    let config_version = record
        .config_version
        .ok_or_else(|| Fault::Image("the image's config version is not known".to_string()))?;
    let image = record.image_ref();
    let files = render_canary(&image, config_version, base)
        .await
        .map_err(Fault::Infra)?;
    // A canary an interrupted step left behind goes first.
    canary_down(&files).await;
    let verdict = canary_checks(&files, &image, known, &record.accepted_problems()).await;
    if verdict.is_err() {
        let tail = logs(&files.container, 40).await;
        tracing::warn!("[hermes] canary of {image} failed; its last log lines:\n{tail}");
    }
    canary_down(&files).await;
    verdict
}

/// The config fault if the log names one, else its last word.
async fn last_word(container: &str) -> Option<String> {
    let log = logs(container, 400).await;
    log_fault(&log).or_else(|| {
        log.lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .map(|l| l.chars().take(120).collect())
    })
}

/// The first fault line in `logs` the fleet's own image does not log too.
fn new_fault(logs: &str, known: &[String]) -> Option<String> {
    logs.lines().find_map(|line| {
        let fault = log_fault(line)?;
        (!known.contains(&normalize_fault(line))).then_some(fault)
    })
}

/// The Hermes answers Talaria reads to tell whether an agent is working
/// (busy.rs), asked of the canary: a shape Talaria cannot read holds the
/// release for an admin.
async fn contract_shapes(url: &str, key: &str) -> Result<Vec<String>, String> {
    // No answer at all (a refused connection, a timeout on a loaded host) is
    // the host's to retry, never a changed contract; an answer of the wrong
    // status or shape is the contract.
    let mut problems = Vec::new();
    match get_json(&format!("{url}/health/detailed"), Some(key)).await {
        Ok((200, body)) => {
            if let Err(e) = crate::busy::hermes_busy(&body, false) {
                problems.push(format!("/health/detailed: {e}"));
            }
        }
        Ok((status, _)) => problems.push(format!("/health/detailed answered {status}")),
        Err(e) => return Err(format!("the canary's /health/detailed: {e}")),
    }
    match get_json(&format!("{url}/api/jobs?include_disabled=true"), Some(key)).await {
        Ok((200, body)) => {
            if let Err(e) = crate::busy::cron_busy(&body, talaria_agent_auth::now_ms()) {
                problems.push(format!("/api/jobs: {e}"));
            }
        }
        Ok((status, _)) => problems.push(format!(
            "/api/jobs answered {status}: Talaria lists, adds and watches an agent's scheduled jobs through it"
        )),
        Err(e) => return Err(format!("the canary's /api/jobs: {e}")),
    }
    Ok(problems)
}

async fn canary_checks(
    files: &CanaryFiles,
    image: &str,
    known: &[String],
    accepted: &[String],
) -> Result<String, Fault> {
    // The image is on the host already (prepared), so compose's default pull
    // policy pulls nothing; no `--pull` flag, which an older compose refuses
    // (and a refusal here would mark a good image bad).
    let up = files.compose_args(&["up", "-d"]);
    let refs: Vec<&str> = up.iter().map(String::as_str).collect();
    docker(&refs, CANARY_UP).await.map_err(|e| {
        Fault::Infra(format!(
            "the canary would not start: {}",
            e.chars().take(400).collect::<String>()
        ))
    })?;
    let started = tokio::time::Instant::now();
    loop {
        match boot_state(&files.container).await {
            Boot::Healthy => break,
            Boot::Gone => {
                return Err(Fault::Infra(
                    "the canary's container was removed by something else before it reported healthy (an operator, or another stack's cleanup)".into(),
                ));
            }
            Boot::Exited(how) => {
                let said = last_word(&files.container).await;
                return Err(Fault::Image(format!(
                    "the canary {how} before it reported healthy{}",
                    said.map(|f| format!(" — its log: {f}")).unwrap_or_default()
                )));
            }
            Boot::Unreadable(e) if started.elapsed() > CANARY_HEALTH_WAIT => {
                return Err(Fault::Infra(format!(
                    "docker would not describe the canary: {e}"
                )));
            }
            Boot::Booting | Boot::Unhealthy | Boot::Unreadable(_)
                if started.elapsed() > CANARY_HEALTH_WAIT =>
            {
                return Err(Fault::Image(format!(
                    "the canary did not report healthy within {} min",
                    CANARY_HEALTH_WAIT.as_secs() / 60
                )));
            }
            _ => tokio::time::sleep(Duration::from_secs(3)).await,
        }
    }
    let booted = started.elapsed().as_secs();
    let url = canary_url(files).await.map_err(Fault::Infra)?;
    let ask = |phase: &'static str| {
        let url = url.clone();
        async move {
            api_answers(&url, Some(&files.api_key))
                .await
                .map_err(|f| match f {
                    Fault::Image(e) => Fault::Image(format!("the canary is up but {e} ({phase})")),
                    Fault::Infra(e) => Fault::Infra(format!(
                        "docker reports the canary healthy, but {e} ({phase}) — the network between the api and the fleet, not the image"
                    )),
                    held => held,
                })?;
            if let Some(fault) = new_fault(&logs(&files.container, 2000).await, known) {
                return Err(Fault::Image(format!(
                    "the canary's log says: {fault} ({phase})"
                )));
            }
            Ok::<(), Fault>(())
        }
    };
    ask("at boot").await?;
    // Always asked: an admin's Accept covers only the problems they were
    // shown, and one found now that they were not holds the release again.
    let mut problems = contract_shapes(&url, &files.api_key)
        .await
        .map_err(Fault::Infra)?;
    problems.retain(|p| !accepted.contains(p));
    if !problems.is_empty() {
        return Err(Fault::Held(problems));
    }
    let runs = match (snapshot(&files.container).await, image_id(image).await) {
        (Ok(s), Ok(want)) => s.image == want,
        _ => false,
    };
    if !runs {
        return Err(Fault::Infra(format!("the canary does not run {image}")));
    }
    drain_ready(&files.container).await.map_err(|f| match f {
        Fault::Image(e) => Fault::Image(format!("the canary booted, but {e}")),
        other => other,
    })?;
    // The soak: up, healthy and answering for the whole of it. The canary is
    // `restart: no`, so a crash is an exit, never a quiet restart.
    let soak = canary_soak();
    let soaking = tokio::time::Instant::now();
    while soaking.elapsed() < soak {
        tokio::time::sleep(CANARY_SOAK_EVERY.min(soak)).await;
        let at = soaking.elapsed().as_secs();
        match boot_state(&files.container).await {
            Boot::Healthy => {}
            Boot::Gone => {
                return Err(Fault::Infra(format!(
                    "the canary's container was removed by something else {at}s into its soak (an operator, or another stack's cleanup)"
                )));
            }
            Boot::Exited(how) => {
                let said = last_word(&files.container).await;
                return Err(Fault::Image(format!(
                    "the canary {how} {at}s into its soak{}",
                    said.map(|f| format!(" — its log: {f}")).unwrap_or_default()
                )));
            }
            Boot::Unhealthy => {
                return Err(Fault::Image(format!(
                    "the canary turned unhealthy {at}s into its soak"
                )));
            }
            Boot::Booting => {
                return Err(Fault::Image(format!(
                    "the canary stopped reporting healthy {at}s into its soak"
                )));
            }
            Boot::Unreadable(e) => {
                return Err(Fault::Infra(format!(
                    "docker would not describe the canary {at}s into its soak: {e}"
                )));
            }
        }
        ask("during its soak").await?;
    }
    Ok(format!(
        "the canary reported healthy in {booted}s and stayed healthy for a {}s soak; its API server answered the gateway; its log names no config or migration fault; its drain tools are in place",
        soak.as_secs()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_or_migration_fault_in_the_log_is_named_and_a_clean_boot_is_not() {
        let clean = "[2026-11-03 01:02:03] gateway starting\nAPI server listening on 0.0.0.0:8642\n[config-migrate] HERMES_SKIP_CONFIG_MIGRATION is set; skipping config migration\n";
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

    #[test]
    fn the_same_fault_on_two_boots_reads_the_same_and_a_new_one_does_not() {
        let old = "2026-11-03 01:02:03,100 ERROR mcp: invalid config for server 'x' (pid 41)\n";
        let new = "2026-11-04 02:03:04,200 ERROR mcp: invalid config for server 'x' (pid 977)\n\
                   2026-11-04 02:03:05,000 ERROR boot: config migration failed\n";
        let before = log_faults(old);
        let after = log_faults(new);
        assert_eq!(before.len(), 1);
        let fresh: Vec<&String> = after.iter().filter(|f| !before.contains(f)).collect();
        assert_eq!(fresh.len(), 1, "{after:?}");
        assert!(fresh[0].contains("config migration failed"));
    }

    /// A server on 127.0.0.1 that answers every request with `status`.
    fn answering(status: &'static str, n: usize) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().take(n) {
                let Ok(mut s) = s else { continue };
                let mut buf = [0u8; 4096];
                let _ = std::io::Read::read(&mut s, &mut buf);
                let _ = std::io::Write::write_all(
                    &mut s,
                    format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}").as_bytes(),
                );
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn a_canary_that_does_not_answer_is_the_hosts_and_a_wrong_answer_is_the_contract() {
        // Nothing listens: no answer at all, the host's to retry.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let e = contract_shapes(&format!("http://127.0.0.1:{port}"), "k")
            .await
            .unwrap_err();
        assert!(e.contains("/health/detailed"), "{e}");
        // An answer of the wrong status: the contract changed.
        let url = answering("404 Not Found", 2);
        let problems = contract_shapes(&url, "k").await.unwrap();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].contains("/health/detailed answered 404"),
            "{problems:?}"
        );
        assert!(
            problems[1].contains("/api/jobs answered 404"),
            "{problems:?}"
        );
        // An answer of the wrong shape: the contract changed.
        let url = answering("200 OK", 2);
        let problems = contract_shapes(&url, "k").await.unwrap();
        assert!(
            problems[0].contains("api_server.active_runs"),
            "{problems:?}"
        );
    }

    #[test]
    fn a_snapshot_reads_dockers_line() {
        let s = parse_snapshot("abc|sha256:img|restarting|false|3|starting\n");
        assert!(s.exists && !s.running);
        assert_eq!(
            (s.status.as_str(), s.restart_count, s.health.as_deref()),
            ("restarting", 3, Some("starting"))
        );
        let s = parse_snapshot("abc|sha256:img|running|true|0||2026-11-03T15:00:00.1Z\n");
        assert_eq!(s.health, None);
        assert_eq!(s.started_at, "2026-11-03T15:00:00.1Z");
        assert!(!parse_snapshot("").exists);
    }

    /// A remove that only untags is not a removal: the prune checks the
    /// image's id is gone. Run by hand on a host with docker:
    /// `TALARIA_PRUNE_TEST_BASE=busybox:1.37` (any small local image).
    #[tokio::test]
    #[ignore = "needs docker and a small local image (TALARIA_PRUNE_TEST_BASE)"]
    async fn a_remove_that_only_untags_is_not_counted_as_removed() {
        let Ok(base) = std::env::var("TALARIA_PRUNE_TEST_BASE") else {
            eprintln!("TALARIA_PRUNE_TEST_BASE is not set: nothing to check");
            return;
        };
        let t = Duration::from_secs(120);
        let (c, _) = docker(&["create", &base, "true"], t).await.unwrap();
        let c = c.trim().to_string();
        let (a, b) = ("talaria-prune-test/x:a", "talaria-prune-test/x:b");
        docker(&["commit", &c, a], t).await.unwrap();
        docker(&["rm", &c], t).await.unwrap();
        docker(&["tag", a, b], t).await.unwrap();
        let first = remove_image_verified(a).await;
        let second = remove_image_verified(b).await;
        let _ = docker(&["image", "rm", a, b], t).await;
        println!("first remove: {first:?}, second: {second:?}");
        assert_eq!(first, Ok(false), "another tag holds it: untagged only");
        assert_eq!(second, Ok(true), "its last reference: gone");
    }
}
