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
//     The agent itself never gets root.
//   · mise's shims on PATH for every login shell, so `cargo` / `bun` / `go`
//     resolve to the repo's pinned versions by directory, with no activation
//     step the agent can forget.
//
// .talaria/workbench.toml (all optional):
//
//     apt = ["libssl-dev", "protobuf-compiler"]
//     [tools]              # extra mise tools, merged over detection
//     protoc = "28"

use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::OnceLock;

use regex::Regex;
use talaria_fleet_docker::{docker_exec, docker_exec_opt, managed_container};

/// Files the scan reads, relative to the repo root. `*/` entries are one
/// level down, which is where monorepos keep their per-language roots
/// (Talaria's own rust-toolchain.toml lives in api/).
const MARKERS: &[&str] = &[
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
const SCAN_SH: &str = r#"
wd="$1"; shift
root=""
if [ -d "$wd/.git" ]; then root="$wd"; else
  for d in "$wd"/*/; do if [ -d "$d.git" ]; then root="${d%/}"; break; fi; done
fi
if [ -z "$root" ]; then echo NOREPO; exit 0; fi
printf 'ROOT %s\n' "$root"
printf 'OWNER %s\n' "$(stat -c %U "$root")"
cd "$root" || exit 1
for pat in "$@"; do
  for f in $pat; do
    [ -f "$f" ] || continue
    printf '\036%s\n' "$f"
    head -c 8192 "$f"
    printf '\n'
  done
done
"#;

/// Root half: OS packages, and nothing else. Only names the repo declared
/// and plan_env validated reach it, and a repo that declares none never
/// runs it. $1.. = apt packages.
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

/// User half, as the checkout's owner: the mise binary, the shims line in
/// the shell profiles, the generated config kept out of git, every tool the
/// repo's mise configs name, and what resolved. $1 = home, $2 = repo root,
/// $3 = "1" when a mise.local.toml was written.
const USER_SH: &str = r#"
set -e
home="$1"; root="$2"; wrote="$3"
export HOME="$home" MISE_YES=1 MISE_TRUSTED_CONFIG_PATHS=/opt/data/workbench/jobs
bin="$home/.local/bin"
export PATH="$bin:$PATH"
if ! command -v mise >/dev/null 2>&1; then
  mkdir -p "$bin"
  curl -fsSL https://mise.run | MISE_INSTALL_PATH="$bin/mise" sh >/dev/null
  command -v mise >/dev/null 2>&1 || { echo "could not download mise (https://mise.run)" >&2; exit 1; }
fi
# Rewritten on every call (old block out, current block in), so a fix to the
# block reaches containers that already have an older one. Written back in
# place (not replaced) so the file keeps its mode.
for rc in "$home/.profile" "$home/.bashrc"; do
  touch "$rc"
  tmp=$(mktemp)
  awk '/^# >>> talaria workbench: mise >>>$/{skip=1} !skip{print} /^# <<< talaria workbench: mise <<<$/{skip=0}' "$rc" > "$tmp"
  cat >> "$tmp" <<'EOF'
# >>> talaria workbench: mise >>>
export MISE_YES=1
export MISE_TRUSTED_CONFIG_PATHS=/opt/data/workbench/jobs
# mise's own lookup: MISE_DATA_DIR, else XDG_DATA_HOME/mise (the render sets
# XDG_DATA_HOME on workbenches), else ~/.local/share/mise. Always to the
# FRONT, even if already present: a stock .profile puts ~/.local/bin ahead
# after .bashrc ran, and a hand-installed tool there must not shadow the pin.
export PATH="${MISE_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/mise}/shims:$PATH"
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
echo "=== resolved"
mise ls --current 2>&1
"#;

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
    /// Declared entries that failed validation, told back to the agent.
    pub rejected: Vec<String>,
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

    // The repo's explicit extras apply on top of either source.
    if let Some((_, text)) = files.iter().find(|(p, _)| p == ".talaria/workbench.toml") {
        match text.parse::<toml_edit::DocumentMut>() {
            Ok(doc) => {
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
}

/// Parses SCAN_SH's output. None = no git checkout under the workdir yet.
pub fn parse_scan(out: &str) -> Option<Scan> {
    let (head, rest) = out.split_once('\u{1e}').unwrap_or((out, ""));
    let mut root = None;
    let mut owner = String::new();
    for line in head.lines() {
        if line.trim() == "NOREPO" {
            return None;
        }
        if let Some(r) = line.strip_prefix("ROOT ") {
            root = Some(r.to_string());
        } else if let Some(o) = line.strip_prefix("OWNER ") {
            owner = o.to_string();
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
    })
}

/// The verb's body. `workdir` comes from a validated job id.
pub async fn prepare_env(pg: &PgPool, department: &str, workdir: &str) -> Result<Value, String> {
    let container = managed_container(pg, department).await;

    let mut scan_cmd: Vec<&str> = vec!["sh", "-c", SCAN_SH, "sh", workdir];
    scan_cmd.extend(MARKERS.iter().copied());
    let (out, _) = docker_exec(&container, &scan_cmd, 60_000).await?;
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
        "rejected": plan.rejected,
        "resolved": resolved,
        "usage": "Toolchains resolve through mise shims by directory in every NEW login shell (bash -l). In a shell opened before this call, run `. ~/.profile` first, or prefix a command with `mise exec --`. Do not install toolchains yourself: add them to the repo's mise.toml or .talaria/workbench.toml and call prepare_env again.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, body: &str) -> (String, String) {
        (path.to_string(), body.to_string())
    }

    #[test]
    fn a_repo_mise_config_is_used_as_is() {
        let plan = plan_env(&[
            f("mise.toml", "[tools]\nnode = \"22\"\n"),
            f("Cargo.toml", "[package]\n"),
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
            ]
        );
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
        let out = "ROOT /opt/data/workbench/jobs/x/repo\nOWNER hermes\n\u{1e}package.json\n{}\n\n\u{1e}api/rust-toolchain.toml\nchannel = \"1\"\n";
        let scan = parse_scan(out).unwrap();
        assert_eq!(scan.root, "/opt/data/workbench/jobs/x/repo");
        assert_eq!(scan.owner, "hermes");
        assert_eq!(scan.files.len(), 2);
        assert_eq!(scan.files[1].0, "api/rust-toolchain.toml");
        assert!(parse_scan("NOREPO\n").is_none());
    }
}
