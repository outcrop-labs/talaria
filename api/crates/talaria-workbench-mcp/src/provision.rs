// Provisioning: a job is READY before the agent's first harness turn.
//
// What a Developer Agent used to be handed by start_job was a clone URL and a
// list of chores — clone it, call prepare_env, find a bun omp would start on,
// get omp a writable config dir and a key it could read — and on the live
// fleet every one of those went wrong somewhere, so each job's agent spent its
// first 13 to 75 turns (1M to 5M tokens) repairing its own harness before the
// ticket's first real step, and wrote its fixes down as wrappers the next job
// copied (docs: the 2026-11-04 findings, `release/workbench-provisioning/`).
//
// Now Talaria does the chores, in order, as the agent's user inside its
// sandbox, and checks the result before the agent sees a single command:
//
//   sandbox    — the container answers a command, and which render built it
//   clone      — the repo, on the job's branch, into the job's workdir
//   toolchain  — the repo's declared toolchains and packages (prepare_env)
//   harness    — bun and the PINNED omp, then this job's own harness dir and
//                launcher (talaria_workbench_harnesses::launcher_script)
//   preflight  — the launcher starts omp, its dir is writable, the model is in
//                its catalog, and the gateway accepts the key — no model call
//
// The state lives on the job row (`workbench_jobs.provision`, jsonb) so the
// MCP answers, the ticket strip and the Work view all read one record, and so
// a provision survives the api restarting under it: a run that stops
// heartbeating is reclaimed by the next job_status. A failure is LOUD: the
// state names the failing step and what it printed, the ticket gets an audit
// line, and the agent is given no harness lines to discover the breakage with.

use serde_json::{Map, Value, json};
use sqlx::PgPool;

use talaria_agent_auth::{AgentSubject, epoch_ms_to_iso, now_ms};
use talaria_body::truncate_utf16;
use talaria_fleet_docker::{docker_exec_as, managed_container};
use talaria_workbench_harnesses as wh;

use talaria_git_hosts::GitHost;

use crate::{WorkbenchActor, WorkbenchDeps, devenv, log_ticket};

/// The steps, in the order they run. Wire names.
pub const STEPS: [&str; 5] = ["sandbox", "clone", "toolchain", "harness", "preflight"];

/// A running provision heartbeats this often…
const HEARTBEAT_MS: u64 = 20_000;
/// …and one that has not for this long is dead (its api restarted): the next
/// claim takes it over. Long enough to ride out a slow DB write, short enough
/// that a job_status a few minutes later restarts it.
const STALE_AFTER_MS: i64 = 90_000;

/// How long `git clone` may take: a large monorepo on a slow link.
const CLONE_TIMEOUT_MS: u64 = 15 * 60_000;
const PREFLIGHT_TIMEOUT_MS: u64 = 2 * 60_000;

/// What a step's failure keeps of its output, for the job row and the agent.
const OUTPUT_KEEP: usize = 4_000;

/// Clone the job's branch into its workdir, as the agent's user. Positional:
/// `$1` workdir, `$2` clone URL (no credential: the sandbox's git asks Talaria
/// for one), `$3` branch. An existing checkout is NEVER touched — it can hold
/// the agent's uncommitted work from an earlier round — beyond giving it the
/// agent's git identity when it has none. Hermes strips nothing git needs,
/// but commits from a fresh shell still failed live with `unable to
/// auto-detect email address`, so the identity lives in the repo's own
/// config, from the author the render stamped on the container.
pub const CLONE_SH: &str = r#"set -eu
dir="$1"; url="$2"; branch="$3"
ident() {
  if [ -n "${GIT_AUTHOR_NAME:-}" ] && ! git -C "$dir" config --local user.name >/dev/null 2>&1; then
    git -C "$dir" config user.name "$GIT_AUTHOR_NAME"
  fi
  if [ -n "${GIT_AUTHOR_EMAIL:-}" ] && ! git -C "$dir" config --local user.email >/dev/null 2>&1; then
    git -C "$dir" config user.email "$GIT_AUTHOR_EMAIL"
  fi
}
if [ -d "$dir/.git" ]; then
  ident
  echo "kept the checkout already there: $(git -C "$dir" rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?') at $(git -C "$dir" rev-parse --short HEAD 2>/dev/null || echo '?')"
  exit 0
fi
if [ -e "$dir" ] && [ -n "$(ls -A "$dir" 2>/dev/null)" ]; then
  echo "$dir exists, is not empty and is not a git checkout, so the repo cannot be cloned into it" >&2
  exit 1
fi
mkdir -p "$dir"
export GIT_TERMINAL_PROMPT=0
if ! out=$(git clone --quiet --branch "$branch" -- "$url" "$dir" 2>&1); then
  printf '%s\n' "$out" | tail -n 15 >&2
  exit 1
fi
ident
echo "cloned $branch at $(git -C "$dir" rev-parse --short HEAD)"
"#;

/// One job, as provisioning needs it.
#[derive(Debug, Clone)]
pub struct ProvisionJob {
    pub id: String,
    pub agent_id: String,
    pub agent_model: String,
    pub department: String,
    pub repo: String,
    pub branch: String,
    pub host_id: String,
    pub task_id: Option<String>,
}

impl ProvisionJob {
    pub fn workdir(&self) -> String {
        format!("{}/{}", crate::teardown::JOBS_ROOT, self.id)
    }

    pub fn session_dir(&self) -> String {
        format!("/opt/data/workbench/sessions/{}", self.id)
    }

    /// A started job's row, by id — `None` for any other status.
    pub async fn load(pg: &PgPool, job_id: &str) -> Result<Option<ProvisionJob>, String> {
        type Row = (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
        );
        let row: Option<Row> = sqlx::query_as(
            "select j.id::text, j.agent_id::text, j.agent_model, d.department, j.repo, j.branch, \
                    j.host_id, j.task_id::text \
             from workbench_jobs j join agent_defs d on d.id = j.agent_id \
             where j.id = $1::uuid and j.status = 'started'",
        )
        .bind(job_id)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("job read: {e}"))?;
        Ok(row.map(|r| ProvisionJob {
            id: r.0,
            agent_id: r.1,
            agent_model: r.2,
            department: r.3,
            repo: r.4,
            branch: r.5,
            host_id: r.6,
            task_id: r.7,
        }))
    }
}

// ── The record ──────────────────────────────────────────────────────────────

/// The provision state as it is stored and put on the wire. Pure, so the
/// shape is pinned by tests.
#[derive(Debug, Clone, Default)]
pub struct Record {
    pub state: String,
    pub step: Option<String>,
    pub steps: Vec<Value>,
    pub error: Option<String>,
    pub output: Option<String>,
    pub warnings: Vec<String>,
    pub harness: Option<Value>,
    pub toolchain: Option<Value>,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
}

impl Record {
    pub fn start(now: i64) -> Record {
        Record {
            state: "running".into(),
            step: Some(STEPS[0].into()),
            started_ms: now,
            ..Default::default()
        }
    }

    pub fn begin(&mut self, step: &str) {
        self.step = Some(step.into());
    }

    pub fn ok(&mut self, step: &str, ms: u64, note: &str) {
        self.steps.push(json!({
            "step": step, "ok": true, "ms": ms, "note": truncate_utf16(note, 400),
        }));
    }

    pub fn fail(&mut self, step: &str, ms: u64, error: &str, output: &str, now: i64) {
        self.steps.push(json!({
            "step": step, "ok": false, "ms": ms, "note": truncate_utf16(error, 400),
        }));
        self.state = "failed".into();
        self.step = Some(step.into());
        self.error = Some(truncate_utf16(error, 1_000).to_string());
        let output = output.trim();
        self.output = (!output.is_empty()).then(|| tail_chars(output, OUTPUT_KEEP));
        self.finished_ms = Some(now);
    }

    pub fn ready(&mut self, now: i64) {
        self.state = "ready".into();
        self.step = None;
        self.finished_ms = Some(now);
    }

    /// Stored and wire shape (camelCase, fixed key order).
    pub fn to_json(&self, heartbeat_ms: i64) -> Value {
        json!({
            "state": self.state,
            "step": self.step,
            "steps": self.steps,
            "error": self.error,
            "output": self.output,
            "warnings": self.warnings,
            "harness": self.harness,
            "toolchain": self.toolchain,
            "startedAt": epoch_ms_to_iso(self.started_ms),
            "finishedAt": self.finished_ms.map(epoch_ms_to_iso),
            "heartbeatMs": heartbeat_ms,
        })
    }
}

/// The last `keep` characters of `s`, on a char boundary, marked when cut.
fn tail_chars(s: &str, keep: usize) -> String {
    let n = s.chars().count();
    if n <= keep {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - keep).collect();
    format!("…{tail}")
}

/// What the agent and the Work view are told about a stored state: the state
/// itself minus bookkeeping, plus `next` — the one sentence of what to do.
pub fn wire(state: &Value) -> Value {
    let mut out: Map<String, Value> = state.as_object().cloned().unwrap_or_default();
    out.remove("heartbeatMs");
    // The full toolchain answer belongs to prepare_env's reply, not to every
    // poll; the steps already carry its one-line note.
    out.remove("toolchain");
    let next = next_step(state);
    out.insert("next".into(), json!(next));
    Value::Object(out)
}

/// The one thing to do about a provision in this state. Pure.
pub fn next_step(state: &Value) -> String {
    match state.get("state").and_then(Value::as_str) {
        Some("ready") => "Ready: run the harness lines exactly as given. Do not clone, install, upgrade or wrap anything — Talaria already did, and checked it.".into(),
        Some("running") => format!(
            "Talaria is provisioning this job (now: {}). Poll job_status until provision.state is ready — about a minute when the toolchain is cached, longer for a first-time toolchain. Do not clone, install or start the harness yourself meanwhile.",
            state.get("step").and_then(Value::as_str).unwrap_or("starting")
        ),
        Some("failed") => {
            let step = state.get("step").and_then(Value::as_str).unwrap_or("?");
            let repo_fixable = step == "toolchain";
            format!(
                "Provisioning FAILED at `{step}`: the platform could not make this job's workbench ready, and it is not yours to repair — do not install, upgrade or work around the harness or toolchain. {}report_problem with the step and output, comment on the ticket, and set it blocked.",
                if repo_fixable {
                    "If the output shows the REPO declares something wrong (a tool or package name or version in its mise.toml or .talaria/workbench.toml), fix that declaration in the checkout and call prepare_env to provision again; otherwise "
                } else {
                    ""
                }
            )
        }
        _ => "Not provisioned yet: job_status starts it.".into(),
    }
}

/// Is `state` a provision that is ready?
pub fn is_ready(state: Option<&Value>) -> bool {
    state.and_then(|s| s.get("state")).and_then(Value::as_str) == Some("ready")
}

// ── Claiming ────────────────────────────────────────────────────────────────

/// When a caller may (re)start a provision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Anything but a provision that is alive right now: start_job and
    /// prepare_env, which ask for a fresh one.
    Fresh,
    /// Only a job never provisioned, or one whose run died: job_status.
    IfMissing,
}

/// Take the job's provision for this process, or learn that someone else has
/// it. One UPDATE, so two api processes (or two calls) can never both run it.
async fn claim(pg: &PgPool, job_id: &str, mode: Claim, rec: &Record) -> Result<bool, String> {
    let now = now_ms();
    let alive_cut = now - STALE_AFTER_MS;
    let sql = match mode {
        Claim::Fresh => {
            "update workbench_jobs set provision = $2::jsonb \
             where id = $1::uuid and status = 'started' \
               and (provision is null or provision->>'state' is distinct from 'running' \
                    or coalesce((provision->>'heartbeatMs')::bigint, 0) < $3::bigint) \
             returning id::text"
        }
        Claim::IfMissing => {
            "update workbench_jobs set provision = $2::jsonb \
             where id = $1::uuid and status = 'started' \
               and (provision is null \
                    or (provision->>'state' = 'running' \
                        and coalesce((provision->>'heartbeatMs')::bigint, 0) < $3::bigint)) \
             returning id::text"
        }
    };
    let got: Option<String> = sqlx::query_scalar(sql)
        .bind(job_id)
        .bind(rec.to_json(now))
        .bind(alive_cut)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("provision claim: {e}"))?;
    Ok(got.is_some())
}

async fn save(pg: &PgPool, job_id: &str, rec: &Record) {
    if let Err(e) =
        sqlx::query("update workbench_jobs set provision = $2::jsonb where id = $1::uuid")
            .bind(job_id)
            .bind(rec.to_json(now_ms()))
            .execute(pg)
            .await
    {
        tracing::warn!("[workbench] provision state of job {job_id} not saved: {e}");
    }
}

/// The job's stored provision, or `None` (never provisioned / unreadable).
pub async fn read(pg: &PgPool, job_id: &str) -> Option<Value> {
    sqlx::query_scalar::<_, Option<Value>>(
        "select provision from workbench_jobs where id = $1::uuid",
    )
    .bind(job_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
    .flatten()
}

/// Start the job's provision when `mode` allows, in the background. Returns
/// whether this call started one (false: one is alive already, or the job is
/// not started).
pub async fn kick(deps: &WorkbenchDeps, job: &ProvisionJob, mode: Claim) -> Result<bool, String> {
    let rec = Record::start(now_ms());
    if !claim(&deps.pg, &job.id, mode, &rec).await? {
        return Ok(false);
    }
    let deps = deps.clone();
    let job = job.clone();
    tokio::spawn(async move {
        run(&deps, &job, rec).await;
    });
    Ok(true)
}

/// Wait for the job's provision to leave `running`, for at most `budget_ms`.
/// Returns the latest stored state either way.
pub async fn wait(pg: &PgPool, job_id: &str, budget_ms: u64) -> Option<Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(budget_ms);
    loop {
        let state = read(pg, job_id).await;
        let running = state
            .as_ref()
            .and_then(|s| s.get("state"))
            .and_then(Value::as_str)
            == Some("running");
        if !running || std::time::Instant::now() >= deadline {
            return state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    }
}

// ── The run ─────────────────────────────────────────────────────────────────

/// Which login the job's harness uses: the gateway (with this default model)
/// or a coding account's subscription.
async fn harness_plan(pg: &PgPool, job: &ProvisionJob) -> (bool, Option<String>) {
    let plan = talaria_coding_accounts::resolve(pg, &job.agent_id, job.task_id.as_deref())
        .await
        .unwrap_or(None);
    match plan {
        Some(p) if !p.is_gateway() => (false, None),
        _ => (true, wh::omp_roles(pg).await.ok().and_then(|r| r.default)),
    }
}

/// Run every step, saving the record as it goes. Never panics out: a step
/// that cannot run is a failed step.
pub async fn run(deps: &WorkbenchDeps, job: &ProvisionJob, mut rec: Record) -> Record {
    let pg = &deps.pg;
    // Heartbeat while it runs, so a dead run is reclaimable and a live one
    // is not.
    let beat = {
        let pg = pg.clone();
        let id = job.id.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(HEARTBEAT_MS)).await;
                let _ = sqlx::query(
                    "update workbench_jobs set provision = jsonb_set(provision, '{heartbeatMs}', to_jsonb($2::bigint)) \
                     where id = $1::uuid and provision->>'state' = 'running'",
                )
                .bind(&id)
                .bind(now_ms())
                .execute(&pg)
                .await;
            }
        })
    };
    let started = std::time::Instant::now();
    let outcome = steps(deps, job, &mut rec).await;
    beat.abort();
    match outcome {
        Ok(()) => rec.ready(now_ms()),
        Err(StepFailure {
            step,
            ms,
            error,
            output,
        }) => {
            let error = match rec.warnings.iter().find(|w| w.starts_with("sandbox: ")) {
                // A stale container is the likeliest cause of whatever broke:
                // say so in the error itself, not only in a warning beside it.
                Some(stale) if step != "toolchain" && step != "clone" => {
                    format!("{error} — and {}", stale.trim_start_matches("sandbox: "))
                }
                _ => error,
            };
            rec.fail(&step, ms, &error, &output, now_ms())
        }
    }
    save(pg, &job.id, &rec).await;
    let by = WorkbenchActor::Agent(AgentSubject::Model(job.agent_model.clone()));
    let line = match rec.state.as_str() {
        "ready" => format!(
            "Workbench ready for {} on {} in {} s: {}",
            job.repo,
            job.branch,
            started.elapsed().as_secs(),
            rec.steps
                .iter()
                .filter_map(|s| s.get("note").and_then(Value::as_str))
                .filter(|n| !n.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        ),
        _ => format!(
            "Workbench provisioning FAILED at {}: {}",
            rec.step.as_deref().unwrap_or("?"),
            rec.error.as_deref().unwrap_or("no reason")
        ),
    };
    log_ticket(pg, job.task_id.as_deref(), &by, truncate_utf16(&line, 900)).await;
    rec
}

struct StepFailure {
    step: String,
    ms: u64,
    error: String,
    output: String,
}

fn failure(step: &str, t: std::time::Instant, error: String, output: String) -> StepFailure {
    StepFailure {
        step: step.into(),
        ms: elapsed_ms(t),
        error,
        output,
    }
}

fn elapsed_ms(t: std::time::Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Split an exec error into a one-line reason and the full text. The reason
/// is the first line that says what went wrong (`mise ERROR Failed to
/// install core:node@0.0.1: … 404`, git's `fatal: …`), not the last line,
/// which for mise is its "run with --verbose" footer.
pub fn reason_and_output(e: &str) -> (String, String) {
    let lines: Vec<&str> = e.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let telling = lines.iter().find(|l| {
        let low = l.to_lowercase();
        (low.contains("error") || low.starts_with("fatal:") || low.contains("failed"))
            && !low.contains("--verbose")
            && !low.contains("error version:")
            && !low.starts_with("mise install failed in ")
    });
    let reason = telling
        .or(lines.last())
        .copied()
        .unwrap_or("no output")
        .to_string();
    (reason, e.to_string())
}

async fn steps(
    deps: &WorkbenchDeps,
    job: &ProvisionJob,
    rec: &mut Record,
) -> Result<(), StepFailure> {
    let pg = &deps.pg;
    let container = managed_container(pg, &job.department).await;
    let workdir = job.workdir();

    // sandbox
    rec.begin("sandbox");
    save(pg, &job.id, rec).await;
    let t = std::time::Instant::now();
    let shape = docker_exec_as(
        &container,
        Some(devenv::AGENT_USER),
        &[
            "sh",
            "-c",
            "printf '%s' \"$(printenv \"$1\" 2>/dev/null)\"",
            "sh",
            wh::SANDBOX_SHAPE_ENV,
        ],
        None,
        30_000,
    )
    .await
    .map_err(|e| {
        failure(
            "sandbox",
            t,
            format!(
                "Talaria cannot run a command in this agent's sandbox ({container}), so nothing can be provisioned: {}. The container needs an operator — restart or roll the agent",
                e.trim()
            ),
            e,
        )
    })?;
    let stale = wh::sandbox_stale_reason(&shape.0);
    if let Some(why) = &stale {
        rec.warnings.push(format!("sandbox: {why}"));
    }
    rec.ok(
        "sandbox",
        elapsed_ms(t),
        if stale.is_some() {
            "sandbox answers, but was created by an older render"
        } else {
            "sandbox answers and is current"
        },
    );

    // clone
    rec.begin("clone");
    save(pg, &job.id, rec).await;
    let t = std::time::Instant::now();
    let url = match crate::hosts::host(pg, &deps.sb, &job.host_id).await {
        Ok(host) => host.clone_url(&job.repo),
        Err(e) => {
            return Err(failure(
                "clone",
                t,
                format!("the git host `{}` is not usable: {e}", job.host_id),
                String::new(),
            ));
        }
    };
    let (out, _) = docker_exec_as(
        &container,
        Some(devenv::AGENT_USER),
        &["sh", "-c", CLONE_SH, "sh", &workdir, &url, &job.branch],
        None,
        CLONE_TIMEOUT_MS,
    )
    .await
    .map_err(|e| {
        let (why, out) = reason_and_output(&e);
        failure(
            "clone",
            t,
            format!(
                "could not clone {} ({}) into {workdir}: {why}",
                job.repo, job.branch
            ),
            out,
        )
    })?;
    rec.ok("clone", elapsed_ms(t), out.trim());

    // toolchain
    rec.begin("toolchain");
    save(pg, &job.id, rec).await;
    let t = std::time::Instant::now();
    let mut toolchain = devenv::prepare_env(pg, &job.department, &workdir)
        .await
        .map_err(|e| {
            let (why, out) = reason_and_output(&e);
            failure(
                "toolchain",
                t,
                format!("the repo's toolchain did not install: {why}"),
                out,
            )
        })?;
    let env_vars = devenv::deliver_job_env(pg, &deps.sb, &job.department, &workdir, &job.repo)
        .await
        .map_err(|e| {
            failure(
                "toolchain",
                t,
                format!("the project env could not be written: {e}"),
                String::new(),
            )
        })?;
    if let Some(obj) = toolchain.as_object_mut() {
        obj.insert(
            "projectEnv".into(),
            devenv::job_env_answer(&workdir, env_vars),
        );
    }
    rec.ok("toolchain", elapsed_ms(t), &toolchain_note(&toolchain));
    let talaria_app = toolchain["talariaApp"] == json!(true);
    rec.toolchain = Some(toolchain);

    // harness
    rec.begin("harness");
    save(pg, &job.id, rec).await;
    let t = std::time::Instant::now();
    let runtime = devenv::ensure_harness_runtime(pg, &job.department)
        .await
        .map_err(|e| {
            let (why, out) = reason_and_output(&e);
            failure("harness", t, why, out)
        })?;
    let (gateway, model) = harness_plan(pg, job).await;
    let launcher = wh::job_launcher(&job.id);
    let text = wh::launcher_script(&job.id, &workdir, &runtime.bun_path, gateway);
    docker_exec_as(
        &container,
        Some(devenv::AGENT_USER),
        &[
            "sh",
            "-c",
            &wh::job_harness_script(),
            "sh",
            &wh::job_harness_dir(&job.id),
            &job.session_dir(),
        ],
        Some(&text),
        30_000,
    )
    .await
    .map_err(|e| {
        failure(
            "harness",
            t,
            format!("could not lay out this job's harness dir: {}", e.trim()),
            e,
        )
    })?;
    // A Talaria app's SDK kit runs on the harness's bun, just ensured.
    if talaria_app {
        let session = crate::record_eval_session(pg, &job.id, job.task_id.as_deref()).await;
        let kit = devenv::install_app_kit(pg, &job.department, &workdir, session.as_deref())
            .await
            .unwrap_or_else(|e| json!({ "ok": false, "error": e }));
        if kit.get("ok") == Some(&json!(false)) {
            rec.warnings.push(format!(
                "appKit: {}",
                kit.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("not installed")
            ));
        }
        if let Some(tc) = rec.toolchain.as_mut().and_then(Value::as_object_mut) {
            tc.insert("appKit".into(), kit);
        }
    }
    rec.harness = Some(json!({
        "omp": runtime.omp,
        "bun": runtime.bun,
        "launcher": launcher,
        "agentDir": wh::job_agent_dir(&job.id),
        "sessionDir": job.session_dir(),
        "gateway": gateway,
        "model": model,
    }));
    rec.ok(
        "harness",
        elapsed_ms(t),
        &format!("{} on bun {}", runtime.omp, runtime.bun),
    );

    // preflight
    rec.begin("preflight");
    save(pg, &job.id, rec).await;
    let t = std::time::Instant::now();
    let model_check = if gateway {
        match model.as_deref() {
            Some(m) => m.to_string(),
            None => {
                return Err(failure(
                    "preflight",
                    t,
                    "no Workbench model role resolves to a gateway model, so omp has nothing to run on — an admin sets the code roles on /models".into(),
                    String::new(),
                ));
            }
        }
    } else {
        String::new()
    };
    let verdict = preflight(
        &container,
        &launcher,
        &wh::job_agent_dir(&job.id),
        &model_check,
        &runtime.bun_path,
    )
    .await;
    match verdict {
        Ok(v) => {
            rec.ok(
                "preflight",
                elapsed_ms(t),
                &format!(
                    "{v} starts; agent dir writable{}",
                    if gateway {
                        "; model listed; gateway accepts the key"
                    } else {
                        ""
                    }
                ),
            );
            Ok(())
        }
        Err((check, detail)) => Err(failure(
            "preflight",
            t,
            format!(
                "the harness check `{check}` failed: {}",
                first_line(&detail)
            ),
            detail,
        )),
    }
}

/// One line for the toolchain step: what resolved (mise's `ls --current`,
/// tool and version), else where the tools came from.
pub fn toolchain_note(toolchain: &Value) -> String {
    let resolved: Vec<String> = toolchain["resolved"]
        .as_str()
        .unwrap_or("")
        .lines()
        .filter_map(|l| {
            let mut cols = l.split_whitespace();
            Some(format!("{} {}", cols.next()?, cols.next()?))
        })
        .collect();
    if !resolved.is_empty() {
        return resolved.join(", ");
    }
    toolchain["toolsFrom"]
        .as_str()
        .unwrap_or("nothing to install")
        .to_string()
}

fn first_line(s: &str) -> &str {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no detail")
}

/// Run the preflight in the shell the agent's terminal gives a command —
/// Hermes's stripped names and every sealed `CONFIG_*` unset — so what passes
/// here passes for the agent.
pub async fn preflight(
    container: &str,
    launcher: &str,
    agent_dir: &str,
    model: &str,
    bun_path: &str,
) -> Result<String, (String, String)> {
    let script = wh::preflight_script(bun_path);
    let stripped_names = talaria_agent_secrets::hermes_stripped_names();
    let mut extra: Vec<&str> = vec![launcher, agent_dir, model];
    extra.extend(stripped_names.iter().map(String::as_str));
    let argv = crate::agent_terminal_argv(&script, &extra);
    match talaria_fleet_docker::docker_exec(container, &argv, PREFLIGHT_TIMEOUT_MS).await {
        Ok((stdout, stderr)) => wh::preflight_verdict(&format!("{stdout}\n{stderr}")),
        Err(e) => Err((
            "preflight".into(),
            format!("the preflight could not run: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_runs_then_lands_ready_with_every_step() {
        let mut r = Record::start(1_000);
        assert_eq!(r.to_json(1_000)["state"], "running");
        assert_eq!(r.to_json(1_000)["step"], "sandbox");
        for s in STEPS {
            r.begin(s);
            r.ok(s, 5, "fine");
        }
        r.ready(2_000);
        let j = r.to_json(2_000);
        assert_eq!(j["state"], "ready");
        assert_eq!(j["step"], Value::Null);
        assert_eq!(j["steps"].as_array().map(Vec::len), Some(STEPS.len()));
        assert!(j["finishedAt"].is_string());
        assert!(is_ready(Some(&j)));
    }

    #[test]
    fn a_failed_record_names_its_step_and_keeps_the_tail_of_the_output() {
        let mut r = Record::start(1_000);
        r.begin("toolchain");
        let long = "x".repeat(OUTPUT_KEEP + 50) + "\nmise ERROR rust@9.9 not found";
        r.fail(
            "toolchain",
            7,
            "the repo's toolchain did not install: mise ERROR",
            &long,
            3_000,
        );
        let j = r.to_json(3_000);
        assert_eq!(j["state"], "failed");
        assert_eq!(j["step"], "toolchain");
        assert!(j["error"].as_str().unwrap().contains("did not install"));
        let out = j["output"].as_str().unwrap();
        assert!(
            out.ends_with("rust@9.9 not found"),
            "the end is what matters"
        );
        assert!(out.starts_with('…'), "and a cut is marked");
        assert!(out.chars().count() <= OUTPUT_KEEP + 1);
        assert!(!is_ready(Some(&j)));
    }

    #[test]
    fn next_says_what_to_do_in_every_state_and_never_to_repair() {
        let ready = next_step(&json!({ "state": "ready" }));
        assert!(ready.contains("exactly as given"), "{ready}");
        let running = next_step(&json!({ "state": "running", "step": "toolchain" }));
        assert!(
            running.contains("Poll job_status") && running.contains("toolchain"),
            "{running}"
        );
        let failed = next_step(&json!({ "state": "failed", "step": "preflight" }));
        assert!(
            failed.contains("FAILED at `preflight`") && failed.contains("report_problem"),
            "{failed}"
        );
        assert!(failed.contains("not yours to repair"), "{failed}");
        assert!(
            !failed.contains("prepare_env"),
            "only a repo declaration is the agent's to fix"
        );
        let repo = next_step(&json!({ "state": "failed", "step": "toolchain" }));
        assert!(
            repo.contains(".talaria/workbench.toml") && repo.contains("prepare_env"),
            "{repo}"
        );
        assert!(next_step(&Value::Null).contains("job_status"));
    }

    #[test]
    fn the_wire_drops_bookkeeping_and_adds_next() {
        let mut r = Record::start(1_000);
        r.toolchain = Some(json!({ "tools": ["rust@1.97"] }));
        let w = wire(&r.to_json(5));
        assert!(w.get("heartbeatMs").is_none());
        assert!(w.get("toolchain").is_none());
        assert!(w["next"].as_str().unwrap().contains("provisioning"));
        assert_eq!(w["state"], "running");
    }

    #[test]
    fn the_reason_is_the_line_that_says_what_broke() {
        let mise = "mise install failed in /w: mise by @jdx – installing 2 tools\nmise ⇢ bun@1.4.4 already installed\n  node@0.0.1  downloading\nmise ✗ node@0.0.1 5.3s · failed: HTTP status client error (404 Not Found)\nmise ERROR Failed to install core:node@0.0.1: 404\nmise ERROR Version: 2026.11.0\nmise ERROR Run with --verbose or MISE_VERBOSE=1 for more information";
        let (why, out) = reason_and_output(mise);
        assert_eq!(
            why,
            "mise ✗ node@0.0.1 5.3s · failed: HTTP status client error (404 Not Found)"
        );
        assert_eq!(out, mise, "the whole text stays as the output");
        let git = "Cloning into '/w'...\nfatal: repository 'http://h/x.git/' not found";
        assert_eq!(
            reason_and_output(git).0,
            "fatal: repository 'http://h/x.git/' not found"
        );
        assert_eq!(reason_and_output("just one line").0, "just one line");
        assert_eq!(reason_and_output("").0, "no output");
    }

    #[test]
    fn the_toolchain_note_says_what_resolved() {
        let tc = json!({
            "toolsFrom": "the repo's mise config",
            "resolved": "bun   1.3.14  ~/jobs/x/mise.toml  1.3.14\nnode  22.11.0 ~/jobs/x/mise.toml 22\n",
        });
        assert_eq!(toolchain_note(&tc), "bun 1.3.14, node 22.11.0");
        assert_eq!(
            toolchain_note(&json!({ "toolsFrom": "nothing to install", "resolved": "" })),
            "nothing to install"
        );
    }

    #[test]
    fn the_clone_never_touches_an_existing_checkout() {
        // The early exit for .git comes before anything that writes.
        let keep = CLONE_SH.find("if [ -d \"$dir/.git\" ]").unwrap();
        let clone = CLONE_SH.find("git clone").unwrap();
        assert!(keep < clone);
        let kept = &CLONE_SH[keep..CLONE_SH[keep..].find("fi\n").unwrap() + keep];
        assert!(kept.contains("exit 0"));
        for verb in [
            "reset", "checkout", "pull", "clean", "stash", "switch", "restore",
        ] {
            assert!(
                !CLONE_SH.contains(&format!("git -C \"$dir\" {verb}"))
                    && !CLONE_SH.contains(&format!("git {verb} ")),
                "{verb} would touch the agent's tree"
            );
        }
        // The branch is the job's, so the checkout is never left on main.
        assert!(CLONE_SH.contains("--branch \"$branch\""));
        assert!(CLONE_SH.contains("GIT_TERMINAL_PROMPT=0"));
    }

    #[cfg(unix)]
    #[test]
    fn the_clone_script_clones_the_branch_and_keeps_a_rerun_harmless() {
        use std::process::Command;
        let have_git = Command::new("git").arg("--version").output().is_ok();
        if !have_git {
            return;
        }
        let tmp = std::env::temp_dir().join(format!("wbprov-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let origin = tmp.join("origin");
        let run = |args: &[&str], dir: &std::path::Path| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        std::fs::create_dir_all(&origin).unwrap();
        run(&["init", "-q", "-b", "main"], &origin);
        std::fs::write(origin.join("README"), "hi").unwrap();
        run(&["add", "README"], &origin);
        run(&["commit", "-qm", "init"], &origin);
        run(&["branch", "agent/x"], &origin);
        let work = tmp.join("jobs/abc");
        let sh = |dir: &std::path::Path| {
            Command::new("sh")
                .args(["-c", CLONE_SH, "sh"])
                .arg(dir)
                .arg(origin.to_str().unwrap())
                .arg("agent/x")
                .env("GIT_AUTHOR_NAME", "Doug")
                .env("GIT_AUTHOR_EMAIL", "doug@agents")
                .output()
                .unwrap()
        };
        let out = sh(&work);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("cloned agent/x at "));
        let head = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&work)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "agent/x");
        let email = Command::new("git")
            .args(["config", "--local", "user.email"])
            .current_dir(&work)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&email.stdout).trim(), "doug@agents");
        // Work in progress survives a second provision.
        std::fs::write(work.join("wip.txt"), "uncommitted").unwrap();
        let again = sh(&work);
        assert!(again.status.success());
        assert!(String::from_utf8_lossy(&again.stdout).starts_with("kept the checkout"));
        assert_eq!(
            std::fs::read_to_string(work.join("wip.txt")).unwrap(),
            "uncommitted"
        );
        // A non-empty dir that is not a checkout is refused, loudly.
        let junk = tmp.join("jobs/junk");
        std::fs::create_dir_all(&junk).unwrap();
        std::fs::write(junk.join("f"), "x").unwrap();
        let refused = sh(&junk);
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains("not a git checkout"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
