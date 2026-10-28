// THE AGENT'S BROWSER ENGINE, installed onto its volume before it starts.
//
// Hermes v0.22.4's browser drives the image's own pinned Chromium
// (`/opt/hermes/tools/chromium-1208`) through `agent-browser`, a native CLI
// that Hermes's package manager (pm) pins in the image's own lock
// (`pm/lock.json`: agent-browser 0.26.0, sha256-checked). The image does not
// stage that CLI: its Dockerfile provisions uv, chromium, npm, ffmpeg and
// ripgrep, and nothing else. And Hermes decides whether to OFFER the
// `browser_*` tools by looking for the CLI without installing it
// (`check_browser_requirements` → `_find_agent_browser(validate=False)`), so
// on a fresh volume those tools are never registered at all. pm's own package
// note says so: "an install without it silently loses every browser_* tool".
// `browser_exec`, the Browser Use tool Hermes offers by default instead,
// would fetch the CLI on its first call, but only a session that has the
// `terminal` toolset ever sees `browser_exec`, and a Talaria agent starts
// with `terminal` closed. Measured on the pinned image: an agent with the
// browser toolset on was offered no browser tool of either kind.
//
// So before an agent whose rendered config keeps the browser toolset on
// starts, Talaria asks the image's own pm to install that pinned CLI onto the
// agent's own state volume: a one-shot container of the agent's own service
// (`compose run --rm --no-deps`, so the image, volume, network and resolvers
// are exactly the agent's), as the image's `hermes` user, running
// `hermes pm install agent-browser`, the command Hermes's own install hint
// names. It lands where Hermes's lazy install would put it (`/opt/data/tools`,
// pm's writable store beside the image's sealed one). The Chromium it drives
// is the one already in the image, so no browser is installed anywhere, and
// nothing is fetched that the image's lock does not pin. A volume that
// already has it answers at once: pm checks its recorded fact against the
// lock, and only a new Hermes pin whose lock moves the version fetches again,
// on the first start after the bump.
//
// It never execs into a running agent. On a roll it runs before the drain,
// so the agent keeps serving while it runs and is untouched if it fails.
//
// Best effort, never a failed start: with no route to the npm registry the
// install fails, the reason is logged, and the agent starts without browser
// tools, as every agent did before this existed.
//
// WHICH agents: the render decides, because only the render knows the
// effective toolset list (the floor, the def's own list and the authored
// config merged). It writes the departments whose agents keep `browser` on
// to `fleet/browser-engine.json` ([`manifest_text`]); `compose_up` reads it.
// No file, or a department not in it, is no install: an agent rendered
// before this existed starts as it always did, and the next render writes
// the file.

use std::time::Duration;

use serde_json::{Value, json};

/// The render's declaration of which departments need the engine, beside
/// `docker-compose.yml` in the fleet dir.
pub const MANIFEST_FILE: &str = "browser-engine.json";

/// The pm package. The image's lock pins its version and digest.
const PACKAGE: &str = "agent-browser";

/// The image's own non-root user, the one its s6 init drops to and the one
/// that owns `/opt/data`. Files pm writes onto the volume as root would be
/// root-owned, and the agent's own later installs into the same store would
/// then fail.
const IMAGE_USER: &str = "hermes";

/// A first install downloads the CLI (one tarball: 18 s measured on the
/// pinned image); an installed one answers in the time a container takes to
/// start.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

/// The manifest the render writes: `{"departments": [...]}`, sorted and
/// deduped so a render of the same fleet writes the same bytes.
pub fn manifest_text(departments: &[String]) -> String {
    let mut departments: Vec<&str> = departments.iter().map(String::as_str).collect();
    departments.sort_unstable();
    departments.dedup();
    let mut text = serde_json::to_string_pretty(&json!({ "departments": departments }))
        .expect("a list of strings serializes");
    text.push('\n');
    text
}

/// Whether the manifest names this department. A manifest that does not
/// parse names nobody: the install is an addition, so an unreadable
/// declaration must not turn it on for agents it was never meant for.
fn manifest_wants(text: &str, department: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .as_ref()
        .and_then(|v| v.get("departments"))
        .and_then(Value::as_array)
        .is_some_and(|ds| ds.iter().any(|d| d.as_str() == Some(department)))
}

/// Whether the last render kept the browser toolset on for this department.
async fn wanted(department: &str) -> bool {
    let path = talaria_fleet_layout::fleet_dir().join(MANIFEST_FILE);
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => manifest_wants(&text, department),
        Err(_) => false,
    }
}

/// The one-shot's compose arguments (after the project, file and env-file
/// prefix). `--entrypoint hermes` replaces the image's s6 init, so nothing
/// but pm runs: no gateway, no cron, no lock on the agent's volume.
fn install_args(service: &str) -> Vec<&str> {
    vec![
        "run",
        "--rm",
        "--no-deps",
        "-T",
        "--user",
        IMAGE_USER,
        "--entrypoint",
        "hermes",
        service,
        "pm",
        "install",
        PACKAGE,
    ]
}

/// Install the browser engine onto `service`'s volume when the render asked
/// for it. Never an error: a failure is logged and the start goes on.
pub(crate) async fn ensure(department: &str, service: &str) {
    if !wanted(department).await {
        return;
    }
    let args = crate::compose_args(&install_args(service));
    match run(&args).await {
        Ok(()) => tracing::info!("[fleet] {service}: browser engine ({PACKAGE}) is installed"),
        Err(e) => tracing::warn!(
            "[fleet] {service}: browser engine ({PACKAGE}) not installed, so this start has no browser tools: {e}"
        ),
    }
}

/// The one-shot, with the reason kept when it fails. pm reports a failed
/// install on STDOUT (`✗ agent-browser: …`), so the shared `docker` helper,
/// which keeps only stderr, would log compose's bare exit line and lose why.
async fn run(args: &[String]) -> Result<(), String> {
    let out = tokio::time::timeout(INSTALL_TIMEOUT, async {
        // kill_on_drop: a timed-out compose CLI dies with this future. The
        // one-off container it started finishes pm's install on its own and
        // removes itself (`--rm`); it holds no port and no lock.
        tokio::process::Command::new("docker")
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| format!("docker compose run: {e}"))
    })
    .await
    .map_err(|_| {
        format!(
            "docker compose run: timed out after {}s",
            INSTALL_TIMEOUT.as_secs()
        )
    })??;
    if out.status.success() {
        return Ok(());
    }
    Err(failure_reason(
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    ))
}

/// What a failed one-shot said, both streams, trimmed to the last few lines
/// of each: pm's own verdict is the last thing it prints.
fn failure_reason(stdout: &str, stderr: &str) -> String {
    let tail = |s: &str| {
        let lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        lines[lines.len().saturating_sub(4)..].join(" / ")
    };
    match (tail(stdout), tail(stderr)) {
        (o, e) if o.is_empty() && e.is_empty() => "exited non-zero with no output".to_string(),
        (o, e) if e.is_empty() => o,
        (o, e) if o.is_empty() => e,
        (o, e) => format!("{o} / {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_is_sorted_deduped_and_stable() {
        let text = manifest_text(&["web".into(), "research".into(), "web".into()]);
        assert_eq!(
            text,
            "{\n  \"departments\": [\n    \"research\",\n    \"web\"\n  ]\n}\n"
        );
        assert_eq!(manifest_text(&[]), "{\n  \"departments\": []\n}\n");
    }

    #[test]
    fn the_manifest_names_exactly_its_departments() {
        let text = manifest_text(&["research".into(), "web-desk".into()]);
        assert!(manifest_wants(&text, "research"));
        assert!(manifest_wants(&text, "web-desk"));
        // Exact match only: a department that is a prefix of a listed one is
        // not in the list.
        assert!(!manifest_wants(&text, "web"));
        assert!(!manifest_wants(&text, "ops"));
    }

    #[test]
    fn an_unreadable_manifest_names_nobody() {
        for text in [
            "",
            "not json",
            "[\"research\"]",
            "{\"departments\": \"research\"}",
            "{\"departments\": [1, null]}",
            "{}",
        ] {
            assert!(!manifest_wants(text, "research"), "{text:?}");
        }
    }

    #[test]
    fn a_failure_keeps_pms_own_verdict_from_stdout() {
        assert_eq!(
            failure_reason(
                "resolving agent-browser\n✗ agent-browser: download failed: Temporary failure in name resolution\n",
                ""
            ),
            "resolving agent-browser / ✗ agent-browser: download failed: Temporary failure in name resolution"
        );
        // Only the last four lines of a long stream.
        assert_eq!(failure_reason("1\n2\n3\n4\n5\n6\n", ""), "3 / 4 / 5 / 6");
        assert_eq!(
            failure_reason("✗ agent-browser: no", "Error: exit 1"),
            "✗ agent-browser: no / Error: exit 1"
        );
        assert_eq!(failure_reason("", "  \n"), "exited non-zero with no output");
    }

    #[test]
    fn the_install_is_the_agents_own_service_as_hermes_running_only_pm() {
        assert_eq!(
            install_args("agent-research"),
            [
                "run",
                "--rm",
                "--no-deps",
                "-T",
                "--user",
                "hermes",
                "--entrypoint",
                "hermes",
                "agent-research",
                "pm",
                "install",
                "agent-browser",
            ]
        );
        // The full command line addresses the rendered compose file and the
        // fleet .env, like every other compose verb here.
        let args = crate::compose_args(&install_args("agent-research"));
        assert_eq!(args[0], "compose");
        assert!(args[4].ends_with("docker-compose.yml"), "{}", args[4]);
        assert_eq!(args[7], "run");
    }
}
