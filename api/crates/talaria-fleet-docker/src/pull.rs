// THE IMAGE PULL — one per service, on a task of its own, with its progress.
//
// A new Hermes pin is about 1.1 GB to download and 4.5 GB on disk, and a
// host downloads it at whatever its link does: a fresh install's first
// agent, and every host's first start or roll after a pin bump. Measured
// with a rate-limited registry (2026-11-02), three things follow:
//
// - A pull has no honest wall-clock bound. It used to be killed at 1,200 s,
//   and before that it ran inside `compose up`'s 120 s, which a cold host's
//   first hire never survived. Now it is killed only when docker has said
//   nothing for [`PULL_SILENCE`] (a link that died, a daemon that hung), or
//   after [`PULL_CEILING`] whatever it says; never for being slow.
// - A pull is ONE per service in this process, and it outlives whoever
//   asked ([`join`]). `docker()` kills its CLI with the future that waits on
//   it, and a killed `compose pull` cancels the daemon's download, so a pull
//   inside a caller's future died with that caller: a hire step that ran out
//   its budget, a route whose client hung up. Here the hire's pull stage
//   waits a slice at a time and shows the progress; a start, a roll or a
//   re-entered hire step that asks meanwhile joins the download in flight
//   instead of starting a second one; and a caller that goes away leaves it
//   going for the next.
// - Progress is compose's own: `--progress json` streams an event per layer
//   update, bytes included ([`Tally`]), which the hire shows as its phase so
//   a long download never reads as a stall. Pinned against the real output
//   of compose v5.5.1 (this host) and v2.31.0 (the production image's
//   Alpine package), which differ in their fields.
//
// Per SERVICE, not per image: two agents hired at once run two `compose
// pull`s, and the daemon downloads each layer once for both (measured: 1,001
// MB through the registry for two hires of a 1 GB layer). And an END is not
// kept: a pull that finished is gone from the map, so the next asker starts a
// fresh `--policy missing` pull — a no-op once the image is there, a fresh
// try after a failure (which resumes what had arrived, on a containerd image
// store).

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::watch;
use tokio::time::Instant;

/// How long a pull may go without a word from docker before it is taken for
/// dead and killed. Docker reports every layer's bytes as they arrive, so
/// silence this long is a connection that stopped, not a slow one; the
/// unpack of a big layer on a busy disk is the longest quiet a healthy pull
/// has.
pub const PULL_SILENCE: Duration = Duration::from_secs(600);

/// The outside bound on one pull, however steadily it reports.
pub const PULL_CEILING: Duration = Duration::from_secs(3 * 60 * 60);

/// Where a pull stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullStatus {
    /// Still going: what it has brought down so far.
    Running(PullProgress),
    /// The image is on the host (it may have been there all along).
    Done,
    /// It failed: why, as `could not pull the image for <svc>: …`.
    Failed(String),
}

/// What a running pull has brought down so far, from compose's own events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullProgress {
    /// Bytes downloaded so far, across every layer that reported any.
    pub downloaded: u64,
    /// Layers that need nothing more: on the host already, or pulled.
    pub layers_ready: usize,
    /// Every layer compose has named so far.
    pub layers: usize,
    /// Every layer is downloaded and some are still being unpacked.
    pub unpacking: bool,
    /// When the pull started.
    pub started: Instant,
}

impl PullProgress {
    fn starting() -> PullProgress {
        PullProgress {
            downloaded: 0,
            layers_ready: 0,
            layers: 0,
            unpacking: false,
            started: Instant::now(),
        }
    }

    /// The pull in a waiting person's words — a hire's phase while it runs:
    /// `downloading the agent image — 412 MB so far, 9 of 37 layers ready
    /// (4 min)`. Bytes and layers are the daemon's own count; there is no
    /// percentage, because a layer's size is known only once it starts.
    pub fn phase(&self) -> String {
        // No layer reported yet: docker has not begun. Said as it is — on a
        // busy host the daemon took minutes to send its first request
        // (measured: none in 120 s, the manifest at 100 s, a layer 16 min
        // after the manifest while another pull unpacked), and "downloading"
        // with no bytes reads as a stall.
        let mut s = String::from(if self.layers == 0 {
            "waiting for docker to start downloading the agent image"
        } else if self.unpacking {
            "unpacking the agent image"
        } else {
            "downloading the agent image"
        });
        if self.downloaded > 0 {
            s.push_str(&format!(" — {} so far", human_bytes(self.downloaded)));
        }
        if self.layers > 0 {
            s.push_str(if self.downloaded > 0 { ", " } else { " — " });
            s.push_str(&format!(
                "{} of {} layers ready",
                self.layers_ready, self.layers
            ));
        }
        let mins = self.started.elapsed().as_secs() / 60;
        if mins >= 1 {
            s.push_str(&format!(" ({mins} min)"));
        }
        s
    }
}

/// `412 MB`, `1.1 GB` — decimal, as docker prints sizes.
fn human_bytes(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1} GB", n as f64 / 1e9)
    } else {
        format!("{} MB", n / 1_000_000)
    }
}

// ── Reading compose's progress ──────────────────────────────────────────────

/// One layer as compose's events have described it.
#[derive(Debug, Default, Clone)]
struct Layer {
    /// Bytes of it downloaded: the last `Downloading` count, then its size
    /// once the download completes.
    downloaded: u64,
    /// Its size, once a `Downloading` event said.
    size: u64,
    /// Downloaded (verified, complete, unpacking or pulled).
    fetched: bool,
    /// Nothing more to do: pulled, or on the host already.
    ready: bool,
}

/// The pull's layers, from `docker compose --progress json pull`'s events:
/// one JSON object per line, `{"id": <layer>, "parent_id": <service>,
/// "text": <docker's status>, "current": n, "total": n, …}`. The service's
/// own events (no `parent_id`) and any line that is not such an object are
/// not layers.
#[derive(Debug, Default)]
pub(crate) struct Tally {
    layers: BTreeMap<String, Layer>,
}

impl Tally {
    /// Read one line. Answers whether it was a layer's event.
    pub(crate) fn see(&mut self, line: &str) -> bool {
        let Ok(serde_json::Value::Object(ev)) = serde_json::from_str::<serde_json::Value>(line)
        else {
            return false;
        };
        let text = |k: &str| ev.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let num = |k: &str| ev.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let (id, parent) = (text("id"), text("parent_id"));
        if id.is_empty() || parent.is_empty() {
            return false;
        }
        let layer = self.layers.entry(id.to_string()).or_default();
        match text("text") {
            "Downloading" => {
                layer.downloaded = layer.downloaded.max(num("current"));
                layer.size = layer.size.max(num("total"));
            }
            "Verifying Checksum" | "Download complete" | "Extracting" => {
                layer.fetched = true;
                layer.downloaded = layer.downloaded.max(layer.size);
            }
            "Pull complete" => {
                layer.fetched = true;
                layer.ready = true;
                layer.downloaded = layer.downloaded.max(layer.size);
            }
            "Already exists" => {
                layer.fetched = true;
                layer.ready = true;
            }
            _ => {}
        }
        true
    }

    pub(crate) fn progress(&self, started: Instant) -> PullProgress {
        let layers = self.layers.len();
        let fetched = self.layers.values().filter(|l| l.fetched).count();
        let layers_ready = self.layers.values().filter(|l| l.ready).count();
        PullProgress {
            downloaded: self.layers.values().map(|l| l.downloaded).sum(),
            layers_ready,
            layers,
            unpacking: layers > 0 && fetched == layers && layers_ready < layers,
            started,
        }
    }
}

// ── One pull per service ────────────────────────────────────────────────────

/// The pull of each service's image in flight in this process, or the last
/// one's end. Only a `Running` entry is joined.
static PULLS: LazyLock<Mutex<HashMap<String, watch::Receiver<PullStatus>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// What one pull runs, and how long it may go quiet or last. The real one is
/// compose's ([`PullCmd::compose`]); a test's is a script.
pub(crate) struct PullCmd {
    program: String,
    args: Vec<String>,
    silence: Duration,
    ceiling: Duration,
    /// The quiet pull to fall back to on a compose too old for
    /// `--progress json` or `--policy`; none for a test's script.
    fallback: Option<Vec<String>>,
}

impl PullCmd {
    fn compose(svc: &str) -> PullCmd {
        PullCmd {
            program: "docker".into(),
            args: super::compose_args(&["--progress", "json", "pull", "--policy", "missing", svc]),
            silence: PULL_SILENCE,
            ceiling: PULL_CEILING,
            fallback: Some(super::compose_args(&[
                "pull", "--policy", "missing", "--quiet", svc,
            ])),
        }
    }

    /// The same pull for a service of another compose project: `project`
    /// is the `compose -p … -f … --env-file …` prefix that addresses it.
    fn project(project: &[String], svc: &str) -> PullCmd {
        let with = |extra: &[&str]| {
            let mut args = project.to_vec();
            args.extend(extra.iter().map(|s| s.to_string()));
            args
        };
        PullCmd {
            program: "docker".into(),
            args: with(&["--progress", "json", "pull", "--policy", "missing", svc]),
            silence: PULL_SILENCE,
            ceiling: PULL_CEILING,
            fallback: Some(with(&["pull", "--policy", "missing", "--quiet", svc])),
        }
    }
}

/// Join the pull of `svc`'s image in flight in this process, or start one on
/// a task of its own.
pub(crate) fn join(svc: &str) -> watch::Receiver<PullStatus> {
    join_with(svc, || PullCmd::compose(svc))
}

/// [`join`] for a service of another compose project (the Hermes image
/// canary's), keyed by `key` so it is one pull however often it is asked.
pub(crate) fn join_project(
    key: &str,
    project: &[String],
    svc: &str,
) -> watch::Receiver<PullStatus> {
    join_with(key, || PullCmd::project(project, svc))
}

/// How many pulls are running in this process now — the agents' own and any
/// other. A background pull that should give way (the channel's
/// preparation of a new image) waits while this is above zero.
pub(crate) fn running() -> usize {
    let pulls = PULLS.lock().unwrap_or_else(|p| p.into_inner());
    pulls
        .values()
        .filter(|rx| matches!(*rx.borrow(), PullStatus::Running(_)) && rx.has_changed().is_ok())
        .count()
}

/// [`join`] for any command, keyed by `key`.
fn join_with(key: &str, cmd: impl FnOnce() -> PullCmd) -> watch::Receiver<PullStatus> {
    let mut pulls = PULLS.lock().unwrap_or_else(|p| p.into_inner());
    // Still running, and its task still holds the sender: a task that died
    // without an answer leaves `Running` behind, which is not a pull to join.
    if let Some(rx) = pulls.get(key)
        && matches!(*rx.borrow(), PullStatus::Running(_))
        && rx.has_changed().is_ok()
    {
        return rx.clone();
    }
    let (tx, rx) = watch::channel(PullStatus::Running(PullProgress::starting()));
    pulls.insert(key.to_string(), rx.clone());
    let key = key.to_string();
    let cmd = cmd();
    tokio::spawn(async move {
        let end = match run(&key, cmd, &tx).await {
            Ok(()) => PullStatus::Done,
            Err(e) => {
                tracing::warn!("[fleet] {e}");
                PullStatus::Failed(e)
            }
        };
        tx.send_replace(end);
        let mut pulls = PULLS.lock().unwrap_or_else(|p| p.into_inner());
        if pulls
            .get(&key)
            .is_some_and(|rx| rx.same_channel(&tx.subscribe()))
        {
            pulls.remove(&key);
        }
    });
    rx
}

/// Wait for a joined pull's end — or, with `within`, at most that long, and
/// answer where it stands then.
pub(crate) async fn wait(
    mut rx: watch::Receiver<PullStatus>,
    within: Option<Duration>,
) -> PullStatus {
    let gone = || {
        PullStatus::Failed("could not pull the image: the pull stopped without an answer".into())
    };
    let settled = {
        let ended = rx.wait_for(|s| !matches!(s, PullStatus::Running(_)));
        match within {
            None => Some(ended.await.map(|s| s.clone())),
            Some(d) => tokio::time::timeout(d, ended)
                .await
                .ok()
                .map(|r| r.map(|s| s.clone())),
        }
    };
    match settled {
        Some(Ok(end)) => end,
        Some(Err(_)) => gone(),
        None => rx.borrow().clone(),
    }
}

/// Feed one stream's lines into `tx` — lossily, so a byte that is not UTF-8
/// costs that line, not the rest of the stream (a reader that stopped would
/// leave the pipe undrained and the CLI blocked on it).
fn feed(
    stream: impl AsyncRead + Send + Unpin + 'static,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// What ended one wait of the read loop.
enum Heard {
    Line(String),
    Closed,
    Exited(std::io::Result<std::process::ExitStatus>),
    Quiet,
}

/// The pull itself: `compose pull --policy missing` for the service (or a
/// test's command), with its json progress read line by line into the
/// channel. `label` names it in the error.
async fn run(label: &str, cmd: PullCmd, tx: &watch::Sender<PullStatus>) -> Result<(), String> {
    let fail = |why: String| format!("could not pull the image for {label}: {why}");
    let mut child = tokio::process::Command::new(&cmd.program)
        .args(&cmd.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| fail(format!("docker compose …: {e}")))?;
    // Both streams, merged into one line feed: compose's events and its
    // errors do not agree on which one they use (v5.5.1 and v2.31.0 both
    // write the json to stderr).
    let (lines_tx, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        feed(out, lines_tx.clone());
    }
    if let Some(err) = child.stderr.take() {
        feed(err, lines_tx.clone());
    }
    drop(lines_tx);

    let started = match &*tx.borrow() {
        PullStatus::Running(p) => p.started,
        _ => Instant::now(),
    };
    let mut tally = Tally::default();
    // What compose said that was not progress — its error, when it fails.
    let mut said: Vec<String> = Vec::new();
    let hear = |line: String, tally: &mut Tally, said: &mut Vec<String>| {
        if tally.see(&line) {
            tx.send_replace(PullStatus::Running(tally.progress(started)));
        } else if let Some(text) = said_of(&line) {
            if said.len() == 8 {
                said.remove(0);
            }
            said.push(text);
        }
    };
    let status = loop {
        let left = cmd.ceiling.saturating_sub(started.elapsed());
        let by_ceiling = left < cmd.silence;
        let heard = tokio::select! {
            line = lines.recv() => match line {
                Some(line) => Heard::Line(line),
                None => Heard::Closed,
            },
            status = child.wait() => Heard::Exited(status),
            _ = tokio::time::sleep(cmd.silence.min(left)) => Heard::Quiet,
        };
        match heard {
            Heard::Line(line) => hear(line, &mut tally, &mut said),
            // Both pipes closed: the CLI is ending — wait for it, bounded.
            Heard::Closed => {
                break tokio::time::timeout(Duration::from_secs(30), child.wait())
                    .await
                    .unwrap_or_else(|_| {
                        Err(std::io::Error::other(
                            "did not exit after closing its output",
                        ))
                    });
            }
            // It exited: what it already wrote is still worth reading.
            Heard::Exited(status) => {
                while let Ok(Some(line)) =
                    tokio::time::timeout(Duration::from_secs(2), lines.recv()).await
                {
                    hear(line, &mut tally, &mut said);
                }
                break status;
            }
            Heard::Quiet => {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
                let silence_min = cmd.silence.as_secs() / 60;
                return Err(fail(if by_ceiling {
                    format!(
                        "it ran past its {}-min ceiling and was stopped — {}",
                        cmd.ceiling.as_secs() / 60,
                        tally.progress(started).phase()
                    )
                } else if tally.layers.is_empty() {
                    format!(
                        "docker never started the download (nothing from docker for {silence_min} min)"
                    )
                } else {
                    format!(
                        "the download stopped making progress (nothing from docker for {silence_min} min) — {}",
                        tally.progress(started).phase()
                    )
                }));
            }
        }
    };
    let status = status.map_err(|e| fail(format!("docker compose …: {e}")))?;
    if status.success() {
        return Ok(());
    }
    let why = said.join(" ");
    // A compose too old for `--policy` (before v2.22) or for `--progress json`
    // (before v2.29, which says `unsupported --progress value "json"`): the
    // quiet pull this replaced, under its old deadline — and with no
    // `--policy` at all, `up` pulls a missing image itself, just without the
    // early, clean failure.
    if let Some(quiet) = cmd.fallback.as_ref().filter(|_| is_too_old(&why)) {
        let refs: Vec<&str> = quiet.iter().map(String::as_str).collect();
        return match super::docker(&refs, Duration::from_secs(1_200)).await {
            Ok(_) => Ok(()),
            Err(e) if e.contains("unknown flag") => Ok(()),
            Err(e) => Err(fail(e)),
        };
    }
    Err(fail(format!(
        "docker compose failed: {}",
        if why.is_empty() {
            status.to_string()
        } else {
            why
        }
    )))
}

/// Whether compose refused the pull's flags rather than the pull: too old for
/// `--policy` or for the `json` progress mode.
fn is_too_old(said: &str) -> bool {
    said.contains("unknown flag")
        || said.contains("unsupported --progress")
        || (said.contains("progress") && said.contains("invalid"))
}

/// The words in a line compose printed that was not a layer's progress: a
/// json event's own text and error, or the plain line.
fn said_of(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(serde_json::Value::Object(ev)) => {
            let field = |k: &str| ev.get(k).and_then(|v| v.as_str()).unwrap_or("");
            // v5.5.1 marks a failed resource `status: "Error"`; v2.31.0 marks
            // it `text: "Error"` and puts the reason in `status`; both end
            // with `{"error": true, "message": …}`.
            let errored = ev.get("error").and_then(|v| v.as_bool()).unwrap_or(false)
                || field("status") == "Error"
                || field("text") == "Error";
            if !errored {
                return None;
            }
            let text = ["message", "details", "status", "text"]
                .iter()
                .map(|k| field(k))
                .find(|s| !s.is_empty() && *s != "Error")
                .unwrap_or("error");
            Some(text.to_string())
        }
        _ => Some(line.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, text: &str, current: u64, total: u64) -> String {
        serde_json::json!({
            "id": id, "parent_id": "agent-ops", "text": text,
            "current": current, "total": total,
        })
        .to_string()
    }

    #[test]
    fn a_pull_s_progress_is_the_layers_compose_reported() {
        let start = Instant::now();
        let mut t = Tally::default();
        // The service's own line and anything that is not an event are not
        // layers.
        assert!(!t.see(r#"{"id":"agent-ops","text":"Pulling"}"#));
        assert!(!t.see("Error response from daemon: x"));
        assert!(t.see(&ev("aa", "Already exists", 0, 0)));
        assert!(t.see(&ev("bb", "Pulling fs layer", 0, 0)));
        assert!(t.see(&ev("cc", "Waiting", 0, 0)));
        assert!(t.see(&ev("bb", "Downloading", 412_000_000, 1_000_000_000)));
        let p = t.progress(start);
        assert_eq!(
            (p.downloaded, p.layers_ready, p.layers, p.unpacking),
            (412_000_000, 1, 3, false)
        );
        assert_eq!(
            p.phase(),
            "downloading the agent image — 412 MB so far, 1 of 3 layers ready"
        );

        // A count never goes back; a finished download counts its size.
        t.see(&ev("bb", "Downloading", 400_000_000, 1_000_000_000));
        t.see(&ev("bb", "Download complete", 0, 0));
        t.see(&ev("cc", "Downloading", 5_000_000, 20_000_000));
        t.see(&ev("cc", "Download complete", 0, 0));
        let p = t.progress(start);
        assert_eq!(p.downloaded, 1_020_000_000);
        assert!(p.unpacking, "every layer is down and two are not ready");
        assert_eq!(
            p.phase(),
            "unpacking the agent image — 1.0 GB so far, 1 of 3 layers ready"
        );

        t.see(&ev("bb", "Pull complete", 0, 0));
        t.see(&ev("cc", "Pull complete", 0, 0));
        let p = t.progress(start);
        assert_eq!((p.layers_ready, p.unpacking), (3, false));
    }

    #[test]
    fn compose_s_own_json_events_read_as_layers() {
        // Verbatim from `docker compose --progress json pull` (compose
        // v5.5.1, docker 29.8.1, containerd image store), on stderr: the
        // image's own lines have no parent_id; `Extracting` counts seconds,
        // not bytes.
        let lines = [
            r#"{"id":"Image localhost:15512/talaria-hire-test/hermes:cold","status":"Working","text":"Pulling"}"#,
            r#"{"id":"536374a1eee3","parent_id":"Image localhost:15512/talaria-hire-test/hermes:cold","status":"Working","text":"Pulling fs layer","details":"0B"}"#,
            r#"{"id":"536374a1eee3","parent_id":"Image localhost:15512/talaria-hire-test/hermes:cold","status":"Working","text":"Downloading","details":"16.78MB","current":16777216,"total":1000305427,"percent":1}"#,
        ];
        let start = Instant::now();
        let mut t = Tally::default();
        let seen: Vec<bool> = lines.iter().map(|l| t.see(l)).collect();
        assert_eq!(seen, vec![false, true, true]);
        assert_eq!(t.progress(start).downloaded, 16_777_216);
        for l in [
            r#"{"id":"536374a1eee3","parent_id":"Image localhost:15512/talaria-hire-test/hermes:cold","status":"Done","text":"Download complete","details":"0B","percent":100}"#,
            r#"{"id":"536374a1eee3","parent_id":"Image localhost:15512/talaria-hire-test/hermes:cold","status":"Working","text":"Extracting","details":"21B","current":21}"#,
        ] {
            assert!(t.see(l));
        }
        let p = t.progress(start);
        assert_eq!((p.downloaded, p.unpacking), (1_000_305_427, true));
        assert!(t.see(r#"{"id":"536374a1eee3","parent_id":"Image localhost:15512/talaria-hire-test/hermes:cold","status":"Done","text":"Pull complete","details":"0B","percent":100}"#));
        assert!(!t.see(r#"{"id":"Image localhost:15512/talaria-hire-test/hermes:cold","status":"Done","text":"Pulled"}"#));
        let p = t.progress(start);
        assert_eq!((p.layers_ready, p.layers, p.unpacking), (1, 1, false));
        // None of it is an error to keep.
        assert!(lines.iter().all(|l| said_of(l).is_none()));
    }

    #[test]
    fn a_pull_that_has_said_nothing_yet_still_says_what_it_is_doing() {
        assert_eq!(
            PullProgress::starting().phase(),
            "waiting for docker to start downloading the agent image"
        );
        let mut p = PullProgress::starting();
        p.layers = 37;
        p.layers_ready = 36;
        assert_eq!(
            p.phase(),
            "downloading the agent image — 36 of 37 layers ready"
        );
    }

    #[test]
    fn what_compose_said_that_was_not_progress_is_kept_for_the_failure() {
        assert_eq!(said_of(&ev("bb", "Downloading", 1, 2)), None);
        assert_eq!(said_of("  "), None);
        assert_eq!(
            said_of("Error response from daemon: toomanyrequests: You have reached your pull rate limit."),
            Some("Error response from daemon: toomanyrequests: You have reached your pull rate limit.".into())
        );
        assert_eq!(
            said_of(
                r#"{"id":"agent-ops","status":"Error","text":"Error","details":"manifest unknown"}"#
            ),
            Some("manifest unknown".into())
        );
    }
    #[test]
    fn compose_2_31_s_json_events_read_as_layers_and_its_error_is_kept() {
        // Verbatim from compose v2.31.0 — the production api image's Alpine
        // 3.21 `docker-cli-compose` — pulling through docker 29.8.1: a
        // layer's parent is the SERVICE, `status` carries the bar text, and a
        // failed service says `text: "Error"` with the reason in `status`.
        let start = Instant::now();
        let mut t = Tally::default();
        for (line, layer) in [
            (r#"{"id":"agent-ok","text":"Pulling"}"#, false),
            (
                r#"{"id":"034d6572bf28","parent_id":"agent-ok","text":"Pulling fs layer"}"#,
                true,
            ),
            (
                r#"{"id":"034d6572bf28","parent_id":"agent-ok","text":"Downloading","status":"[=======================>                           ]  1.049MB/2.206MB","current":1048576,"total":2206402,"percent":47}"#,
                true,
            ),
        ] {
            assert_eq!(t.see(line), layer, "{line}");
            assert_eq!(said_of(line), None, "{line}");
        }
        assert_eq!(t.progress(start).downloaded, 1_048_576);
        for line in [
            r#"{"id":"034d6572bf28","parent_id":"agent-ok","text":"Download complete","percent":100}"#,
            r#"{"id":"034d6572bf28","parent_id":"agent-ok","text":"Extracting","status":"1 s","current":1}"#,
        ] {
            assert!(t.see(line));
        }
        assert!(t.progress(start).unpacking);
        assert!(t.see(
            r#"{"id":"034d6572bf28","parent_id":"agent-ok","text":"Pull complete","percent":100}"#
        ));
        assert!(!t.see(r#"{"id":"agent-ok","text":"Pulled"}"#));
        let p = t.progress(start);
        assert_eq!((p.downloaded, p.layers_ready, p.layers), (2_206_402, 1, 1));

        let service_error = r#"{"id":"agent-bad","text":"Error","status":"failed to resolve reference \"docker.io/library/busybox:talaria-no-such-tag\": docker.io/library/busybox:talaria-no-such-tag: not found"}"#;
        let final_error = r#"{"error":true,"message":"Error response from daemon: failed to resolve reference \"docker.io/library/busybox:talaria-no-such-tag\": docker.io/library/busybox:talaria-no-such-tag: not found"}"#;
        assert!(!t.see(service_error) && !t.see(final_error));
        assert_eq!(
            said_of(service_error).as_deref(),
            Some(
                "failed to resolve reference \"docker.io/library/busybox:talaria-no-such-tag\": docker.io/library/busybox:talaria-no-such-tag: not found"
            )
        );
        assert!(
            said_of(final_error)
                .unwrap()
                .starts_with("Error response from daemon: failed to resolve")
        );
    }

    #[test]
    fn a_compose_too_old_for_the_pull_s_flags_is_told_from_a_failed_pull() {
        assert!(is_too_old("unknown flag: --policy"));
        assert!(is_too_old("unsupported --progress value \"json\""));
        assert!(is_too_old("invalid progress mode json"));
        assert!(!is_too_old("Error response from daemon: manifest unknown"));
        assert!(!is_too_old(""));
    }

    // ── The shared pull, driven by a script in place of compose ─────────────

    fn scratch(name: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("talaria-pull-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn sh(script: String, silence: Duration) -> PullCmd {
        PullCmd {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
            silence,
            ceiling: Duration::from_secs(60),
            fallback: None,
        }
    }

    fn runs(file: &std::path::Path) -> usize {
        std::fs::read_to_string(file)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    const LAYER: &str =
        r#"{"id":"aa","parent_id":"svc","text":"Downloading","current":5000000,"total":10000000}"#;

    #[tokio::test]
    async fn askers_share_one_pull_and_one_who_comes_after_its_end_starts_another() {
        let count = scratch("share");
        let script = format!(
            "echo run >> {c}; echo '{LAYER}' >&2; sleep 1; echo '{{\"id\":\"aa\",\"parent_id\":\"svc\",\"text\":\"Pull complete\"}}' >&2",
            c = count.display()
        );
        let key = "test-share";
        let a = join_with(key, || sh(script.clone(), Duration::from_secs(30)));
        let b = join_with(key, || panic!("a second asker must join, not start a pull"));
        assert!(a.same_channel(&b));
        assert_eq!(wait(a, None).await, PullStatus::Done);
        assert_eq!(wait(b, None).await, PullStatus::Done);
        assert_eq!(runs(&count), 1, "one pull for both");
        // The end is not kept: the next asker starts a fresh pull.
        let c = join_with(key, || sh(script.clone(), Duration::from_secs(30)));
        assert_eq!(wait(c, None).await, PullStatus::Done);
        assert_eq!(runs(&count), 2);
        let _ = std::fs::remove_file(&count);
    }

    #[tokio::test]
    async fn a_caller_that_stops_waiting_leaves_the_pull_going() {
        let done = scratch("drop");
        let script = format!(
            "echo '{LAYER}' >&2; sleep 2; echo done > {d}",
            d = done.display()
        );
        let key = "test-drop";
        let rx = join_with(key, || sh(script, Duration::from_secs(30)));
        // A slice that ends mid-pull answers what has arrived so far…
        let early = wait(rx.clone(), Some(Duration::from_millis(1_200))).await;
        let PullStatus::Running(p) = early else {
            panic!("still running after 1.2 s: {early:?}")
        };
        assert_eq!(p.downloaded, 5_000_000);
        // …and a caller whose wait is dropped outright cancels nothing.
        let dropped = tokio::spawn(wait(rx.clone(), None));
        dropped.abort();
        drop(rx);
        let again = join_with(key, || panic!("still running: must be joined"));
        assert_eq!(wait(again, None).await, PullStatus::Done);
        assert_eq!(std::fs::read_to_string(&done).unwrap().trim(), "done");
        let _ = std::fs::remove_file(&done);
    }

    #[tokio::test]
    async fn a_failed_pull_answers_compose_s_own_reason() {
        let script = r#"echo '{"id":"svc","text":"Error","status":"manifest unknown"}' >&2; echo '{"error":true,"message":"Error response from daemon: manifest unknown"}' >&2; exit 18"#;
        let end = wait(
            join_with("test-fail", || sh(script.into(), Duration::from_secs(30))),
            None,
        )
        .await;
        let PullStatus::Failed(e) = end else {
            panic!("expected a failure: {end:?}")
        };
        assert!(
            e.starts_with(
                "could not pull the image for test-fail: docker compose failed: manifest unknown"
            ),
            "{e}"
        );
        assert!(
            e.contains("Error response from daemon: manifest unknown"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn a_pull_docker_goes_quiet_on_is_stopped_and_says_which_quiet_it_was() {
        let never = wait(
            join_with("test-never", || {
                sh("sleep 30".into(), Duration::from_millis(500))
            }),
            None,
        )
        .await;
        assert!(
            matches!(&never, PullStatus::Failed(e) if e.contains("docker never started the download")),
            "{never:?}"
        );
        let stalled = wait(
            join_with("test-stalled", || {
                sh(
                    format!("echo '{LAYER}' >&2; sleep 30"),
                    Duration::from_millis(500),
                )
            }),
            None,
        )
        .await;
        assert!(
            matches!(&stalled, PullStatus::Failed(e) if e.contains("the download stopped making progress") && e.contains("5 MB so far")),
            "{stalled:?}"
        );
    }

    #[tokio::test]
    async fn a_pull_whose_task_died_without_an_answer_is_not_joined() {
        let key = "test-dead";
        let (tx, rx) = watch::channel(PullStatus::Running(PullProgress::starting()));
        drop(tx);
        PULLS.lock().unwrap().insert(key.to_string(), rx.clone());
        // Its waiter hears the end it never got…
        assert!(
            matches!(wait(rx.clone(), None).await, PullStatus::Failed(e) if e.contains("without an answer"))
        );
        // …and the next asker starts a pull of its own.
        let fresh = join_with(key, || sh("true".into(), Duration::from_secs(30)));
        assert!(!fresh.same_channel(&rx));
        assert_eq!(wait(fresh, None).await, PullStatus::Done);
    }
}
