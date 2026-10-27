// prepare_env: a job's dev environment, set up by the platform.
//
// The agent runs as an unprivileged user in a stock image: no sudo, no
// toolchain manager, only what the image happened to ship. Left to itself it
// spends turns hand-installing rustup and linkers into its home and prefixing
// every command with PATH exports (the 2026-09-24 dogfood agent did exactly
// that for mold). This verb does it once, from what the repo declares:
//
//   · toolchains through mise, in user space on the persistent volume (so a
//     department's agents share one download of each version). A repo's own
//     mise.toml / .tool-versions is used as is; a repo without one gets its
//     tools detected from the files every ecosystem already has
//     (rust-toolchain.toml, .nvmrc, package.json, go.mod, ...), written to a
//     mise.local.toml that .git/info/exclude keeps out of commits.
//   · OS packages through apt, as root, only for names the repo declares in
//     .talaria/workbench.toml and only from the image's own Debian sources.
//     The agent itself never gets root — and root never reads the repo: the
//     scan that finds those names runs as the agent's own user, so a marker
//     file is read with exactly the rights the agent already has (a
//     symlinked `.talaria/workbench.toml` cannot borrow root's to read
//     elsewhere; the scan refuses symlinks and anything outside the checkout
//     outright).
//   · the project's env store (the repo's `repo_env` rows), unsealed into
//     THIS job's workdir as `.talaria/env` (0600, the agent's own) here,
//     once the checkout exists, and removed when the job ends. One job
//     sees one repo's env; nothing is mounted fleet-wide.
//   · mise's shims on PATH for every login shell, so `cargo` / `bun` / `go`
//     resolve to the repo's pinned versions by directory, with no activation
//     step the agent can forget.
//   · for a Talaria app (a talaria.json at the repo's root — any repo, never
//     keyed on Talaria's own), the SDK kit: fetched from this instance as the
//     agent, installed once per kit version beside the harness runtime, and
//     `talaria-app` linked onto the PATH every shell has, so the agent can
//     typecheck, build and eval the app here, in its sandbox, with the
//     host's own recipe. The host never loads unmerged app code.
//   · for Rust repos, sccache as cargo's compiler wrapper with one cache per
//     department, so a job's cold `target/` still gets every dependency
//     another job already compiled instead of rebuilding the whole graph.
//     Scoped to the repo through mise.local.toml's [env], never global: the
//     wrapper is a path mise resolves per directory, and cargo outside a repo
//     that installed it would fail to start rustc.
//
// .talaria/workbench.toml (all optional):
//
//     apt = ["libssl-dev", "protobuf-compiler"]
//     sccache = false      # opt a Rust repo out of the shared compile cache
//     [tools]              # extra mise tools, merged over detection
//     protoc = "28"
//     [cleanup]            # what a reclaim may drop from an idle job
//     artifacts = ["target", ".turbo"]   # merges over detection; [] opts out
//     keep = [".venv"]                   # never dropped, whatever the pressure
//     maxWorkdirGib = 40                 # this repo's ceiling for one job

use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::OnceLock;

use regex::Regex;
use talaria_fleet_docker::{docker_exec, docker_exec_as, docker_exec_opt, managed_container};

/// The sandbox's unprivileged user — the one the image's init drops to and
/// the one every job's files belong to. The scan and the env write run as it.
pub const AGENT_USER: &str = "hermes";

/// Files the scan reads, relative to the repo root. `*/` entries are one
/// level down, which is where monorepos keep their per-language roots
/// (Talaria's own rust-toolchain.toml lives in api/).
const MARKERS: &[&str] = &[
    // A Talaria app: the SDK kit (`install_app_kit`). Root only — the
    // manifest sits at the root of every app.
    "talaria.json",
    "mise.toml",
    ".mise.toml",
    ".tool-versions",
    ".talaria/workbench.toml",
    "rust-toolchain.toml",
    "rust-toolchain",
    "Cargo.toml",
    ".cargo/config.toml",
    ".nvmrc",
    ".node-version",
    "package.json",
    "bun.lock",
    "bun.lockb",
    "pnpm-lock.yaml",
    "go.mod",
    ".python-version",
    "pyproject.toml",
    "requirements.txt",
    "uv.lock",
    ".ruby-version",
    // Read for cleanup detection rather than for toolchains: these name an
    // ecosystem whose build output is rebuildable (see `plan_cleanup`).
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "mix.exs",
    "*.sln",
    "*.csproj",
    "*/rust-toolchain.toml",
    "*/rust-toolchain",
    "*/Cargo.toml",
    "*/.cargo/config.toml",
    "*/.nvmrc",
    "*/package.json",
    "*/bun.lock",
    "*/go.mod",
    "*/.python-version",
    "*/pyproject.toml",
];

/// Finds the repo root (the workdir itself, or the one directory under it
/// that holds .git, depending on how the agent cloned), then dumps each
/// marker file as `\x1e<path>\n<first 8 KiB>`. $1 = workdir, $2.. = markers.
///
/// It runs as the agent's user, never root, and it reads a marker only when
/// it is a plain file whose real path is inside the checkout: a symlink —
/// the file itself, a directory on its path, or the checkout dir the glob
/// found — is reported as `SKIP <path>` in the header and never opened.
/// The agent controls every byte under the workdir, so the scan can only
/// ever be allowed to read what the agent could already read itself.
const SCAN_SH: &str = r#"
wd="$1"; shift
root=""
if [ -d "$wd/.git" ]; then root="$wd"; else
  for d in "$wd"/*/; do
    d="${d%/}"
    [ -L "$d" ] && continue
    if [ -d "$d/.git" ]; then root="$d"; break; fi
  done
fi
if [ -z "$root" ]; then echo NOREPO; exit 0; fi
printf 'ROOT %s\n' "$root"
printf 'OWNER %s\n' "$(stat -c %U "$root")"
cd "$root" || exit 1
real=$(readlink -f -- "$root") || exit 1
# 0 = a plain file inside the checkout; 1 = a symlink or an escape (reported);
# 2 = not a file at all (a directory, an unmatched glob: silently nothing).
inside() {
  [ -L "$1" ] && return 1
  [ -f "$1" ] || return 2
  rp=$(readlink -f -- "$1") || return 1
  case "$rp" in "$real"/*) return 0 ;; esac
  return 1
}
for pat in "$@"; do
  for f in $pat; do
    inside "$f"
    [ $? -eq 1 ] && printf 'SKIP %s\n' "$f"
  done
done
for pat in "$@"; do
  for f in $pat; do
    inside "$f" || continue
    printf '\036%s\n' "$f"
    head -c 8192 "$f"
    printf '\n'
  done
done
"#;

/// Root half: OS packages, and nothing else. Only names the repo declared
/// and plan_env validated reach it, and a repo that declares none never
/// runs it. It is the one step that runs as root, and it runs only AFTER the
/// scan (as the agent) has read the file and `valid_apt` has passed every
/// name: root touches the package list, never the repo. $1.. = apt packages.
const ROOT_SH: &str = r#"
set -e
missing=""
for p in "$@"; do
  dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q "ok installed" || missing="$missing $p"
done
if [ -n "$missing" ]; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  # shellcheck disable=SC2086
  apt-get install -y -qq --no-install-recommends $missing
fi
"#;

/// `HOME` as Hermes sets it for the commands its terminal runs in a container
/// (`$HERMES_HOME/home`, its `TERMINAL_HOME_MODE=auto` in a container). It is
/// not the agent user's passwd home (`/opt/data`), which is the gateway's own.
pub const AGENT_TERMINAL_HOME: &str = "/opt/data/home";

/// User half, as the checkout's owner: the mise binary, the shims line in
/// the shell profiles, the generated config kept out of git, every tool the
/// repo's mise configs name, sccache wired into the repo's env when asked,
/// and what resolved. $1 = home, $2 = repo root, $3 = "1" when a
/// mise.local.toml was written, $4 = the sccache dir ("" = no sccache),
/// $5 = the agent terminal's HOME ([`AGENT_TERMINAL_HOME`]).
const USER_SH: &str = r#"
set -e
home="$1"; root="$2"; wrote="$3"; sccache_dir="$4"; term_home="$5"
export HOME="$home" MISE_YES=1 MISE_TRUSTED_CONFIG_PATHS=/opt/data/workbench/jobs
bin="$home/.local/bin"
export PATH="$bin:$PATH"
if ! command -v mise >/dev/null 2>&1; then
  mkdir -p "$bin"
  curl -fsSL https://mise.run | MISE_INSTALL_PATH="$bin/mise" sh >/dev/null
  command -v mise >/dev/null 2>&1 || { echo "could not download mise (https://mise.run)" >&2; exit 1; }
fi
# Where this install keeps its tools and its global config, resolved NOW, as
# mise resolves them for this user: MISE_DATA_DIR, else XDG_DATA_HOME/mise
# (the render sets XDG_DATA_HOME on workbenches), else ~/.local/share/mise.
# The block below carries them as absolute paths because the shell that reads
# it may have another HOME: Hermes runs the agent's terminal commands with
# HOME=$term_home, where a ~-relative path names an empty directory and a
# global pin (bun, below) is not found.
data="${MISE_DATA_DIR:-${XDG_DATA_HOME:-$home/.local/share}/mise}"
global="${MISE_GLOBAL_CONFIG_FILE:-${MISE_CONFIG_DIR:-${XDG_CONFIG_HOME:-$home/.config}/mise}/config.toml}"
case "$data$global" in *"'"*) echo "mise paths with a quote in them: $data $global" >&2; exit 1 ;; esac
# Rewritten on every call (old block out, current block in), so a fix to the
# block reaches containers that already have an older one. Written back in
# place (not replaced) so the file keeps its mode. Both homes get it: the
# passwd home's profile is what Hermes's terminal sources when it snapshots a
# new session, and the terminal home's is what `. ~/.profile` reads from a
# shell the agent opened before this call (and what a login shell there reads).
rcs="$home/.profile $home/.bashrc"
if [ -n "$term_home" ] && [ "$term_home" != "$home" ] && mkdir -p "$term_home" 2>/dev/null; then
  rcs="$rcs $term_home/.profile $term_home/.bashrc"
fi
for rc in $rcs; do
  touch "$rc"
  tmp=$(mktemp)
  awk '/^# >>> talaria workbench: mise >>>$/{skip=1} !skip{print} /^# <<< talaria workbench: mise <<<$/{skip=0}' "$rc" > "$tmp"
  cat >> "$tmp" <<EOF
# >>> talaria workbench: mise >>>
export MISE_YES=1
export MISE_TRUSTED_CONFIG_PATHS=/opt/data/workbench/jobs
export MISE_DATA_DIR='$data'
export MISE_GLOBAL_CONFIG_FILE='$global'
# Always to the FRONT, even if already present: a stock .profile puts
# ~/.local/bin ahead after .bashrc ran, and a hand-installed tool there must
# not shadow the pin.
export PATH="\$MISE_DATA_DIR/shims:\$PATH"
# <<< talaria workbench: mise <<<
EOF
  cat "$tmp" > "$rc"
  rm -f "$tmp"
done
cd "$root"
if [ "$wrote" = 1 ] && ! grep -qx "mise.local.toml" .git/info/exclude 2>/dev/null; then
  mkdir -p .git/info
  echo "mise.local.toml" >> .git/info/exclude
fi
log=$(mktemp)
if ! mise install >"$log" 2>&1; then
  tail -n 40 "$log" >&2
  rm -f "$log"
  exit 1
fi
rm -f "$log"
# The wrapper is sccache's absolute install path, not its shim: cargo runs
# rustc for registry crates from their own source dirs, outside the repo,
# where a shim finds no version to run.
if [ -n "$sccache_dir" ]; then
  wrapper=$(mise which sccache)
  mise set --file mise.local.toml RUSTC_WRAPPER="$wrapper" SCCACHE_DIR="$sccache_dir" SCCACHE_CACHE_SIZE=20G
fi
# The coding harness is a bun program (omp's bin is `#!/usr/bin/env bun`),
# and `bun` on PATH is a mise SHIM — which resolves only where a mise config
# pins bun. This repo may pin rust and mold and no bun at all, and doctor
# probes the harness from the agent's home rather than from here, so the pin
# has to be GLOBAL or omp dies before it starts wherever it is run.
mise use -g bun@1.4.0 >/dev/null 2>&1 || true
echo "=== resolved"
mise ls --current 2>&1
"#;

/// One compile cache per department, on the shared harness volume beside the
/// npm cache and Playwright browsers, so every agent's jobs feed one cache.
pub const SCCACHE_DIR: &str = "/opt/data/workbench/harness/sccache";

/// Long enough for a first-time Rust toolchain on a slow link; every later
/// job finds the version already in the shared data dir and takes seconds.
const USER_TIMEOUT_MS: u64 = 20 * 60_000;
const ROOT_TIMEOUT_MS: u64 = 10 * 60_000;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EnvPlan {
    /// The repo ships its own mise.toml / .tool-versions.
    pub repo_config: bool,
    /// Tools to write to mise.local.toml, in insertion order.
    pub tools: Vec<(String, String)>,
    pub apt: Vec<String>,
    /// A Rust repo that has not opted out: sccache is in `tools`, and the
    /// install wires it into the repo's env.
    pub sccache: bool,
    /// Declared entries that failed validation, told back to the agent.
    pub rejected: Vec<String>,
    /// The repo is a Talaria app (a talaria.json at its root): prepare_env
    /// installs the SDK kit, and the kit's checkers get node.
    pub talaria_app: bool,
}

impl EnvPlan {
    fn set_tool(&mut self, name: &str, version: &str) {
        if !valid_tool(name) || !valid_version(version) {
            self.rejected.push(format!("tool {name}@{version}"));
            return;
        }
        match self.tools.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = version.to_string(),
            None => self.tools.push((name.to_string(), version.to_string())),
        }
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("a static pattern compiles"))
}

/// Debian policy for package names (plus an optional :arch).
fn valid_apt(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    re(&RE, r"^[a-z0-9][a-z0-9+.\-]{1,62}(:[a-z0-9]+)?$").is_match(name)
}

/// A mise tool, bare (`node`) or with a backend (`aqua:rui314/mold`).
fn valid_tool(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    re(&RE, r"^[a-z0-9][a-z0-9_\-]*(:[A-Za-z0-9_.@/\-]+)?$").is_match(name) && name.len() <= 100
}

fn valid_version(v: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    re(&RE, r"^[A-Za-z0-9][A-Za-z0-9._+\-]{0,39}$").is_match(v)
}

fn first_line(s: &str) -> &str {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("")
}

fn is_root(path: &str) -> bool {
    !path.contains('/') || path.starts_with(".cargo/") || path.starts_with(".talaria/")
}

/// Root-level markers win over one-level-down ones for the same ecosystem.
fn pick<'a>(files: &'a [(String, String)], names: &[&str]) -> Option<&'a (String, String)> {
    let hit = |root: bool| {
        files.iter().find(|(p, _)| {
            is_root(p) == root
                && names
                    .iter()
                    .any(|n| p == n || p.ends_with(&format!("/{n}")))
        })
    };
    hit(true).or_else(|| hit(false))
}

fn any(files: &[(String, String)], names: &[&str]) -> bool {
    pick(files, names).is_some()
}

/// The detection. Pure: `files` is what the scan found (path, content).
pub fn plan_env(files: &[(String, String)]) -> EnvPlan {
    let mut plan = EnvPlan {
        repo_config: files
            .iter()
            .any(|(p, _)| matches!(p.as_str(), "mise.toml" | ".mise.toml" | ".tool-versions")),
        ..EnvPlan::default()
    };

    if !plan.repo_config {
        // Rust: the toolchain file's channel, else stable for any crate.
        static CHANNEL: OnceLock<Regex> = OnceLock::new();
        if let Some((_, text)) = pick(files, &["rust-toolchain.toml"])
            && let Some(c) = re(&CHANNEL, r#"(?m)^\s*channel\s*=\s*"([^"]+)""#).captures(text)
        {
            plan.set_tool("rust", &c[1]);
        } else if let Some((_, text)) = pick(files, &["rust-toolchain"]) {
            plan.set_tool("rust", first_line(text));
        } else if any(files, &["Cargo.toml"]) {
            plan.set_tool("rust", "stable");
        }
        if files
            .iter()
            .any(|(p, t)| p.ends_with(".cargo/config.toml") && t.contains("fuse-ld=mold"))
        {
            plan.set_tool("mold", "latest");
        }

        // JavaScript: an explicit node pin, else LTS for any package.json;
        // bun / pnpm from packageManager or their lockfiles.
        let package_manager = pick(files, &["package.json"])
            .and_then(|(_, t)| serde_json::from_str::<Value>(t).ok())
            .and_then(|v| v["packageManager"].as_str().map(str::to_string));
        if let Some((_, text)) = pick(files, &[".nvmrc", ".node-version"]) {
            plan.set_tool("node", first_line(text).trim_start_matches('v'));
        } else if any(files, &["package.json"]) {
            plan.set_tool("node", "lts");
        }
        let pm_version = |tool: &str| {
            package_manager
                .as_deref()
                .and_then(|pm| pm.strip_prefix(&format!("{tool}@")))
                .map(|v| v.split('+').next().unwrap_or(v).to_string())
        };
        if let Some(v) = pm_version("bun") {
            plan.set_tool("bun", &v);
        } else if any(files, &["bun.lock", "bun.lockb"]) {
            plan.set_tool("bun", "latest");
        }
        if let Some(v) = pm_version("pnpm") {
            plan.set_tool("pnpm", &v);
        } else if any(files, &["pnpm-lock.yaml"]) {
            plan.set_tool("pnpm", "latest");
        }

        // Go: the toolchain directive, else the go directive.
        static GO_TOOLCHAIN: OnceLock<Regex> = OnceLock::new();
        static GO_LINE: OnceLock<Regex> = OnceLock::new();
        if let Some((_, text)) = pick(files, &["go.mod"]) {
            if let Some(c) = re(&GO_TOOLCHAIN, r"(?m)^toolchain\s+go([0-9][0-9.]*)").captures(text)
            {
                plan.set_tool("go", &c[1]);
            } else if let Some(c) = re(&GO_LINE, r"(?m)^go\s+([0-9][0-9.]*)").captures(text) {
                plan.set_tool("go", &c[1]);
            } else {
                plan.set_tool("go", "latest");
            }
        }

        // Python: an explicit pin, else a current 3.x for any project file.
        if let Some((_, text)) = pick(files, &[".python-version"]) {
            plan.set_tool("python", first_line(text));
        } else if any(files, &["pyproject.toml", "requirements.txt"]) {
            plan.set_tool("python", "3.12");
        }
        if any(files, &["uv.lock"]) {
            plan.set_tool("uv", "latest");
        }

        if let Some((_, text)) = pick(files, &[".ruby-version"]) {
            plan.set_tool("ruby", first_line(text));
        }
    }

    // A Talaria app. Keyed on the manifest at the root and nothing else —
    // never on the repo being Talaria's. An app repo has no package.json of
    // its own, but the kit's checkers (svelte-check, vitest) are node
    // programs, so a repo that pins no node gets one; the kit itself runs on
    // the harness's bun, which every sandbox has.
    plan.talaria_app = files.iter().any(|(p, _)| p == "talaria.json");
    if plan.talaria_app && !plan.repo_config && !plan.tools.iter().any(|(n, _)| n == "node") {
        plan.set_tool("node", "lts");
    }

    // The repo's explicit extras apply on top of either source.
    let mut sccache_opt_out = false;
    if let Some((_, text)) = files.iter().find(|(p, _)| p == ".talaria/workbench.toml") {
        match text.parse::<toml_edit::DocumentMut>() {
            Ok(doc) => {
                sccache_opt_out = doc.get("sccache").and_then(|v| v.as_bool()) == Some(false);
                if let Some(list) = doc.get("apt").and_then(|a| a.as_array()) {
                    for item in list.iter() {
                        match item.as_str() {
                            Some(name) if valid_apt(name) => {
                                if !plan.apt.iter().any(|a| a == name) {
                                    plan.apt.push(name.to_string());
                                }
                            }
                            _ => plan.rejected.push(format!("apt package {item}")),
                        }
                    }
                }
                if let Some(tools) = doc.get("tools").and_then(|t| t.as_table_like()) {
                    for (name, version) in tools.iter() {
                        match version.as_str() {
                            Some(v) => plan.set_tool(name, v),
                            None => plan
                                .rejected
                                .push(format!("tool {name} (version must be a string)")),
                        }
                    }
                }
            }
            Err(e) => plan
                .rejected
                .push(format!(".talaria/workbench.toml does not parse: {e}")),
        }
    }
    // Any crate in the repo, whichever file pinned the toolchain.
    if any(files, &["Cargo.toml"]) && !sccache_opt_out {
        plan.sccache = true;
        plan.set_tool("sccache", "latest");
    }
    plan
}

// ── Cleanup: what a job's workdir may give back ─────────────────────────────
//
// Job teardown asks "is this job over?". It never asked "how big is this?",
// so a job that is perfectly alive holds a checkout plus whatever its
// toolchain writes beside it — and a cold Rust `target/` is 4-20 GiB on its
// own. Reclaim is the rung between Stop and Remove: drop what the project can
// rebuild, keep the checkout, the branch and every uncommitted edit.
//
// The platform cannot guess what is rebuildable. `target/` costs a cargo
// build; `.venv` cost twenty minutes and a private index. So the project says
// — and a project that says nothing still gets the detection below, because a
// repo that must opt in mostly will not, and then the disk fills anyway.

/// Rebuildable output per ecosystem. The rule is CHEAP AND LOCAL is
/// reclaimable, SLOW OR REMOTE is not.
///
/// What is deliberately absent matters as much as what is here.
/// `node_modules` itself is not reclaimable — rebuilding it means going back
/// to the network — so only its cache is. `vendor/` is often committed and
/// load-bearing offline. Go's build cache lives in `GOCACHE` outside the
/// repo, so it is a department cache to cap, never a per-job path to drop.
const DETECTED_ARTIFACTS: &[(&[&str], &[&str])] = &[
    (&["Cargo.toml"], &["target"]),
    (
        &["package.json"],
        &[
            "node_modules/.cache",
            "dist",
            "build",
            ".next",
            ".nuxt",
            ".svelte-kit",
            ".turbo",
        ],
    ),
    (
        &["pyproject.toml", "requirements.txt", ".python-version"],
        &["__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache"],
    ),
    (&["pom.xml"], &["target"]),
    (&["build.gradle", "build.gradle.kts"], &["build", ".gradle"]),
    (&["mix.exs"], &["_build"]),
];

/// What one job's workdir may give back, and what it never may.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupPlan {
    /// Repo-relative and rebuildable: detection, plus whatever the repo
    /// declared. This list ends in an `rm -rf`, so every entry is validated.
    pub artifacts: Vec<String>,
    /// Repo-relative and never dropped, whatever the pressure. Wins over
    /// `artifacts`, including over a detected entry.
    pub keep: Vec<String>,
    /// This repo's ceiling for ONE job's workdir, in GiB.
    pub max_workdir_gib: Option<u64>,
    /// Declared entries that failed validation, told back through `doctor`.
    pub rejected: Vec<String>,
}

/// A path this may hand to `rm -rf`, inside the workdir and nowhere else.
/// Absolute paths, `..` in any position, an empty segment, and `.git` at any
/// depth are all refused here rather than left to the shell.
pub fn valid_rel_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 200
        && !p.starts_with('/')
        && !p.starts_with('~')
        && !p.split('/').any(|seg| {
            seg.is_empty() || seg == ".." || seg == "." || seg.eq_ignore_ascii_case(".git")
        })
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

/// The cleanup rules for a repo. Pure, like `plan_env`: `files` is what the
/// scan found, so the whole policy is pinned by tests without a container.
pub fn plan_cleanup(files: &[(String, String)]) -> CleanupPlan {
    let mut plan = CleanupPlan::default();
    for (markers, paths) in DETECTED_ARTIFACTS {
        if any(files, markers) {
            for p in *paths {
                push_unique(&mut plan.artifacts, p);
            }
        }
    }
    // .NET names its project file after the project, so it is a suffix match
    // rather than one of `any`'s fixed names.
    if files
        .iter()
        .any(|(p, _)| p.ends_with(".csproj") || p.ends_with(".sln"))
    {
        push_unique(&mut plan.artifacts, "bin");
        push_unique(&mut plan.artifacts, "obj");
    }

    let mut declared: Option<Vec<String>> = None;
    if let Some((_, text)) = files.iter().find(|(p, _)| p == ".talaria/workbench.toml") {
        match text.parse::<toml_edit::DocumentMut>() {
            Ok(doc) => {
                if let Some(t) = doc.get("cleanup").and_then(|c| c.as_table_like()) {
                    if let Some(list) = t.get("artifacts").and_then(|a| a.as_array()) {
                        let mut got: Vec<String> = Vec::new();
                        for item in list.iter() {
                            match item.as_str() {
                                Some(s) if valid_rel_path(s) => push_unique(&mut got, s),
                                _ => plan.rejected.push(format!("cleanup artifact {item}")),
                            }
                        }
                        declared = Some(got);
                    }
                    if let Some(list) = t.get("keep").and_then(|a| a.as_array()) {
                        for item in list.iter() {
                            match item.as_str() {
                                Some(s) if valid_rel_path(s) => push_unique(&mut plan.keep, s),
                                _ => plan.rejected.push(format!("cleanup keep {item}")),
                            }
                        }
                    }
                    match t.get("maxWorkdirGib").map(|v| v.as_integer()) {
                        Some(Some(n)) if n > 0 => plan.max_workdir_gib = Some(n as u64),
                        Some(_) => plan
                            .rejected
                            .push("cleanup maxWorkdirGib (a positive integer)".to_string()),
                        None => {}
                    }
                }
            }
            Err(e) => plan
                .rejected
                .push(format!(".talaria/workbench.toml does not parse: {e}")),
        }
    }

    // Declared artifacts MERGE OVER detection — name what detection misses,
    // without restating it. An explicit empty list is the opt-out.
    if let Some(got) = declared {
        if got.is_empty() {
            plan.artifacts.clear();
        } else {
            for p in got {
                push_unique(&mut plan.artifacts, &p);
            }
        }
    }
    // `keep` wins over everything, including a detected default, and covers
    // the subtree beneath it.
    let keep = plan.keep.clone();
    plan.artifacts.retain(|a| {
        !keep
            .iter()
            .any(|k| k == a || a.starts_with(&format!("{k}/")))
    });
    plan
}

/// mise.local.toml for the plan's tools, written with toml_edit so no value
/// can escape its string.
pub fn mise_local_toml(tools: &[(String, String)]) -> String {
    let mut doc = toml_edit::DocumentMut::new();
    let mut table = toml_edit::Table::new();
    for (name, version) in tools {
        table.insert(name, toml_edit::value(version.as_str()));
    }
    doc.insert("tools", toml_edit::Item::Table(table));
    format!(
        "# Written by Talaria's prepare_env from the repo's own files; kept out of\n\
         # git by .git/info/exclude. Edit freely, or commit a mise.toml instead.\n{doc}"
    )
}

pub struct Scan {
    pub root: String,
    pub owner: String,
    pub files: Vec<(String, String)>,
    /// Markers the scan found but refused: a symlink, or a path that resolves
    /// outside the checkout. Told back to the agent, never read.
    pub skipped: Vec<String>,
}

/// The marker scan on its own, for a caller that wants a repo's policy
/// without setting anything up. The sweep reads it to learn what a job may
/// give back, and pays a `head -c 8192` of a handful of files rather than a
/// column on `workbench_jobs` that would then have to be kept in step with
/// the repo — the repo is the authority, so ask the repo.
pub async fn scan_repo_files(
    pg: &PgPool,
    department: &str,
    workdir: &str,
) -> Option<Vec<(String, String)>> {
    let container = managed_container(pg, department).await;
    let (out, _) = scan_exec(&container, workdir).await.ok()?;
    parse_scan(&out).map(|s| s.files)
}

/// The scan, as the agent's user. Positional args only: the script is
/// `-c`'s text, the workdir is `$1`, the markers are `$2..`.
async fn scan_exec(container: &str, workdir: &str) -> Result<(String, String), String> {
    let mut cmd: Vec<&str> = vec!["sh", "-c", SCAN_SH, "sh", workdir];
    cmd.extend(MARKERS.iter().copied());
    docker_exec_as(container, Some(AGENT_USER), &cmd, None, 60_000).await
}

/// Parses SCAN_SH's output. None = no git checkout under the workdir yet.
pub fn parse_scan(out: &str) -> Option<Scan> {
    let (head, rest) = out.split_once('\u{1e}').unwrap_or((out, ""));
    let mut root = None;
    let mut owner = String::new();
    let mut skipped = Vec::new();
    for line in head.lines() {
        if line.trim() == "NOREPO" {
            return None;
        }
        if let Some(r) = line.strip_prefix("ROOT ") {
            root = Some(r.to_string());
        } else if let Some(o) = line.strip_prefix("OWNER ") {
            owner = o.to_string();
        } else if let Some(p) = line.strip_prefix("SKIP ") {
            skipped.push(p.to_string());
        }
    }
    let files = if rest.is_empty() {
        Vec::new()
    } else {
        rest.split('\u{1e}')
            .filter_map(|chunk| {
                let (path, body) = chunk.split_once('\n')?;
                Some((path.to_string(), body.to_string()))
            })
            .collect()
    };
    Some(Scan {
        root: root?,
        owner,
        files,
        skipped,
    })
}

// ── The project env: one repo's store, in one job's workdir ────────────────
//
// A repo's env store (`repo_env`, sealed at rest) used to be unsealed by the
// fleet render into `fleet/agents/<slug>/workbench-env/<owner>/<name>.env`
// on the host — plain files, every granted repo at once — and mounted into
// the agent's container for the life of the container. That made the
// "secrets never sit in config files" rule untrue for exactly these values,
// and let a job on repo A read repo B's env from the same mount. Now the
// value leaves the database into one place only: `<workdir>/.talaria/env`
// for the job that is working that repo, written as the agent (0600, owned
// by it, through stdin — never a shell interpolation) by prepare_env, once
// the checkout exists (a non-empty workdir would refuse `git clone <url>
// <workdir>`), and removed by the job's teardown.

/// How long a container's provisioned harness runtime is trusted before it
/// is checked again. The check is cheap once done (no download), so this only
/// spares doctor and start_job an exec on calls a minute apart.
const RUNTIME_OK_FOR: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Long enough for a first-time mise + bun download on a slow link.
const RUNTIME_TIMEOUT_MS: u64 = 5 * 60_000;

fn runtime_ok_cache()
-> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static CACHE: OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    > = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// PUT THE HARNESS'S RUNTIME IN THE SANDBOX before anything asks it to run.
///
/// omp is a bun program, and nothing else guarantees bun is there: the
/// container image does not ship it, and the only other step that installed it
/// (prepare_env) runs after a clone and reached the PATH only through a login
/// shell's profile. So a fresh Developer Agent's doctor, and any command its
/// terminal ran without a login shell, met `env: 'bun': No such file or
/// directory` — and the agent, correctly, refused to start jobs. This runs the
/// shared bootstrap (`talaria_workbench_harnesses::harness_runtime_script`) as
/// the agent's own user: mise if missing, bun pinned and installed, and `bun`
/// linked onto the PATH every shell has. Idempotent; Ok carries bun's version.
pub async fn ensure_harness_runtime(pg: &PgPool, department: &str) -> Result<String, String> {
    let container = managed_container(pg, department).await;
    if let Some(at) = runtime_ok_cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&container)
        && at.elapsed() < RUNTIME_OK_FOR
    {
        return Ok(talaria_workbench_harnesses::HARNESS_BUN.to_string());
    }
    let script = talaria_workbench_harnesses::harness_runtime_script();
    let (out, err) = docker_exec_as(
        &container,
        Some(AGENT_USER),
        &["sh", "-c", &script],
        None,
        RUNTIME_TIMEOUT_MS,
    )
    .await
    .map_err(|e| {
        // The exec's own error carries the script's stderr; the last line
        // is the reason worth repeating.
        let why = e
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("no output")
            .to_string();
        format!(
            "could not provision bun {} for the harness: {why}",
            talaria_workbench_harnesses::HARNESS_BUN
        )
    })?;
    let version = out
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            let why = err
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .unwrap_or("no output");
            format!("bun did not answer after provisioning: {why}")
        })?;
    runtime_ok_cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(container, std::time::Instant::now());
    Ok(version)
}

// ── The SDK kit: a Talaria app's toolchain ─────────────────────────────────
//
// A repo with a talaria.json at its root is a Talaria app, and the instance
// serves the kit that builds one (`GET /api/sdk/kit.tgz`: the SDK's
// declarations, the host's build recipe, the `talaria-app` CLI). The sandbox
// fetches it AS THE AGENT — its own credential and the app's origin are
// already in the container's env for the git credential helper — so the kit
// takes no path but the one any agent's request takes, and the api hands the
// container nothing. One install per kit version, on the department volume
// beside the harness runtime, shared by every job; `talaria-app` linked into
// the bin directory every shell has, beside bun.

/// Where kits are unpacked and installed, one directory per kit version: in
/// the harness's data dir on the department volume, which the agent's user
/// owns (mise keeps its installs there).
pub fn kit_store() -> String {
    format!(
        "{}/talaria-sdk-kit",
        talaria_workbench_harnesses::HARNESS_XDG_DATA_HOME
    )
}

/// The workdir-relative file a job's eval session is written to. The kit's
/// `talaria-app eval --gateway` finds it walking up from the app and sends
/// its key as `x-talaria-session`, so the relay books those turns to the
/// job's ticket.
pub const EVAL_SESSION_FILE: &str = ".talaria/eval-session.json";

/// `talaria-app` where the agent's terminal finds it (`install_app_kit`).
pub const KIT_TERMINAL_LINK: &str = "/usr/local/bin/talaria-app";

/// Long enough for a first install of the kit's packages on a slow link;
/// every later job of the same version finds it installed.
const KIT_TIMEOUT_MS: u64 = 10 * 60_000;

/// $1 = the kit store, $2 = the bin dir (bun is there), $3 = the workdir.
/// stdin = the eval session's JSON, or nothing. Runs as the agent.
const KIT_SH: &str = r#"
set -eu
kits="$1"; bin="$2"; wd="$3"
base="${TALARIA_API_URL:-}"
[ -n "$base" ] || { echo "TALARIA_API_URL is not set in this sandbox, so the SDK kit has nowhere to come from" >&2; exit 1; }
[ -n "${TALARIA_AGENT_KEY:-}" ] || { echo "TALARIA_AGENT_KEY is not set in this sandbox, so it cannot ask for the SDK kit" >&2; exit 1; }
[ -x "$bin/bun" ] || { echo "no bun at $bin/bun to install the SDK kit with" >&2; exit 1; }
mkdir -p "$kits" "$bin"
tmp=$(mktemp -d "$kits/.fetch-XXXXXX")
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/session"
url="${base%/}/api/sdk/kit.tgz"
code=$(curl -sS -o "$tmp/kit.tgz" -D "$tmp/headers" -w '%{http_code}' \
  -H "X-Agent-Name: ${API_SERVER_MODEL_NAME:-}" -H "X-Api-Key: $TALARIA_AGENT_KEY" "$url") \
  || { echo "could not reach $url" >&2; exit 1; }
if [ "$code" != 200 ]; then
  echo "the instance answered $code for $url: $(head -c 300 "$tmp/kit.tgz")" >&2
  exit 1
fi
ver=$(tr -d '\r' < "$tmp/headers" | sed -n 's/^[Xx]-[Tt]alaria-[Kk]it-[Vv]ersion: *//p' | tail -n 1)
case "$ver" in
  ''|*[!A-Za-z0-9._+-]*|[!A-Za-z0-9]*) echo "the instance named no usable kit version (\"$ver\")" >&2; exit 1 ;;
esac
dest="$kits/$ver"
if [ ! -f "$dest/.installed" ]; then
  mkdir "$tmp/kit"
  tar -xzf "$tmp/kit.tgz" -C "$tmp/kit"
  if ! (cd "$tmp/kit" && "$bin/bun" install --no-progress >"$tmp/install.log" 2>&1); then
    tail -n 20 "$tmp/install.log" >&2
    exit 1
  fi
  touch "$tmp/kit/.installed"
  # A half-placed earlier attempt goes; a finished one another job placed
  # meanwhile stays, and this copy is dropped.
  if [ -e "$dest" ] && [ ! -f "$dest/.installed" ]; then rm -rf "$dest"; fi
  mv -T "$tmp/kit" "$dest" 2>/dev/null || [ -f "$dest/.installed" ] \
    || { echo "could not place the SDK kit at $dest" >&2; exit 1; }
fi
ln -sfn "$dest" "$kits/current"
ln -sfn "$kits/current/bin/talaria-app" "$bin/talaria-app"
if [ -s "$tmp/session" ]; then
  umask 077
  mkdir -p "$wd/.talaria"
  cp "$tmp/session" "$wd/.talaria/.eval-session.tmp"
  mv -f "$wd/.talaria/.eval-session.tmp" "$wd/.talaria/eval-session.json"
  if [ -d "$wd/.git" ] && ! grep -qx '.talaria/eval-session.json' "$wd/.git/info/exclude" 2>/dev/null; then
    mkdir -p "$wd/.git/info"
    echo '.talaria/eval-session.json' >> "$wd/.git/info/exclude"
  fi
fi
echo "=== kit"
"$bin/talaria-app" version
"#;

/// The eval session a job's `talaria-app eval --gateway` books under, as the
/// file the kit reads it from.
pub fn eval_session_json(key: &str, job_id: &str) -> String {
    json!({ "key": key, "jobId": job_id }).to_string()
}

/// Install the SDK kit for a Talaria app's job. Ok carries what the agent is
/// told: the version, where `talaria-app` is, and the loop to run before
/// finish_job. `session` is the job's eval session (`eval_session_json`), or
/// None when none could be recorded — eval through the gateway still works,
/// booked to the workbench key without a ticket.
pub async fn install_app_kit(
    pg: &PgPool,
    department: &str,
    workdir: &str,
    session: Option<&str>,
) -> Result<Value, String> {
    let container = managed_container(pg, department).await;
    let bin = talaria_workbench_harnesses::AGENT_BIN_DIR;
    let (out, _) = docker_exec_as(
        &container,
        Some(AGENT_USER),
        &["sh", "-c", KIT_SH, "sh", &kit_store(), bin, workdir],
        Some(session.unwrap_or("")),
        KIT_TIMEOUT_MS,
    )
    .await
    .map_err(|e| {
        let why = e
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("no output")
            .to_string();
        format!("the SDK kit could not be installed: {why}")
    })?;
    let version = out
        .split_once("=== kit\n")
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    // And where the AGENT'S TERMINAL looks: Hermes runs each command from a
    // login-shell snapshot whose PATH Debian's /etc/profile resets to
    // /usr/local/bin:/usr/bin:/bin, so the bin dir above is not on it (the
    // boot hook links bun there for the same reason). Root writes one fixed
    // link and reads nothing of the repo; /usr/local/bin is the container
    // layer, so every prepare_env lays it again after a restart.
    let linked = docker_exec(
        &container,
        &[
            "ln",
            "-sfn",
            &format!("{bin}/talaria-app"),
            KIT_TERMINAL_LINK,
        ],
        10_000,
    )
    .await
    .is_ok();
    Ok(json!({
        "ok": true,
        "version": version,
        "bin": if linked { KIT_TERMINAL_LINK.to_string() } else { format!("{bin}/talaria-app") },
        "evalSession": session.map(|_| format!("{workdir}/{EVAL_SESSION_FILE}")),
        "usage": "This repo is a Talaria app. Before finish_job, from the repo: `talaria-app check` (talaria.json, typecheck, the app's own tests), `talaria-app build` (the host's own build recipe and load probe, per surface and per harness) and `talaria-app eval` (every harness fixture, against the replies in harnesses/recorded/; add `--gateway --model <id>` to run them through the Talaria gateway, booked to this job's ticket, and `--record` to keep the replies). `talaria-app scaffold` starts an app in an empty repo. All of it runs here, never on the instance.",
    }))
}

/// Where a job finds its repo's env, relative to the job's workdir.
pub const JOB_ENV_FILE: &str = ".talaria/env";

/// Writes stdin to `$1/.talaria/env`, atomically and 0600. A checkout that
/// sits at the workdir root would see the file as untracked, so it joins
/// `.git/info/exclude` there, the way mise.local.toml does: no `git add`
/// can commit it.
const JOB_ENV_WRITE_SH: &str = r#"
set -e
wd="$1"
umask 077
mkdir -p "$wd/.talaria"
tmp="$wd/.talaria/.env.tmp"
cat > "$tmp"
chmod 600 "$tmp"
mv -f -- "$tmp" "$wd/.talaria/env"
if [ -d "$wd/.git" ] && ! grep -qx '.talaria/env' "$wd/.git/info/exclude" 2>/dev/null; then
  mkdir -p "$wd/.git/info"
  echo '.talaria/env' >> "$wd/.git/info/exclude"
fi
"#;

/// The store was emptied (or never existed): a file left from an earlier
/// write must not outlive the store it mirrored. $1 = workdir.
const JOB_ENV_REMOVE_SH: &str = r#"rm -f -- "$1/.talaria/env""#;

/// The shell line an agent loads the file with: `set -a` exports every
/// assignment, which a bare `source` would leave in the shell alone.
pub fn job_env_source_line(workdir: &str) -> String {
    format!("set -a; . {workdir}/{JOB_ENV_FILE}; set +a")
}

/// Materialize `repo`'s env store into the job's workdir, as the agent.
/// `Ok(Some(n))` — n variables written; `Ok(None)` — the repo has no store
/// (and any earlier file is gone). The sealed values are opened here and go
/// straight down the exec's stdin; nothing of them touches the host's disk
/// or a command line.
pub async fn deliver_job_env(
    pg: &PgPool,
    sb: &talaria_secretbox::SecretBox,
    department: &str,
    workdir: &str,
    repo: &str,
) -> Result<Option<usize>, String> {
    let body = talaria_repo_env::env_file(pg, sb, repo).await?;
    let container = managed_container(pg, department).await;
    match body {
        Some(body) => {
            docker_exec_as(
                &container,
                Some(AGENT_USER),
                &["sh", "-c", JOB_ENV_WRITE_SH, "sh", workdir],
                Some(&body),
                30_000,
            )
            .await
            .map_err(|e| format!("writing {workdir}/{JOB_ENV_FILE}: {e}"))?;
            Ok(Some(body.lines().count()))
        }
        None => {
            docker_exec_as(
                &container,
                Some(AGENT_USER),
                &["sh", "-c", JOB_ENV_REMOVE_SH, "sh", workdir],
                None,
                30_000,
            )
            .await
            .map_err(|e| format!("removing {workdir}/{JOB_ENV_FILE}: {e}"))?;
            Ok(None)
        }
    }
}

/// What start_job and prepare_env tell the agent about its env, in one
/// shape: the path, the count, and the line that loads it.
pub fn job_env_answer(workdir: &str, vars: Option<usize>) -> Value {
    match vars {
        Some(n) => json!({
            "path": format!("{workdir}/{JOB_ENV_FILE}"),
            "vars": n,
            "load": job_env_source_line(workdir),
            "note": "This job's project env, owned by you (0600). Load it before you build or test; it is removed when the job ends.",
        }),
        None => Value::Null,
    }
}

/// The verb's body. `workdir` comes from a validated job id.
pub async fn prepare_env(pg: &PgPool, department: &str, workdir: &str) -> Result<Value, String> {
    let container = managed_container(pg, department).await;

    // As the agent: the repo is the agent's content, and root reads none of
    // it. Only the validated apt names (below) ever reach a root step.
    let (out, _) = scan_exec(&container, workdir).await?;
    let Some(scan) = parse_scan(&out) else {
        return Err(format!(
            "no git checkout in {workdir} yet: clone the repo there first, then call prepare_env again"
        ));
    };
    // Everything past apt runs as the checkout's owner, which is the agent's
    // user whatever the image names it, in that user's home.
    let owner = if scan.owner.is_empty() || scan.owner == "root" {
        "hermes".to_string()
    } else {
        scan.owner.clone()
    };
    let (home_out, _) = docker_exec(
        &container,
        &[
            "sh",
            "-c",
            "getent passwd \"$1\" | cut -d: -f6",
            "sh",
            &owner,
        ],
        10_000,
    )
    .await?;
    let home = match home_out.trim() {
        "" => "/opt/data".to_string(),
        h => h.to_string(),
    };

    let plan = plan_env(&scan.files);

    if !plan.apt.is_empty() {
        let mut root_cmd: Vec<&str> = vec!["sh", "-c", ROOT_SH, "sh"];
        root_cmd.extend(plan.apt.iter().map(String::as_str));
        docker_exec(&container, &root_cmd, ROOT_TIMEOUT_MS)
            .await
            .map_err(|e| format!("apt install failed ({}): {e}", plan.apt.join(" ")))?;
    }

    let wrote = !plan.tools.is_empty();
    if wrote {
        let local = format!("{}/mise.local.toml", scan.root);
        docker_exec_opt(
            &container,
            &[
                "runuser",
                "-u",
                &owner,
                "--",
                "sh",
                "-c",
                "cat > \"$1\"",
                "sh",
                &local,
            ],
            Some(&mise_local_toml(&plan.tools)),
            30_000,
        )
        .await
        .map_err(|e| format!("writing mise.local.toml: {e}"))?;
    }

    let (installed, _) = docker_exec(
        &container,
        &[
            "runuser",
            "-u",
            &owner,
            "--",
            "sh",
            "-c",
            USER_SH,
            "sh",
            &home,
            &scan.root,
            if wrote { "1" } else { "0" },
            if plan.sccache { SCCACHE_DIR } else { "" },
            AGENT_TERMINAL_HOME,
        ],
        USER_TIMEOUT_MS,
    )
    .await
    .map_err(|e| format!("mise install failed in {}: {e}", scan.root))?;
    let resolved = installed
        .split_once("=== resolved\n")
        .map(|(_, r)| r.trim().to_string())
        .unwrap_or_else(|| installed.trim().to_string());

    Ok(json!({
        "repoRoot": scan.root,
        "toolsFrom": match (plan.repo_config, wrote) {
            (true, false) => "the repo's mise config",
            (true, true) => "the repo's mise config, plus .talaria/workbench.toml tools (mise.local.toml, git-excluded)",
            (false, true) => "detected from the repo's files (mise.local.toml, git-excluded)",
            (false, false) => "nothing to install",
        },
        "tools": plan.tools.iter().map(|(n, v)| format!("{n}@{v}")).collect::<Vec<_>>(),
        "apt": plan.apt,
        "sccache": plan.sccache,
        "talariaApp": plan.talaria_app,
        "rejected": plan.rejected,
        "skipped": scan.skipped,
        "resolved": resolved,
        "usage": "Toolchains resolve through mise shims by directory, so run them from inside the repo (cd into it first; outside it a shim answers \"No version is set\"). A terminal session started after this call has them on PATH; in one opened before it, run `. ~/.profile` first, or prefix a command with `mise exec --`. Do not install toolchains yourself: add them to the repo's mise.toml or .talaria/workbench.toml and call prepare_env again.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, body: &str) -> (String, String) {
        (path.to_string(), body.to_string())
    }

    #[test]
    fn a_repo_that_declares_nothing_still_gets_its_build_output_detected() {
        let rust = plan_cleanup(&[f("Cargo.toml", "[package]\n")]);
        assert_eq!(rust.artifacts, vec!["target"]);
        let node = plan_cleanup(&[f("package.json", "{}")]);
        assert!(node.artifacts.contains(&"dist".to_string()));
        assert!(node.artifacts.contains(&"node_modules/.cache".to_string()));
        // node_modules ITSELF is rebuildable only by going back to the
        // network, so it is never on the list.
        assert!(!node.artifacts.contains(&"node_modules".to_string()));
        let dotnet = plan_cleanup(&[f("Api.csproj", "<Project/>")]);
        assert_eq!(dotnet.artifacts, vec!["bin", "obj"]);
        // Go's build cache lives outside the repo, so nothing is dropped
        // per job — vendor/ is often committed and load-bearing offline.
        assert!(
            plan_cleanup(&[f("go.mod", "module x\n")])
                .artifacts
                .is_empty()
        );
    }

    #[test]
    fn declared_artifacts_merge_over_detection_and_an_empty_list_opts_out() {
        let merged = plan_cleanup(&[
            f("Cargo.toml", "[package]\n"),
            f(
                ".talaria/workbench.toml",
                "[cleanup]\nartifacts = [\".turbo\"]\n",
            ),
        ]);
        assert_eq!(merged.artifacts, vec!["target", ".turbo"]);
        let opted_out = plan_cleanup(&[
            f("Cargo.toml", "[package]\n"),
            f(".talaria/workbench.toml", "[cleanup]\nartifacts = []\n"),
        ]);
        assert!(opted_out.artifacts.is_empty());
    }

    #[test]
    fn keep_wins_over_detection_and_covers_its_subtree() {
        let plan = plan_cleanup(&[
            f("package.json", "{}"),
            f(".talaria/workbench.toml", "[cleanup]\nkeep = [\"dist\"]\n"),
        ]);
        assert!(!plan.artifacts.contains(&"dist".to_string()));
        assert!(plan.artifacts.contains(&".next".to_string()));
        let subtree = plan_cleanup(&[
            f("Cargo.toml", "[package]\n"),
            f(
                ".talaria/workbench.toml",
                "[cleanup]\nartifacts = [\"target/debug\"]\nkeep = [\"target\"]\n",
            ),
        ]);
        assert!(subtree.artifacts.is_empty());
    }

    #[test]
    fn a_path_that_could_escape_the_workdir_is_refused() {
        assert!(valid_rel_path("target"));
        assert!(valid_rel_path("node_modules/.cache"));
        assert!(!valid_rel_path(""));
        assert!(!valid_rel_path("/etc/passwd"));
        assert!(!valid_rel_path("~/.ssh"));
        assert!(!valid_rel_path("../../etc"));
        assert!(!valid_rel_path("target/../../etc"));
        assert!(!valid_rel_path("target//x"));
        // .git is never a legal target, at any depth or casing.
        assert!(!valid_rel_path(".git"));
        assert!(!valid_rel_path(".git/objects"));
        assert!(!valid_rel_path("sub/.GIT"));
        assert!(!valid_rel_path("a b; rm -rf /"));
    }

    #[test]
    fn a_rejected_declaration_is_reported_not_silently_dropped() {
        let plan = plan_cleanup(&[f(
            ".talaria/workbench.toml",
            "[cleanup]\nartifacts = [\"../escape\"]\nmaxWorkdirGib = 0\n",
        )]);
        assert!(plan.artifacts.is_empty());
        assert_eq!(plan.max_workdir_gib, None);
        assert_eq!(plan.rejected.len(), 2);
    }

    #[test]
    fn a_ceiling_is_read_when_it_is_a_positive_integer() {
        let plan = plan_cleanup(&[f(
            ".talaria/workbench.toml",
            "[cleanup]\nmaxWorkdirGib = 40\n",
        )]);
        assert_eq!(plan.max_workdir_gib, Some(40));
    }

    #[test]
    fn a_repo_mise_config_is_used_as_is() {
        let plan = plan_env(&[
            f("mise.toml", "[tools]\nnode = \"22\"\n"),
            f("go.mod", "module x\n\ngo 1.22\n"),
            f("package.json", "{}"),
        ]);
        assert!(plan.repo_config);
        assert!(plan.tools.is_empty());
    }

    #[test]
    fn talaria_itself_detects_its_pins() {
        let plan = plan_env(&[
            f("package.json", r#"{"packageManager": "bun@1.4.0"}"#),
            f("bun.lock", ""),
            f(
                "api/rust-toolchain.toml",
                "[toolchain]\nchannel = \"1.97.1\"\n",
            ),
            f("api/Cargo.toml", "[workspace]\n"),
            f(
                "api/.cargo/config.toml",
                "[target.x86_64-unknown-linux-gnu]\nrustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]\n",
            ),
        ]);
        assert!(!plan.repo_config);
        assert_eq!(
            plan.tools,
            vec![
                ("rust".into(), "1.97.1".into()),
                ("mold".into(), "latest".into()),
                ("node".into(), "lts".into()),
                ("bun".into(), "1.4.0".into()),
                ("sccache".into(), "latest".into()),
            ]
        );
        assert!(plan.sccache);
    }

    #[test]
    fn a_rust_repo_with_its_own_mise_config_still_gets_sccache() {
        let plan = plan_env(&[
            f("mise.toml", "[tools]\nrust = \"1.97.1\"\n"),
            f("api/Cargo.toml", "[workspace]\n"),
        ]);
        assert!(plan.repo_config && plan.sccache);
        assert_eq!(plan.tools, vec![("sccache".into(), "latest".into())]);
    }

    #[test]
    fn sccache_is_opt_out_and_rust_only() {
        let plan = plan_env(&[
            f("Cargo.toml", "[package]\n"),
            f(".talaria/workbench.toml", "sccache = false\n"),
        ]);
        assert!(!plan.sccache);
        assert_eq!(plan.tools, vec![("rust".into(), "stable".into())]);
        let plan = plan_env(&[f("package.json", "{}")]);
        assert!(!plan.sccache);
    }

    #[test]
    fn root_markers_win_over_nested_ones() {
        let plan = plan_env(&[f("web/.nvmrc", "18\n"), f(".nvmrc", "v22.12.0\n")]);
        assert_eq!(plan.tools, vec![("node".into(), "22.12.0".into())]);
    }

    #[test]
    fn go_prefers_the_toolchain_directive() {
        let plan = plan_env(&[f("go.mod", "module x\n\ngo 1.22\n\ntoolchain go1.22.5\n")]);
        assert_eq!(plan.tools, vec![("go".into(), "1.22.5".into())]);
        let plan = plan_env(&[f("go.mod", "module x\n\ngo 1.21\n")]);
        assert_eq!(plan.tools, vec![("go".into(), "1.21".into())]);
    }

    #[test]
    fn python_pins_and_uv() {
        let plan = plan_env(&[f("pyproject.toml", ""), f("uv.lock", "")]);
        assert_eq!(
            plan.tools,
            vec![
                ("python".into(), "3.12".into()),
                ("uv".into(), "latest".into())
            ]
        );
    }

    #[test]
    fn the_workbench_file_adds_apt_and_tools_and_rejects_junk() {
        let plan = plan_env(&[
            f("mise.toml", ""),
            f(
                ".talaria/workbench.toml",
                "apt = [\"libssl-dev\", \"protobuf-compiler\", \"x; rm -rf /\", \"libssl-dev\"]\n\
                 [tools]\nprotoc = \"28\"\nevil = \"1; curl x | sh\"\n",
            ),
        ]);
        assert_eq!(plan.apt, vec!["libssl-dev", "protobuf-compiler"]);
        assert_eq!(plan.tools, vec![("protoc".into(), "28".into())]);
        assert_eq!(plan.rejected.len(), 2);
    }

    #[test]
    fn a_broken_workbench_file_is_reported_not_fatal() {
        let plan = plan_env(&[f(".talaria/workbench.toml", "apt = [")]);
        assert!(plan.rejected[0].contains("does not parse"));
    }

    #[test]
    fn the_generated_config_quotes_every_value() {
        let text = mise_local_toml(&[
            ("rust".into(), "1.97.1".into()),
            ("aqua:rui314/mold".into(), "latest".into()),
        ]);
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(doc["tools"]["rust"].as_str(), Some("1.97.1"));
        assert_eq!(doc["tools"]["aqua:rui314/mold"].as_str(), Some("latest"));
    }

    #[test]
    fn the_scan_output_parses() {
        let out = "ROOT /opt/data/workbench/jobs/x/repo\nOWNER hermes\nSKIP .talaria/workbench.toml\n\u{1e}package.json\n{}\n\n\u{1e}api/rust-toolchain.toml\nchannel = \"1\"\n";
        let scan = parse_scan(out).unwrap();
        assert_eq!(scan.root, "/opt/data/workbench/jobs/x/repo");
        assert_eq!(scan.owner, "hermes");
        assert_eq!(scan.files.len(), 2);
        assert_eq!(scan.files[1].0, "api/rust-toolchain.toml");
        assert_eq!(scan.skipped, vec![".talaria/workbench.toml"]);
        assert!(parse_scan("NOREPO\n").is_none());
    }

    /// A throwaway directory for the shell-snippet tests, removed on drop
    /// so a failing assertion never leaves it behind.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "talaria-devenv-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Runs one of the snippets the way the container does: `sh -c <script>
    /// sh <args…>`, with stdin when given. Returns (stdout, success).
    fn run_sh(script: &str, args: &[&str], stdin: Option<&str>) -> (String, bool) {
        use std::io::Write;
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        let out = child.wait_with_output().unwrap();
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.success(),
        )
    }

    /// Runs USER_SH as prepare_env does, with a stub `mise` already in the
    /// passwd home's bin (so nothing downloads) and a pinned env.
    #[cfg(unix)]
    fn run_user_sh(
        home: &std::path::Path,
        term: &str,
        root: &std::path::Path,
        xdg: &std::path::Path,
    ) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mise = bin.join("mise");
        std::fs::write(&mise, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&mise, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::process::Command::new("sh")
            .arg("-c")
            .arg(USER_SH)
            .arg("sh")
            .args([
                home.to_str().unwrap(),
                root.to_str().unwrap(),
                "0",
                "",
                term,
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_DATA_HOME", xdg)
            .output()
            .unwrap()
            .status
            .success()
    }

    #[cfg(unix)]
    #[test]
    fn the_mise_block_reaches_the_agent_terminals_home_with_absolute_paths() {
        // Hermes runs the agent's terminal with HOME at $HERMES_HOME/home, not
        // the passwd home prepare_env runs as. A block written only there, or
        // one that finds mise through $HOME, left a shell the agent opened
        // before prepare_env with no toolchain after `. ~/.profile`.
        let scratch = Scratch::new("miseblock");
        let (home, term, root, xdg) = (
            scratch.0.join("home"),
            scratch.0.join("term"),
            scratch.0.join("repo"),
            scratch.0.join("xdg"),
        );
        std::fs::create_dir_all(&root).unwrap();
        // Twice: the block is rewritten in place, never appended again.
        for _ in 0..2 {
            assert!(run_user_sh(&home, term.to_str().unwrap(), &root, &xdg));
        }
        let data = format!("export MISE_DATA_DIR='{}/mise'", xdg.display());
        let global = format!(
            "export MISE_GLOBAL_CONFIG_FILE='{}/.config/mise/config.toml'",
            home.display()
        );
        for rc in [
            home.join(".profile"),
            home.join(".bashrc"),
            term.join(".profile"),
            term.join(".bashrc"),
        ] {
            let body = std::fs::read_to_string(&rc).unwrap();
            assert_eq!(
                body.matches("# >>> talaria workbench: mise >>>").count(),
                1,
                "{}",
                rc.display()
            );
            assert!(body.contains(&data), "{}: {body}", rc.display());
            assert!(body.contains(&global), "{}: {body}", rc.display());
            assert!(!body.contains("$HOME"), "{}: {body}", rc.display());
        }
        // What the agent's own shell gets from `. ~/.profile`: the shims first.
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(". \"$HOME/.profile\"; printf '%s' \"$PATH\"")
            .env_clear()
            .env("HOME", &term)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let path = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            path.starts_with(&format!("{}/mise/shims:", xdg.display())),
            "{path}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_mise_block_is_written_once_when_both_homes_are_the_same() {
        let scratch = Scratch::new("miseblock-same");
        let (home, root, xdg) = (
            scratch.0.join("home"),
            scratch.0.join("repo"),
            scratch.0.join("xdg"),
        );
        std::fs::create_dir_all(&root).unwrap();
        assert!(run_user_sh(&home, home.to_str().unwrap(), &root, &xdg));
        assert!(run_user_sh(&home, "", &root, &xdg));
        let body = std::fs::read_to_string(home.join(".profile")).unwrap();
        assert_eq!(
            body.matches("# >>> talaria workbench: mise >>>").count(),
            1,
            "{body}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_scan_reads_plain_files_inside_the_checkout_and_nothing_else() {
        use std::os::unix::fs::symlink;
        let scratch = Scratch::new("scan");
        let wd = &scratch.0;
        // The secret a symlink would try to borrow: outside the checkout.
        let outside = wd.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.toml"), "apt = [\"leaked\"]\n").unwrap();
        std::fs::write(outside.join("Cargo.toml"), "[package]\nname = \"leaked\"\n").unwrap();
        // The checkout, one level down the way `git clone <url>` lands it.
        let repo = wd.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join(".talaria")).unwrap();
        std::fs::create_dir_all(repo.join("api")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(repo.join("api/Cargo.toml"), "[package]\n").unwrap();
        // 1. the marker itself is a symlink to a file outside the checkout
        symlink(
            outside.join("secret.toml"),
            repo.join(".talaria/workbench.toml"),
        )
        .unwrap();
        // 2. the marker is a plain file but a directory on its path is a
        //    symlink that leaves the checkout (`*/Cargo.toml` finds it)
        symlink(&outside, repo.join("vendor")).unwrap();
        // 3. a symlink that stays INSIDE the checkout is fine to follow
        symlink(repo.join("api"), repo.join("alias")).unwrap();
        // 4. a dangling symlink is a symlink: skipped, not an error
        symlink(wd.join("nowhere"), repo.join(".nvmrc")).unwrap();
        // A symlinked directory beside the real checkout must not be picked
        // as the root either.
        symlink(&outside, wd.join("0-first")).unwrap();

        let mut args: Vec<&str> = vec![wd.to_str().unwrap()];
        args.extend(MARKERS.iter().copied());
        let (out, ok) = run_sh(SCAN_SH, &args, None);
        assert!(ok, "{out}");
        let scan = parse_scan(&out).expect("a checkout was found");
        assert_eq!(scan.root, repo.display().to_string());
        let read: Vec<&str> = scan.files.iter().map(|(p, _)| p.as_str()).collect();
        assert!(read.contains(&"Cargo.toml"), "{read:?}");
        assert!(read.contains(&"api/Cargo.toml"), "{read:?}");
        assert!(read.contains(&"alias/Cargo.toml"), "{read:?}");
        assert!(!read.contains(&".talaria/workbench.toml"), "{read:?}");
        assert!(!read.contains(&"vendor/Cargo.toml"), "{read:?}");
        assert!(!read.contains(&".nvmrc"), "{read:?}");
        // Not one byte of the outside files came back.
        assert!(!out.contains("leaked"), "{out}");
        let mut skipped = scan.skipped.clone();
        skipped.sort();
        assert_eq!(
            skipped,
            vec![".nvmrc", ".talaria/workbench.toml", "vendor/Cargo.toml"]
        );
        // And the plan built from it sees no apt names: the leak would have
        // been the one way for a symlink to put a package on root's list.
        assert!(plan_env(&scan.files).apt.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_workdir_without_a_checkout_is_norepo() {
        let scratch = Scratch::new("norepo");
        let (out, ok) = run_sh(SCAN_SH, &[scratch.0.to_str().unwrap(), "Cargo.toml"], None);
        assert!(ok);
        assert!(parse_scan(&out).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_env_file_is_private_git_excluded_and_gone_when_the_store_is() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("env");
        let wd = scratch.0.to_str().unwrap();
        // A fresh job: no workdir contents at all yet.
        let (_, ok) = run_sh(JOB_ENV_WRITE_SH, &[wd], Some("API_KEY='a'\nDB='x'\n"));
        assert!(ok);
        let file = scratch.0.join(JOB_ENV_FILE);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "API_KEY='a'\nDB='x'\n"
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!scratch.0.join(".talaria/.env.tmp").exists());
        assert!(!scratch.0.join(".git").exists(), "no checkout, no exclude");
        // A resumed job whose checkout sits at the workdir root: the rewrite
        // replaces the content and keeps the file out of git — once.
        std::fs::create_dir_all(scratch.0.join(".git/info")).unwrap();
        for _ in 0..2 {
            let (_, ok) = run_sh(JOB_ENV_WRITE_SH, &[wd], Some("API_KEY='b'\n"));
            assert!(ok);
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "API_KEY='b'\n");
        assert_eq!(
            std::fs::read_to_string(scratch.0.join(".git/info/exclude")).unwrap(),
            ".talaria/env\n"
        );
        // The store emptied: the file goes, and removing twice is fine.
        for _ in 0..2 {
            let (_, ok) = run_sh(JOB_ENV_REMOVE_SH, &[wd], None);
            assert!(ok);
        }
        assert!(!file.exists());
    }

    #[test]
    fn the_env_answer_names_the_path_and_how_to_load_it() {
        let wd = "/opt/data/workbench/jobs/x";
        let v = job_env_answer(wd, Some(2));
        assert_eq!(v["path"], "/opt/data/workbench/jobs/x/.talaria/env");
        assert_eq!(v["vars"], 2);
        assert_eq!(
            v["load"],
            "set -a; . /opt/data/workbench/jobs/x/.talaria/env; set +a"
        );
        assert!(job_env_answer(wd, None).is_null());
    }

    #[test]
    fn a_talaria_app_is_known_by_its_manifest_at_the_root_and_gets_node_for_the_kit() {
        // Any repo with a talaria.json at its root: no package.json, no
        // mention of Talaria anywhere else.
        let plan = plan_env(&[f("talaria.json", "{\"name\":\"Notes\"}")]);
        assert!(plan.talaria_app);
        assert_eq!(plan.tools, vec![("node".to_string(), "lts".to_string())]);
        // A node the repo pins is kept, not replaced.
        let pinned = plan_env(&[f("talaria.json", "{}"), f(".nvmrc", "20\n")]);
        assert!(pinned.talaria_app);
        assert_eq!(pinned.tools, vec![("node".to_string(), "20".to_string())]);
        // A repo's own mise config decides its tools, app or not.
        let own = plan_env(&[
            f("talaria.json", "{}"),
            f("mise.toml", "[tools]\nbun = \"1\"\n"),
        ]);
        assert!(own.talaria_app);
        assert!(own.tools.is_empty());
        // Not an app: nothing about the kit, nothing added.
        let plain = plan_env(&[f("package.json", "{}")]);
        assert!(!plain.talaria_app);
        assert_eq!(plain.tools, vec![("node".to_string(), "lts".to_string())]);
        // The scan reads the manifest at the root only.
        assert!(MARKERS.contains(&"talaria.json"));
        assert!(!MARKERS.contains(&"*/talaria.json"));
    }

    #[test]
    fn the_eval_session_file_is_what_the_kit_reads() {
        let v: Value = serde_json::from_str(&eval_session_json("wbeval_00ff", "job-1")).unwrap();
        assert_eq!(v, json!({ "key": "wbeval_00ff", "jobId": "job-1" }));
        assert_eq!(EVAL_SESSION_FILE, ".talaria/eval-session.json");
        assert!(kit_store().starts_with(talaria_workbench_harnesses::HARNESS_XDG_DATA_HOME));
    }

    /// KIT_SH as prepare_env runs it, against a stub `curl` that answers with
    /// a kit tarball built here and a stub `bun` that counts installs.
    #[cfg(unix)]
    fn run_kit_sh(
        scratch: &std::path::Path,
        status: &str,
        session: &str,
    ) -> (String, String, bool) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let exe = |p: &std::path::Path, body: &str| {
            std::fs::write(p, body).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let fake = scratch.join("fakebin");
        let bin = scratch.join("bin");
        let kit_src = scratch.join("kit-src");
        std::fs::create_dir_all(kit_src.join("bin")).unwrap();
        std::fs::create_dir_all(&fake).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        exe(
            &kit_src.join("bin/talaria-app"),
            "#!/bin/sh\necho \"kit test-1 · $1\"\n",
        );
        std::fs::write(kit_src.join("package.json"), "{}").unwrap();
        let tgz = scratch.join("kit.tgz");
        assert!(
            std::process::Command::new("tar")
                .args([
                    "-czf",
                    tgz.to_str().unwrap(),
                    "-C",
                    kit_src.to_str().unwrap(),
                    "."
                ])
                .status()
                .unwrap()
                .success()
        );
        // curl: -o <body> -D <headers> -w '%{http_code}' -H … <url>
        exe(
            &fake.join("curl"),
            &format!(
                "#!/bin/sh\nout=; hdr=\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=$2; shift ;; -D) hdr=$2; shift ;; -H) echo \"$2\" >> {asked} ;; http*) echo \"$1\" >> {asked} ;; esac; shift; done\nif [ {status} = 200 ]; then cp {tgz} \"$out\"; printf 'HTTP/1.1 200 OK\\r\\nX-Talaria-Kit-Version: test-1\\r\\n\\r\\n' > \"$hdr\"; else echo '{{\"error\":\"unknown agent credential\"}}' > \"$out\"; printf 'HTTP/1.1 {status} X\\r\\n\\r\\n' > \"$hdr\"; fi\nprintf {status}\n",
                asked = scratch.join("asked").display(),
                tgz = tgz.display(),
                status = status,
            ),
        );
        exe(
            &bin.join("bun"),
            &format!(
                "#!/bin/sh\n[ \"$1\" = install ] && echo x >> {} && mkdir -p node_modules\n",
                scratch.join("installs").display()
            ),
        );
        let wd = scratch.join("job");
        std::fs::create_dir_all(wd.join(".git")).unwrap();
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(KIT_SH)
            .arg("sh")
            .args([
                scratch.join("kits").to_str().unwrap(),
                bin.to_str().unwrap(),
                wd.to_str().unwrap(),
            ])
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", fake.display()))
            .env("TALARIA_API_URL", "http://app.test:5307/")
            .env("TALARIA_AGENT_KEY", "tak_agent_own")
            .env("API_SERVER_MODEL_NAME", "dev-agent")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(session.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.success(),
        )
    }

    #[cfg(unix)]
    #[test]
    fn the_kit_installs_once_per_version_as_the_agent_and_lands_on_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("kit");
        let session = eval_session_json("wbeval_0123", "job-1");
        for _ in 0..2 {
            let (out, err, ok) = run_kit_sh(&scratch.0, "200", &session);
            assert!(ok, "{out}{err}");
            assert!(out.contains("=== kit\nkit test-1 · version"), "{out}");
        }
        // Asked as the agent, at the app's own origin.
        let asked = std::fs::read_to_string(scratch.0.join("asked")).unwrap();
        assert!(asked.contains("X-Api-Key: tak_agent_own"), "{asked}");
        assert!(asked.contains("X-Agent-Name: dev-agent"), "{asked}");
        assert!(
            asked.contains("http://app.test:5307/api/sdk/kit.tgz"),
            "{asked}"
        );
        // Installed once for the version, however many jobs ask.
        let installs = std::fs::read_to_string(scratch.0.join("installs")).unwrap();
        assert_eq!(installs.lines().count(), 1);
        let kits = scratch.0.join("kits");
        assert!(kits.join("test-1/.installed").is_file());
        assert!(kits.join("test-1/node_modules").is_dir());
        assert_eq!(
            std::fs::read_link(kits.join("current")).unwrap(),
            kits.join("test-1")
        );
        assert_eq!(
            std::fs::read_link(scratch.0.join("bin/talaria-app")).unwrap(),
            kits.join("current/bin/talaria-app")
        );
        // No download is left behind.
        assert!(std::fs::read_dir(&kits).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".fetch-")
        }));
        // The job's eval session: the agent's own, 0600, out of git.
        let file = scratch.0.join("job/.talaria/eval-session.json");
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), session);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let exclude = std::fs::read_to_string(scratch.0.join("job/.git/info/exclude")).unwrap();
        assert_eq!(exclude.matches(".talaria/eval-session.json").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_kit_says_what_the_instance_answered() {
        let scratch = Scratch::new("kit-refused");
        let (_, err, ok) = run_kit_sh(&scratch.0, "401", "");
        assert!(!ok);
        assert!(err.contains("the instance answered 401"), "{err}");
        assert!(err.contains("unknown agent credential"), "{err}");
        assert!(!scratch.0.join("bin/talaria-app").exists());
        assert!(!scratch.0.join("job/.talaria").exists());
    }
}
