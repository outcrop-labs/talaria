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
// `terminal` toolset ever sees `browser_exec`. Measured on the pinned image:
// an agent with the browser toolset on and the terminal closed (a new
// agent's default) was offered no browser tool of either kind.
//
// WHO GETS IT is the terminal's call (talaria-fleet-render's
// `fleet_browser_engine`): an agent with both the browser and the terminal
// on. Hermes's built-in `browser_navigate` tools would work without a
// terminal, but they are a local browser, and v0.22.4 skips the
// private-address check for a local browser on the grounds that the
// terminal reaches those networks anyway, which is false for an agent with
// no terminal. So a terminal-less agent gets no engine and no browser tool,
// exactly as before this existed, and an upgrade installs nothing for it.
//
// So before an agent whose rendered config keeps the browser and terminal
// toolsets on starts, Talaria asks the image's own pm to install that pinned
// CLI onto the agent's own state volume (it would otherwise arrive on the
// first `browser_exec` call, inside a turn, with any failure lost there): a
// one-shot container of the agent's own service
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
// tools, as every agent did before this existed. And bounded: a host whose
// egress DROPS packets instead of refusing them would otherwise hold every
// start for minutes (pm retries each of its two mirrors four times; 145 s
// measured with a resolver that never answers, longer on a dropped TCP
// connect), so the one-shot is capped ([`INSTALL_TIMEOUT`]), removed when it
// runs past the cap, and a timeout makes the next few minutes of starts skip
// it ([`TIMEOUT_COOLDOWN`]).
//
// WHICH agents: the render decides, because only the render knows the
// effective toolset list (the floor, the def's own list and the authored
// config merged). It writes the departments whose agents keep both
// `browser` and `terminal` on to `fleet/browser-engine.json`
// ([`manifest_text`]); `compose_up` reads it. No file, or a department not
// in it, is no install: an agent rendered before this existed starts as it
// always did, and the next render writes the file, without a terminal-less
// agent in it.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// How long a start waits for the one-shot. A first install downloads the
/// CLI (one tarball: 4 to 18 s measured on the pinned image, container start
/// included); an installed one answers in the time a container takes to
/// start (4.3 s). The cap is five times the slowest install measured. It is
/// what a start pays on a host whose egress drops packets: uncapped, pm's
/// own retries (four attempts per mirror on 60 s and 120 s socket timeouts,
/// pm/network.py and pm/downloader.py) held a start for 145 s with a
/// resolver that never answers, and longer on a dropped connect.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(90);

/// After a one-shot TIMES OUT, how long every start skips the install (an
/// engine an earlier start put on a volume stays there and works). A
/// timeout is the egress, which every agent shares (one network, one pair of
/// resolvers, one registry), not the agent, so the memory is fleet-wide: a
/// reconcile that starts ten browser-on agents on such a host pays the cap
/// once, not ten times. A fast failure (no DNS, a refused connection, an
/// HTTP error) costs seconds and is not remembered, so the first start after
/// the fix installs. In-process: an api restart forgets it, and the next
/// start tries again.
const TIMEOUT_COOLDOWN: Duration = Duration::from_secs(300);

/// When the last one-shot timed out, if one did since this api started.
static LAST_TIMEOUT: Mutex<Option<Instant>> = Mutex::new(None);

/// The one-shot names a sweep is watching ([`sweep_after_timeout`]). A start
/// skips its install while its name is here: the sweep force-removes
/// whatever container lands under the name, so it would remove that start's
/// own one-shot mid-install. The cooldown does not cover this on its own: it
/// is fleet-wide, and another agent's install that succeeds ends it (the
/// egress works again) while this name's sweep still runs.
static SWEEPING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Holds a name in [`SWEEPING`] for as long as it lives: the sweep's task
/// owns it, so the name is released when the sweep ends, or if it panics.
struct Sweeping(String);

impl Sweeping {
    fn hold(name: &str) -> Self {
        SWEEPING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string());
        Sweeping(name.to_string())
    }
}

impl Drop for Sweeping {
    fn drop(&mut self) {
        SWEEPING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Whether a sweep still watches `name`.
fn being_swept(name: &str) -> bool {
    SWEEPING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(name)
}

/// Why a one-shot did not install.
#[derive(Debug, PartialEq)]
enum Failure {
    /// It ran past [`INSTALL_TIMEOUT`], and its container was removed.
    TimedOut,
    /// It exited non-zero, or could not be run, with what it said.
    Failed(String),
}

/// How much of the cooldown is left after a timeout at `last`, or `None`
/// when a start should try the install.
fn cooling_down(last: Option<Instant>, now: Instant) -> Option<Duration> {
    let left = TIMEOUT_COOLDOWN.saturating_sub(now.saturating_duration_since(last?));
    (!left.is_zero()).then_some(left)
}

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

/// Whether the last render kept the browser and terminal toolsets on for
/// this department.
async fn wanted(department: &str) -> bool {
    let path = talaria_fleet_layout::fleet_dir().join(MANIFEST_FILE);
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => manifest_wants(&text, department),
        Err(_) => false,
    }
}

/// The one-shot's container name: fixed per service, so a one-shot that
/// runs past the cap is removed by name, and a second start of the same
/// agent cannot run a second install beside the first (docker refuses the
/// name). It never matches an agent's own container, which compose names
/// `<project>-<service>-1` ([`crate::slot_container`]).
///
/// THE NAME CAN BE LEFT BEHIND. `--rm` is docker's AutoRemove, which removes
/// a container once it has RUN and exited: a one-shot whose api died
/// mid-install finishes and removes itself (measured: it outlives a killed
/// compose CLI, then goes when it exits). A one-shot that never started is
/// not covered. Under heavy IO a start's compose CLI was killed at the cap
/// while the daemon was still creating its container; the remove after the
/// kill ran before the create landed, or ran out its own time, and the
/// create then landed with nobody left to start it. That container sat in
/// `created` (seen: `agent-eng-browser-engine`), and every later start's
/// install failed at once on its name, so the agent never got its browser
/// tools. So a start clears a leftover first ([`clear_leftover`]), and a
/// timed-out one-shot is watched until it is gone ([`sweep_after_timeout`]).
fn one_shot_name(project: &str, service: &str) -> String {
    format!("{project}-{service}-browser-engine")
}

/// The one-shot's compose arguments (after the project, file and env-file
/// prefix). `--entrypoint hermes` replaces the image's s6 init, so nothing
/// but pm runs: no gateway, no cron, no lock on the agent's volume.
fn install_args<'a>(service: &'a str, name: &'a str) -> Vec<&'a str> {
    vec![
        "run",
        "--rm",
        "--no-deps",
        "-T",
        "--name",
        name,
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
    let last = *LAST_TIMEOUT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(left) = cooling_down(last, Instant::now()) {
        tracing::warn!(
            "[fleet] {service}: browser engine ({PACKAGE}) install skipped for this start: an install timed out after {}s less than {}s ago (this host's egress drops packets). An engine already on the agent's volume still works; a missing one stays missing until a start in {}s or later",
            INSTALL_TIMEOUT.as_secs(),
            TIMEOUT_COOLDOWN.as_secs(),
            left.as_secs()
        );
        return;
    }
    let name = one_shot_name(&talaria_fleet_layout::fleet_project(), service);
    if being_swept(&name) {
        tracing::warn!(
            "[fleet] {service}: browser engine ({PACKAGE}) install skipped for this start: this agent's last install timed out less than {}s ago, and its name ({name}) is still watched for a container that lands late, which would remove this start's own. An engine already on the agent's volume still works; a start after the watch installs",
            SWEEP_FOR.as_secs()
        );
        return;
    }
    // An earlier one-shot's container under this name: cleared first, or
    // this start's install would fail on the name (and every one after it).
    match clear_leftover("docker", &name).await {
        Leftover::None => {}
        Leftover::Cleared(state) => tracing::warn!(
            "[fleet] {service}: removed the browser engine install container an earlier start left {state} ({name}) — installing again"
        ),
        Leftover::Running => {
            tracing::warn!(
                "[fleet] {service}: an earlier start's browser engine install ({name}) is still running — this start runs no second one beside it; the agent starts with whatever engine its volume has"
            );
            return;
        }
        Leftover::Stuck(why) => {
            tracing::warn!(
                "[fleet] {service}: browser engine ({PACKAGE}) install skipped for this start: a container an earlier install left ({name}) could not be cleared: {why}"
            );
            return;
        }
    }
    let args = crate::compose_args(&install_args(service, &name));
    let outcome = run("docker", &args, &name, INSTALL_TIMEOUT).await;
    {
        let mut last = LAST_TIMEOUT.lock().unwrap_or_else(|e| e.into_inner());
        match &outcome {
            Ok(()) => *last = None,
            Err(Failure::TimedOut) => *last = Some(Instant::now()),
            Err(Failure::Failed(_)) => {}
        }
    }
    match outcome {
        Ok(()) => tracing::info!("[fleet] {service}: browser engine ({PACKAGE}) is installed"),
        Err(Failure::TimedOut) => tracing::warn!(
            "[fleet] {service}: browser engine ({PACKAGE}) not installed, so this start has no browser tools: the install ran past {}s and its container is removed (its name is watched {}s for one that lands late); starts skip it for {}s",
            INSTALL_TIMEOUT.as_secs(),
            SWEEP_FOR.as_secs(),
            TIMEOUT_COOLDOWN.as_secs()
        ),
        Err(Failure::Failed(e)) => tracing::warn!(
            "[fleet] {service}: browser engine ({PACKAGE}) not installed, so this start has no browser tools: {e}"
        ),
    }
}

/// The one-shot (`program` is `docker`; the tests pass a stand-in), with
/// the reason kept when it fails. pm reports a failed install on STDOUT
/// (`✗ agent-browser: …`), so the shared `docker` helper, which keeps only
/// stderr, would log compose's bare exit line and lose why.
async fn run(program: &str, args: &[String], name: &str, cap: Duration) -> Result<(), Failure> {
    let out = tokio::time::timeout(
        cap,
        // kill_on_drop: a timed-out compose CLI dies with this future.
        tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let out = match out {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(Failure::Failed(format!("{program} compose run: {e}"))),
        Err(_) => {
            // Killing the compose CLI does not stop the container it
            // started, which would go on running pm against the volume while
            // the start (re)creates the agent on it. pm publishes atomically
            // under its store's lock (a staging dir, then a rename), so that
            // overlap could not leave a half-written engine, but the cap is
            // only a bound if the one-shot is really gone: remove it. And a
            // create the daemon had not finished when the CLI died lands
            // after this remove, as a container nobody starts: watched for
            // off the start's path, and removed when it appears.
            if !docker_ok(program, &["container", "rm", "-f", name], DOCKER_CALL).await {
                tracing::debug!("[fleet] {name}: the remove after the timeout did not land");
            }
            // Held before this returns, so no start can run a one-shot
            // under the name between the timeout and the sweep's first read.
            let held = Sweeping::hold(name);
            let (program, name) = (program.to_string(), name.to_string());
            tokio::spawn(async move {
                let _held = held;
                sweep_after_timeout(&program, &name, SWEEP_EVERY, SWEEP_FOR).await;
            });
            return Err(Failure::TimedOut);
        }
    };
    if out.status.success() {
        return Ok(());
    }
    Err(Failure::Failed(failure_reason(
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )))
}

/// How long one docker call of the leftover handling may take: a remove of a
/// stopped container, an inspect.
const DOCKER_CALL: Duration = Duration::from_secs(20);

/// After a timed-out one-shot, how often and for how long its name is
/// watched for a container that landed late ([`sweep_after_timeout`]). No
/// start runs a one-shot of its own under the name while the sweep still
/// watches it: [`ensure`] skips the install for a name in [`SWEEPING`].
/// The cooldown, though longer, is fleet-wide and ends at any install that
/// succeeds, so it cannot promise that on its own.
const SWEEP_EVERY: Duration = Duration::from_secs(5);
const SWEEP_FOR: Duration = Duration::from_secs(120);

/// What a start found under the one-shot's name before running one.
#[derive(Debug, PartialEq)]
enum Leftover {
    /// Nothing: the name is free.
    None,
    /// A container an earlier one-shot left in this state (`created`: its
    /// CLI died between the daemon's create and its start; `exited` or
    /// `dead`: its remove did not land), now removed.
    Cleared(String),
    /// An earlier start's install is still running under the name.
    Running,
    /// It could not be read or removed: why.
    Stuck(String),
}

/// One docker call (`program` is `docker`; the tests pass a stand-in):
/// whether it exited zero within `cap`.
async fn docker_ok(program: &str, args: &[&str], cap: Duration) -> bool {
    matches!(docker_out(program, args, cap).await, Ok((true, _, _)))
}

/// One docker call's exit, stdout and stderr, or why it could not be had.
async fn docker_out(
    program: &str,
    args: &[&str],
    cap: Duration,
) -> Result<(bool, String, String), String> {
    match tokio::time::timeout(
        cap,
        tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(out)) => Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        )),
        Ok(Err(e)) => Err(format!("{program}: {e}")),
        Err(_) => Err(format!("docker did not answer in {}s", cap.as_secs())),
    }
}

/// The state of the container under `name`: `Ok(None)` when there is none.
async fn one_shot_state(program: &str, name: &str) -> Result<Option<String>, String> {
    let (ok, stdout, stderr) = docker_out(
        program,
        &["container", "inspect", "-f", "{{.State.Status}}", name],
        DOCKER_CALL,
    )
    .await?;
    if ok {
        return Ok(Some(stdout));
    }
    // Docker's wording for a name no container holds: "No such container"
    // from `docker container inspect` (29.8.1 here); a bare `docker inspect`
    // says "no such object", lower case on 29.x. `container` keeps an image,
    // volume or network of the same name from answering instead.
    if stderr.to_ascii_lowercase().contains("no such") {
        return Ok(None);
    }
    Err(if stderr.is_empty() {
        "docker container inspect failed with no output".into()
    } else {
        stderr
    })
}

/// Clear what an earlier one-shot left under `name` before a start runs
/// its own: a container that is not running is removed (`docker rm`,
/// unforced, which refuses one that runs); a running one is left to finish,
/// and this start runs no second install beside it.
async fn clear_leftover(program: &str, name: &str) -> Leftover {
    match one_shot_state(program, name).await {
        Ok(None) => Leftover::None,
        Ok(Some(state)) if matches!(state.as_str(), "running" | "restarting" | "paused") => {
            Leftover::Running
        }
        Ok(Some(state)) => match docker_out(program, &["container", "rm", name], DOCKER_CALL).await
        {
            Ok((true, _, _)) => Leftover::Cleared(state),
            Ok((false, _, stderr)) => Leftover::Stuck(stderr),
            Err(why) => Leftover::Stuck(why),
        },
        Err(why) => Leftover::Stuck(why),
    }
}

/// After a timed-out one-shot: watch `name` for `within`, every `every`,
/// and remove whatever container is there — the create that landed after
/// the remove, or a remove that did not land. An empty name is no proof:
/// the late create can land at any point in the window. Done early once a
/// remove has landed and the name reads empty after it. Off every start's
/// path.
async fn sweep_after_timeout(program: &str, name: &str, every: Duration, within: Duration) {
    let deadline = tokio::time::Instant::now() + within;
    let mut removed = false;
    let mut present = false;
    loop {
        match one_shot_state(program, name).await {
            Ok(None) => {
                present = false;
                if removed {
                    return;
                }
            }
            Ok(Some(state)) => {
                present = true;
                if docker_ok(program, &["container", "rm", "-f", name], DOCKER_CALL).await {
                    removed = true;
                    tracing::warn!(
                        "[fleet] {name}: removed the browser engine install container that was {state} after its install timed out"
                    );
                }
            }
            Err(why) => tracing::debug!("[fleet] {name}: sweep read failed: {why}"),
        }
        if tokio::time::Instant::now() + every > deadline {
            if present {
                tracing::warn!(
                    "[fleet] {name}: a browser engine install container is still there {}s after its install timed out — the next start clears it",
                    within.as_secs()
                );
            }
            return;
        }
        tokio::time::sleep(every).await;
    }
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
        let name = one_shot_name("talaria-fleet", "agent-research");
        assert_eq!(
            install_args("agent-research", &name),
            [
                "run",
                "--rm",
                "--no-deps",
                "-T",
                "--name",
                "talaria-fleet-agent-research-browser-engine",
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
        let args = crate::compose_args(&install_args("agent-research", &name));
        assert_eq!(args[0], "compose");
        assert!(args[4].ends_with("docker-compose.yml"), "{}", args[4]);
        assert_eq!(args[7], "run");
    }

    #[test]
    fn the_one_shot_is_never_named_like_an_agent() {
        // An agent's container is `<project>-<service>-1`; the one-shot's
        // name cannot be any department's, so status and the roll, which
        // find agents by exact name, never read it as one.
        let project = talaria_fleet_layout::fleet_project();
        for dept in ["research", "research-browser-engine", "a"] {
            let one_shot = one_shot_name(&project, &crate::slot_service(dept, crate::Slot::A));
            for other in ["research", "research-browser-engine", "a", "a-browser"] {
                for slot in [crate::Slot::A, crate::Slot::B] {
                    assert_ne!(
                        one_shot,
                        crate::slot_container(other, slot),
                        "{dept} vs {other}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_timeout_cools_the_install_down_for_a_while_and_no_longer() {
        let now = Instant::now();
        // Nothing timed out: install.
        assert_eq!(cooling_down(None, now), None);
        // Just timed out: skip for the whole window.
        assert_eq!(cooling_down(Some(now), now), Some(TIMEOUT_COOLDOWN));
        // Part of the way through: what is left.
        let later = now + Duration::from_secs(100);
        assert_eq!(
            cooling_down(Some(now), later),
            Some(TIMEOUT_COOLDOWN - Duration::from_secs(100))
        );
        // At and past the end of the window: install again.
        assert_eq!(cooling_down(Some(now), now + TIMEOUT_COOLDOWN), None);
        assert_eq!(
            cooling_down(Some(now), now + TIMEOUT_COOLDOWN + Duration::from_secs(1)),
            None
        );
        // The window covers the cap many times over, so a reconcile that
        // starts a fleet of browser-on agents pays the cap once.
        const { assert!(TIMEOUT_COOLDOWN.as_secs() >= 3 * INSTALL_TIMEOUT.as_secs()) };
    }

    /// A stand-in for docker in a directory of its own, removed when it is
    /// dropped (a test that fails still leaves nothing on the tmpfs).
    struct FakeDocker {
        dir: std::path::PathBuf,
        script: String,
        state: std::path::PathBuf,
    }

    impl Drop for FakeDocker {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A stand-in for docker that keeps one container's state in a file
    /// beside it: `container inspect` prints it (no file: a lower-case
    /// "error: no such container", as docker 29 words its misses — a
    /// case-sensitive match misread one as a stuck container and skipped
    /// every install), `container rm` refuses a running one, `container rm
    /// -f` removes whatever is there. Anything but a `container` call fails.
    fn fake_docker(tag: &str) -> FakeDocker {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "talaria-browser-engine-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir");
        let state = dir.join("state");
        let script = dir.join("docker");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
S="{state}"
[ "$1" = container ] || exit 2
shift
case "$1" in
  inspect) [ -f "$S" ] && {{ cat "$S"; exit 0; }}; echo "error: no such container: x" >&2; exit 1 ;;
  rm)
    if [ "$2" = "-f" ]; then rm -f "$S"; exit 0; fi
    [ -f "$S" ] || {{ echo "Error: No such container: x" >&2; exit 1; }}
    [ "$(cat "$S")" = running ] && {{ echo "Error response from daemon: cannot remove container: container is running" >&2; exit 1; }}
    rm -f "$S"; exit 0 ;;
esac
exit 2
"#,
                state = state.display()
            ),
        )
        .expect("fake docker");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        // Another test's child, forked while the script was open for
        // writing, holds that descriptor until it execs: executing the
        // script then fails with ETXTBSY. Wait until it runs.
        for _ in 0..100 {
            match std::process::Command::new(&script).arg("probe").output() {
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                _ => break,
            }
        }
        FakeDocker {
            script: script.to_str().expect("utf-8 path").to_string(),
            dir,
            state,
        }
    }

    #[tokio::test]
    async fn a_one_shot_left_behind_is_cleared_before_a_start_runs_its_own() {
        let fake = fake_docker("leftover");
        let (docker, state) = (fake.script.as_str(), &fake.state);
        // Nothing there: the name is free.
        assert_eq!(clear_leftover(docker, "x").await, Leftover::None);
        // Created and never started (its CLI died before the start), or
        // exited with its remove lost: removed, and the start goes on.
        for left in ["created", "exited", "dead"] {
            std::fs::write(state, left).expect("state");
            assert_eq!(
                clear_leftover(docker, "x").await,
                Leftover::Cleared(left.to_string())
            );
            assert!(!state.exists(), "{left} was not removed");
        }
        // Still running: an earlier start's install, left to finish — no
        // second one beside it, and it is not killed.
        std::fs::write(state, "running").expect("state");
        assert_eq!(clear_leftover(docker, "x").await, Leftover::Running);
        assert!(state.exists());
        // Docker answering with something else: not guessed at.
        assert!(matches!(
            clear_leftover("/nonexistent/talaria-docker", "x").await,
            Leftover::Stuck(_)
        ));
    }

    #[tokio::test]
    async fn a_one_shot_whose_create_lands_after_its_timeout_is_removed() {
        let fake = fake_docker("sweep");
        let (docker, state) = (fake.script.clone(), &fake.state);
        // The remove after the timeout found nothing; the daemon's create
        // lands a moment later, as a container nobody will start.
        let sweep = {
            let docker = docker.clone();
            tokio::spawn(async move {
                sweep_after_timeout(
                    &docker,
                    "x",
                    Duration::from_millis(50),
                    Duration::from_secs(5),
                )
                .await;
            })
        };
        tokio::time::sleep(Duration::from_millis(120)).await;
        std::fs::write(state, "created").expect("state");
        tokio::time::timeout(Duration::from_secs(10), sweep)
            .await
            .expect("the sweep ends")
            .expect("the sweep task");
        assert!(!state.exists(), "the late one-shot was not removed");
        // A sweep that only ever reads the name empty watches its whole
        // window (a late create can land at any point in it), and no more.
        let started = Instant::now();
        sweep_after_timeout(
            &docker,
            "x",
            Duration::from_millis(50),
            Duration::from_millis(400),
        )
        .await;
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(350) && took < Duration::from_secs(3),
            "{took:?}"
        );
    }

    #[tokio::test]
    async fn a_one_shot_past_the_cap_is_a_timeout_and_is_killed() {
        // `sleep` stands in for docker, so no daemon is needed: a one-shot
        // that outlives the cap is a timeout (not a failure with a reason),
        // and the CLI dies with it rather than running on.
        let started = Instant::now();
        let got = run(
            "sleep",
            &["30".into()],
            "nobody",
            Duration::from_millis(200),
        )
        .await;
        assert_eq!(got, Err(Failure::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_timed_out_one_shot_keeps_its_name_from_a_start_until_its_sweep_ends() {
        // Two agents' installs overlap; one times out and the other then
        // succeeds, which ends the fleet-wide cooldown. The timed-out one's
        // name is still watched, and its sweep force-removes whatever lands
        // under it, so a start of that agent must not run a one-shot there
        // (`ensure` checks `being_swept`): the name is held from the timeout
        // on, before `run` returns.
        let name = format!("talaria-held-{}", std::process::id());
        let got = run("sleep", &["30".into()], &name, Duration::from_millis(200)).await;
        assert_eq!(got, Err(Failure::TimedOut));
        assert!(being_swept(&name), "the name is not held after the timeout");
        // The hold is the sweep task's: it ends with the task, panic or not.
        let probe = format!("talaria-held-probe-{}", std::process::id());
        let task = {
            let probe = probe.clone();
            tokio::spawn(async move {
                let _held = Sweeping::hold(&probe);
                panic!("a sweep that panics");
            })
        };
        assert!(task.await.is_err());
        assert!(!being_swept(&probe), "a panicked sweep kept its name");
        let held = Sweeping::hold(&probe);
        assert!(being_swept(&probe));
        drop(held);
        assert!(!being_swept(&probe));
    }

    #[tokio::test]
    async fn a_one_shot_that_exits_is_installed_or_says_why() {
        assert_eq!(
            run("true", &[], "nobody", Duration::from_secs(10)).await,
            Ok(())
        );
        let failed = run(
            "sh",
            &[
                "-c".into(),
                "echo '✗ agent-browser: download failed'; exit 1".into(),
            ],
            "nobody",
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(
            failed,
            Err(Failure::Failed("✗ agent-browser: download failed".into()))
        );
        // No docker at all is a failure with the reason, not a timeout.
        let missing = run(
            "/nonexistent/talaria-docker",
            &[],
            "nobody",
            Duration::from_secs(10),
        )
        .await;
        assert!(
            matches!(&missing, Err(Failure::Failed(e)) if e.contains("compose run")),
            "{missing:?}"
        );
    }
}
