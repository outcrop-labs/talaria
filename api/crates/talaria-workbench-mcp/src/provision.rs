// Provisioning: a job is READY before the agent's first harness turn.
//
// What a Developer Agent used to be handed by start_job was a clone URL and a
// list of chores — clone it, call prepare_env, find a bun omp would start on,
// get omp a writable config dir and a key it could read — and on the live
// fleet every one of those went wrong somewhere, so each job's agent spent its
// first 13 to 75 turns (1M to 5M tokens) repairing its own harness before the
// ticket's first real step, and wrote its fixes down as wrappers the next job
// copied.
//
// Now Talaria does the chores, in order, as the agent's user inside its
// sandbox, and checks the result before the agent sees a single command:
//
//   sandbox    — the container answers a command, and which render built it
//   clone      — the repo, on the job's branch, into the job's workdir
//   toolchain  — the repo's declared toolchains and packages (prepare_env)
//   harness    — bun and the PINNED omp, then this job's own harness dir and
//                launcher (talaria_workbench_harnesses::launcher_script)
//   preflight  — the launcher starts omp, its dir is writable, the model the
//                job's lines name is in its catalog, and the gateway accepts
//                the key — no model call
//
// The state lives on the job row (`workbench_jobs.provision`, jsonb) so the
// MCP answers, the ticket strip and the Work view all read one record, and so
// a provision survives the api restarting under it: a run that stops
// heartbeating is taken over by the next job_status. A failure is LOUD: the
// state names the failing step and what it printed (scrubbed of anything
// credential-shaped — board members read it), the ticket gets an audit line,
// and the agent is given no harness lines to discover the breakage with.
//
// THE WIRE (`provision` on GET /api/workbench/jobs and the MCP answers) is a
// contract the Work view reads: `state` (running | ready | failed), `step`,
// `steps[]` ({step, ok, ms, note, attempts?}), `error`, `output`, `warnings`,
// `harness` ({omp, bun, launcher, agentDir, sessionDir, gateway, model}),
// `startedAt`, `finishedAt`. Keep those names.

use serde_json::{Map, Value, json};
use sqlx::PgPool;

use talaria_agent_auth::{AgentSubject, epoch_ms_to_iso, now_ms};
use talaria_body::truncate_utf16;
use talaria_fleet_docker::{docker_exec_as, managed_container};
use talaria_git_hosts::GitHost;
use talaria_workbench_harnesses as wh;

use crate::{WorkbenchActor, WorkbenchDeps, devenv, log_ticket};

/// The steps, in the order they run. Wire names.
pub const STEPS: [&str; 5] = ["sandbox", "clone", "toolchain", "harness", "preflight"];

/// A running provision heartbeats this often…
const HEARTBEAT_MS: u64 = 20_000;
/// …and one that has not for this long is dead (its api restarted): the next
/// claim takes it over. Long enough to ride out a slow DB write, short enough
/// that a job_status a few minutes later restarts it.
pub const STALE_AFTER_MS: i64 = 90_000;

/// How long `git clone` may take: a large monorepo on a slow link.
const CLONE_TIMEOUT_MS: u64 = 15 * 60_000;
const PREFLIGHT_TIMEOUT_MS: u64 = 2 * 60_000;

/// What a step's failure keeps of its output, for the job row and the agent.
const OUTPUT_KEEP: usize = 4_000;

/// A step that fails is tried once more — a git host blip, a registry
/// timeout, the install lock held by a sibling job — unless the attempt that
/// failed already took this long: a wall-clock timeout is not worth doubling.
const RETRY_UNDER_MS: u64 = 5 * 60_000;
const RETRY_PAUSE_MS: u64 = 10_000;

/// Clone the job's branch into its workdir, as the agent's user. Positional:
/// `$1` workdir, `$2` clone URL (no credential: the sandbox's git asks Talaria
/// for one), `$3` branch.
///
/// The clone goes to a temp dir beside the workdir and is RENAMED into place
/// only once it has a HEAD, so a clone that was cut off (the exec's timeout
/// kills the docker CLI, not the git inside the container) never leaves a
/// tree that reads as a checkout. A stray clone from an earlier attempt is
/// killed and removed first.
///
/// An existing checkout is NEVER touched — it can hold the agent's
/// uncommitted work from an earlier round — beyond giving it the agent's git
/// identity when it has none. One with no HEAD (cut off before this script
/// cloned to a temp dir) is moved aside, not deleted, and cloned again.
/// Commits from a fresh shell failed live with `unable to auto-detect email
/// address`, so the identity lives in the repo's own config, from the author
/// the render stamped on the container.
pub const CLONE_SH: &str = r#"set -eu
dir="$1"; url="$2"; branch="$3"
parent=$(dirname "$dir"); base=$(basename "$dir")
ident() {
  if [ -n "${GIT_AUTHOR_NAME:-}" ] && ! git -C "$dir" config --local user.name >/dev/null 2>&1; then
    git -C "$dir" config user.name "$GIT_AUTHOR_NAME"
  fi
  if [ -n "${GIT_AUTHOR_EMAIL:-}" ] && ! git -C "$dir" config --local user.email >/dev/null 2>&1; then
    git -C "$dir" config user.email "$GIT_AUTHOR_EMAIL"
  fi
}
# An earlier attempt's clone, still running or cut off: stop it, drop it.
for p in /proc/[0-9]*; do
  [ "${p#/proc/}" = "$$" ] && continue
  tr '\0' ' ' < "$p/cmdline" 2>/dev/null | grep -qF "$parent/.$base.clone." && kill -KILL "${p#/proc/}" 2>/dev/null
done
rm -rf "$parent/.$base.clone."* 2>/dev/null || true
if [ -d "$dir/.git" ]; then
  if git -C "$dir" rev-parse --verify -q HEAD >/dev/null 2>&1; then
    ident
    echo "kept the checkout already there: $(git -C "$dir" rev-parse --abbrev-ref HEAD) at $(git -C "$dir" rev-parse --short HEAD)"
    exit 0
  fi
  aside="$parent/.$base.broken.$(date +%s)"
  mv "$dir" "$aside"
  echo "moved a checkout with no HEAD aside to $aside" >&2
fi
if [ -e "$dir" ] && [ -n "$(ls -A "$dir" 2>/dev/null)" ]; then
  echo "$dir exists, is not empty and is not a git checkout, so the repo cannot be cloned into it" >&2
  exit 1
fi
mkdir -p "$parent"
tmp="$parent/.$base.clone.$$"
export GIT_TERMINAL_PROMPT=0
if ! out=$(git clone --quiet --branch "$branch" -- "$url" "$tmp" 2>&1); then
  printf '%s\n' "$out" | tail -n 15 >&2
  rm -rf "$tmp"
  exit 1
fi
if ! git -C "$tmp" rev-parse --verify -q HEAD >/dev/null 2>&1; then
  rm -rf "$tmp"
  echo "the clone of $branch finished with no HEAD" >&2
  exit 1
fi
# The workdir must still be empty (or absent): something that appeared in it
# while the clone ran is not overwritten, and the clone is never nested in it.
if [ -d "$dir" ] && ! rmdir "$dir" 2>/dev/null; then
  rm -rf "$tmp"
  echo "$dir gained files while the repo was cloning, so the clone was not put there" >&2
  exit 1
fi
mv -T "$tmp" "$dir"
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

// ── Which model the job's lines run ─────────────────────────────────────────

/// The model a job's harness lines name, resolved ONCE for both the lines
/// (`harness_answer`) and the preflight that vouches for them: the ticket's
/// pinned plan, else the agent's own plan, else the org's Workbench roles.
/// A gateway plan that fills no default falls back to the org's default, so
/// a line is never handed out with `<model>` in it.
pub struct JobModel {
    /// The harness authenticates to the Talaria gateway (not a subscription).
    pub gateway: bool,
    /// The default role's model as the gateway's catalog lists it (no
    /// provider prefix) — what the preflight looks for in `models.yml`.
    pub model: Option<String>,
    /// The `--model` argument, provider-qualified.
    pub arg: Option<String>,
    /// Inline role assignments for a coding-account plan.
    pub env_prefix: String,
    pub roles: wh::OmpRoles,
    pub plan: Option<talaria_coding_accounts::Resolved>,
}

pub async fn job_model(
    pg: &PgPool,
    agent_id: &str,
    task_id: Option<&str>,
) -> Result<JobModel, String> {
    let roles = wh::omp_roles(pg).await?;
    let plan = talaria_coding_accounts::resolve(pg, agent_id, task_id)
        .await
        .unwrap_or(None);
    Ok(match &plan {
        Some(p) => {
            let gateway = p.is_gateway();
            let model = p
                .model_for("default")
                .map(str::to_string)
                .or_else(|| gateway.then(|| roles.default.clone()).flatten());
            let arg = p.model_arg("default").or_else(|| {
                gateway
                    .then(|| roles.default.as_deref().map(wh::model_arg))
                    .flatten()
            });
            let env_prefix = wh::role_env_prefix(
                &p.roles
                    .iter()
                    .filter_map(|(role, _)| p.model_arg(role).map(|a| (role.clone(), a)))
                    .collect::<Vec<_>>(),
            );
            JobModel {
                gateway,
                model,
                arg,
                env_prefix,
                roles,
                plan,
            }
        }
        None => JobModel {
            gateway: true,
            model: roles.default.clone(),
            arg: roles.default.as_deref().map(wh::model_arg),
            env_prefix: String::new(),
            roles,
            plan: None,
        },
    })
}

// ── Scrubbing what a step printed ───────────────────────────────────────────

/// Everything a step printed goes through this before it is stored or shown:
/// board members read the record, and the output is mise's, apt's, git's and
/// a repo's own install hooks' — any of which can echo a token (`401 …
/// Authorization: Bearer …`, a URL with userinfo). `known` holds the secret
/// values Talaria handed this job and fleet, each taken out by value.
///
/// WHICH values: only ones a secret would be. A value is taken out by value
/// when its key is credential-named (`*KEY*`, `*TOKEN*`, `*SECRET*`,
/// `*PASSWORD*`, a DSN, …) or the value itself is credential-shaped, AND it
/// is at least [`KNOWN_MIN_CHARS`] long — or it is a per-agent secret, which
/// is a secret whatever its name. A project env's `CI=true` or
/// `NODE_ENV=production` must not turn every `true` or `production` in a
/// toolchain answer into a redaction.
#[derive(Default, Clone)]
pub struct Scrub {
    known: Vec<String>,
}

/// The shortest value [`Scrub`] takes out by value.
pub const KNOWN_MIN_CHARS: usize = 8;

impl Scrub {
    pub fn with(known: Vec<String>) -> Scrub {
        Scrub { known }
    }

    pub fn text(&self, s: &str) -> String {
        talaria_error::scrub_secrets(&talaria_agent_secrets::scrub_text(s, &self.known))
    }

    /// A JSON value with every STRING in it scrubbed, keys and the rest of
    /// its shape untouched — never the serialized text, where a redaction can
    /// land on a `true` or across a quote and break the JSON.
    pub fn value(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::Object(o) => {
                Value::Object(o.iter().map(|(k, x)| (k.clone(), self.value(x))).collect())
            }
            other => other.clone(),
        }
    }

    /// The values to take out, from what the sandbox was given: the repo's
    /// project env (dotenv bytes, as `env_file` writes them), the fleet's
    /// `.env`, and each agent's `secrets.env`. Pure — [`Scrub::for_job`]
    /// reads the sources.
    pub fn from_sources(
        repo_env: Option<&str>,
        fleet_env: Option<&str>,
        agent_secrets: &[String],
    ) -> Scrub {
        let long = |v: &str| v.chars().count() >= KNOWN_MIN_CHARS;
        let mut known = Vec::new();
        for text in [repo_env, fleet_env].into_iter().flatten() {
            for (k, v) in dotenv_pairs(text) {
                if long(&v) && talaria_agent_secrets::may_hold_credential(&k, &v) {
                    known.push(v);
                }
            }
        }
        for text in agent_secrets {
            known.extend(dotenv_values(text).into_iter().filter(|v| long(v)));
        }
        known.sort();
        known.dedup();
        Scrub { known }
    }

    /// [`Scrub::from_sources`] for one job: its repo's project env, and the
    /// fleet dir the sandbox was rendered from.
    pub async fn for_job(deps: &WorkbenchDeps, repo: &str) -> Scrub {
        let repo_env = talaria_repo_env::env_file(&deps.pg, &deps.sb, repo)
            .await
            .ok()
            .flatten();
        let dir = talaria_fleet_layout::fleet_dir();
        let fleet_env = std::fs::read_to_string(dir.join(".env")).ok();
        let mut agents = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir.join("agents")) {
            for a in entries.flatten() {
                if let Ok(text) = std::fs::read_to_string(a.path().join("secrets.env")) {
                    agents.push(text);
                }
            }
        }
        Scrub::from_sources(repo_env.as_deref(), fleet_env.as_deref(), &agents)
    }
}

/// `KEY=value` lines, quotes (and the `'\''` escape `env_file` writes) taken
/// off the value. Comments and blank lines skipped.
fn dotenv_pairs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            let v = v.trim();
            let v = if v.len() >= 2
                && ((v.starts_with('\'') && v.ends_with('\''))
                    || (v.starts_with('"') && v.ends_with('"')))
            {
                v[1..v.len() - 1].replace("'\\''", "'")
            } else {
                v.to_string()
            };
            Some((k.trim().to_string(), v))
        })
        .collect()
}

fn dotenv_values(text: &str) -> Vec<String> {
    dotenv_pairs(text).into_iter().map(|(_, v)| v).collect()
}

// ── The record ──────────────────────────────────────────────────────────────

/// The provision state as it is stored and put on the wire. Pure, so the
/// shape is pinned by tests.
#[derive(Debug, Clone, Default)]
pub struct Record {
    /// Which run owns the record: a run writes only while it still does, so
    /// a run taken over (its heartbeat stalled) cannot overwrite its
    /// successor's record.
    pub run_id: String,
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
        let mut raw = [0u8; 8];
        let _ = getrandom::fill(&mut raw);
        Record {
            run_id: raw.iter().map(|b| format!("{b:02x}")).collect(),
            state: "running".into(),
            step: Some(STEPS[0].into()),
            started_ms: now,
            ..Default::default()
        }
    }

    pub fn begin(&mut self, step: &str) {
        self.step = Some(step.into());
    }

    pub fn ok(&mut self, step: &str, ms: u64, attempts: u32, note: &str) {
        let mut s = json!({
            "step": step, "ok": true, "ms": ms, "note": truncate_utf16(note, 400),
        });
        if attempts > 1 {
            s["attempts"] = json!(attempts);
        }
        self.steps.push(s);
    }

    pub fn fail(&mut self, f: &StepFailure, now: i64) {
        let mut s = json!({
            "step": f.step, "ok": false, "ms": f.ms, "note": truncate_utf16(&f.error, 400),
        });
        if f.attempts > 1 {
            s["attempts"] = json!(f.attempts);
        }
        self.steps.push(s);
        self.state = "failed".into();
        self.step = Some(f.step.clone());
        self.error = Some(truncate_utf16(&f.error, 1_000).to_string());
        let output = f.output.trim();
        self.output = (!output.is_empty()).then(|| tail_chars(output, OUTPUT_KEEP));
        self.finished_ms = Some(now);
    }

    pub fn ready(&mut self, now: i64) {
        self.state = "ready".into();
        self.step = None;
        self.finished_ms = Some(now);
    }

    /// Every text a step put on the record, through `scrub`.
    pub fn scrub(&mut self, scrub: &Scrub) {
        for s in &mut self.steps {
            if let Some(n) = s.get("note").and_then(Value::as_str) {
                s["note"] = json!(scrub.text(n));
            }
        }
        self.error = self.error.as_deref().map(|e| scrub.text(e));
        self.output = self.output.as_deref().map(|o| scrub.text(o));
        for w in &mut self.warnings {
            *w = scrub.text(w);
        }
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
            "runId": self.run_id,
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
    out.remove("runId");
    // The full toolchain answer belongs to prepare_env's reply, not to every
    // poll; the steps already carry its one-line note.
    out.remove("toolchain");
    let next = next_step(state);
    out.insert("next".into(), json!(next));
    Value::Object(out)
}

/// The one thing to do about a provision in this state. Pure. It never tells
/// the agent to move the ticket: a person does that (report_outcome records
/// the agent's result without moving anything).
pub fn next_step(state: &Value) -> String {
    match state.get("state").and_then(Value::as_str) {
        Some("ready") => "Ready: run the harness lines exactly as given. Do not clone, install, upgrade or wrap anything — Talaria already did, and checked it.".into(),
        Some("running") => format!(
            "Talaria is provisioning this job (now: {}). Call job_status with this jobId until provision.state is ready — it waits up to a minute for the answer, and a first-time toolchain can take several. Do not clone, install or start the harness yourself meanwhile.",
            state.get("step").and_then(Value::as_str).unwrap_or("starting")
        ),
        Some("failed") => {
            let step = state.get("step").and_then(Value::as_str).unwrap_or("?");
            // "Retried" only when it was: sandbox and preflight never are, and
            // neither is a step whose attempt took longer than RETRY_UNDER_MS.
            let retried = state
                .get("steps")
                .and_then(Value::as_array)
                .and_then(|steps| {
                    steps.iter().rev().find(|s| {
                        s.get("step").and_then(Value::as_str) == Some(step)
                            && s.get("ok") == Some(&json!(false))
                    })
                })
                .and_then(|s| s.get("attempts"))
                .and_then(Value::as_u64)
                .unwrap_or(1)
                > 1;
            format!(
                "Provisioning FAILED at `{step}`{}: the platform could not make this job's workbench ready, and it is not yours to repair — do not install, upgrade or work around the harness or toolchain. {}If the output looks transient (a timeout, a network or git-host error, the install lock), provision again ONCE with prepare_env (or job_status with retry: true). If it fails again: report_problem with the step and output, comment on the ticket with what failed, and call report_outcome with errorMessage saying the workbench could not be provisioned.",
                if retried { " (already retried once)" } else { "" },
                if step == "toolchain" {
                    "If the output shows the REPO declares something wrong (a tool or package name or version in its mise.toml or .talaria/workbench.toml), fix that declaration in the checkout and call prepare_env to provision again. "
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

/// Should job_status (re)start this job's provision? Never provisioned; a
/// run that died (no heartbeat for [`STALE_AFTER_MS`]); or a failed one the
/// agent asked to retry. Pure.
///
/// A READY job is never re-provisioned because the harness channel adopted a
/// newer Oh My Pi or bun since: a running job keeps the release it started
/// on until it ends. Its launcher names that install's version link, the
/// runtime script's prune keeps any install a job's launcher names, and the
/// job's teardown removes the launcher. (A ready record used to be rebuilt
/// whenever its omp was not the compiled pin.)
pub fn needs_provision(state: Option<&Value>, now: i64, retry: bool) -> bool {
    let Some(s) = state else { return true };
    match s.get("state").and_then(Value::as_str) {
        Some("running") => {
            s.get("heartbeatMs").and_then(Value::as_i64).unwrap_or(0) < now - STALE_AFTER_MS
        }
        Some("ready") => false,
        Some("failed") => retry,
        _ => true,
    }
}

/// Has the job's model moved since its ready provision checked one — a
/// ticket re-pinned to another plan, an agent's pick or the org's role
/// changed? Then the lines it would hand out name a model no preflight
/// checked, and job_status provisions it again. A record from before the
/// resolved model was stored (no `modelArg`) is not second-guessed. Pure.
pub fn model_moved(state: Option<&Value>, now: &JobModel) -> bool {
    let Some(h) = state
        .filter(|s| s.get("state").and_then(Value::as_str) == Some("ready"))
        .and_then(|s| s.get("harness"))
    else {
        return false;
    };
    let Some(arg) = h.get("modelArg") else {
        return false;
    };
    arg.as_str() != now.arg.as_deref()
        || h.get("envPrefix").and_then(Value::as_str).unwrap_or("") != now.env_prefix
}

// ── Claiming ────────────────────────────────────────────────────────────────

/// Take the job's provision for a new run, unless one is alive right now (its
/// heartbeat within [`STALE_AFTER_MS`]) or the job is not started. One UPDATE,
/// so two api processes (or two calls) can never both run it.
pub async fn claim(pg: &PgPool, job_id: &str, rec: &Record) -> Result<bool, String> {
    let now = now_ms();
    let got: Option<String> = sqlx::query_scalar(
        "update workbench_jobs set provision = $2::jsonb \
         where id = $1::uuid and status = 'started' and workspace_cleared_at is null \
           and (provision is null or provision->>'state' is distinct from 'running' \
                or coalesce((provision->>'heartbeatMs')::bigint, 0) < $3::bigint) \
         returning id::text",
    )
    .bind(job_id)
    .bind(rec.to_json(now))
    .bind(now - STALE_AFTER_MS)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("provision claim: {e}"))?;
    Ok(got.is_some())
}

/// Write the record — only while this run still owns it.
async fn save(pg: &PgPool, job_id: &str, rec: &Record) -> bool {
    match sqlx::query(
        "update workbench_jobs set provision = $2::jsonb \
         where id = $1::uuid and provision->>'runId' = $3",
    )
    .bind(job_id)
    .bind(rec.to_json(now_ms()))
    .bind(&rec.run_id)
    .execute(pg)
    .await
    {
        Ok(r) => r.rows_affected() > 0,
        Err(e) => {
            tracing::warn!("[workbench] provision state of job {job_id} not saved: {e}");
            true
        }
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

/// Start the job's provision in the background unless one is alive. Returns
/// whether this call started one.
pub async fn kick(deps: &WorkbenchDeps, job: &ProvisionJob) -> Result<bool, String> {
    let rec = Record::start(now_ms());
    if !claim(&deps.pg, &job.id, &rec).await? {
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

/// Run every step, saving the record as it goes. Never panics out: a step
/// that cannot run is a failed step, and a job that ended under it stops it.
pub async fn run(deps: &WorkbenchDeps, job: &ProvisionJob, mut rec: Record) -> Record {
    let pg = &deps.pg;
    // Heartbeat while it runs, so a dead run is reclaimable and a live one
    // is not — and only this run's record.
    let beat = {
        let pg = pg.clone();
        let id = job.id.clone();
        let run_id = rec.run_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(HEARTBEAT_MS)).await;
                let _ = sqlx::query(
                    "update workbench_jobs set provision = jsonb_set(provision, '{heartbeatMs}', to_jsonb($2::bigint)) \
                     where id = $1::uuid and provision->>'state' = 'running' and provision->>'runId' = $3",
                )
                .bind(&id)
                .bind(now_ms())
                .bind(&run_id)
                .execute(&pg)
                .await;
            }
        })
    };
    let started = std::time::Instant::now();
    let scrub = Scrub::for_job(deps, &job.repo).await;
    let outcome = steps(deps, job, &mut rec, &scrub).await;
    beat.abort();
    let ended = matches!(&outcome, Err(f) if f.ended);
    match outcome {
        Ok(()) => rec.ready(now_ms()),
        Err(mut f) => {
            // A stale container is the likeliest cause of whatever broke:
            // say so in the error itself, not only in a warning beside it.
            if let Some(stale) = rec.warnings.iter().find(|w| w.starts_with("sandbox: "))
                && f.step != "toolchain"
                && f.step != "clone"
                && !f.ended
            {
                f.error = format!(
                    "{} — and {}",
                    f.error,
                    stale.trim_start_matches("sandbox: ")
                );
            }
            rec.fail(&f, now_ms())
        }
    }
    rec.scrub(&scrub);
    if !save(pg, &job.id, &rec).await {
        // Taken over by a newer run: it owns the record and the ticket line.
        return rec;
    }
    if ended {
        return rec;
    }
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

/// Why a step stopped the run.
#[derive(Debug, Clone)]
pub struct StepFailure {
    pub step: String,
    pub ms: u64,
    pub attempts: u32,
    pub error: String,
    pub output: String,
    /// The job ended (finished, abandoned, swept) while it was provisioning:
    /// not a failure of the workbench, and nothing more is written for it.
    pub ended: bool,
}

fn failure(step: &str, t: std::time::Instant, error: String, output: String) -> StepFailure {
    StepFailure {
        step: step.into(),
        ms: elapsed_ms(t),
        attempts: 1,
        error,
        output,
        ended: false,
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

/// Is the job still one to provision — started, its workspace not cleared?
/// Asked before every step and before each write into the sandbox, so a job
/// abandoned mid-provision gets nothing re-created after its teardown.
async fn still_live(pg: &PgPool, job_id: &str) -> Result<(), String> {
    let row: Option<(String, bool)> = sqlx::query_as(
        "select status, workspace_cleared_at is not null from workbench_jobs where id = $1::uuid",
    )
    .bind(job_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("job read: {e}"))?;
    match row {
        Some((status, false)) if status == "started" => Ok(()),
        Some((status, cleared)) => Err(format!(
            "the job ended ({status}{}) while it was provisioning; nothing more was set up",
            if cleared { ", workspace cleared" } else { "" }
        )),
        None => Err("the job is gone".into()),
    }
}

fn ended(step: &str, why: String) -> StepFailure {
    StepFailure {
        step: step.into(),
        ms: 0,
        attempts: 1,
        error: why,
        output: String::new(),
        ended: true,
    }
}

/// May this run keep going? The job is still started with its workspace,
/// and the record is still THIS run's — a run taken over (its heartbeat
/// stalled past STALE_AFTER_MS while it was alive) must stop rather than
/// retry or clone over its successor.
async fn may_continue(pg: &PgPool, job_id: &str, run_id: &str) -> Result<(), String> {
    still_live(pg, job_id).await?;
    let owner: Option<Option<String>> =
        sqlx::query_scalar("select provision->>'runId' from workbench_jobs where id = $1::uuid")
            .bind(job_id)
            .fetch_optional(pg)
            .await
            .map_err(|e| format!("job read: {e}"))?;
    match owner.flatten() {
        Some(o) if o == run_id => Ok(()),
        _ => Err("a newer provision of this job took over".into()),
    }
}

/// Begin a step: the job must still be live and the record this run's; the
/// record, scrubbed (a mid-run save is on the wire too), says where it is.
async fn begin(
    pg: &PgPool,
    job: &ProvisionJob,
    rec: &mut Record,
    scrub: &Scrub,
    step: &str,
) -> Result<(), StepFailure> {
    may_continue(pg, &job.id, &rec.run_id)
        .await
        .map_err(|why| ended(step, why))?;
    rec.begin(step);
    rec.scrub(scrub);
    if !save(pg, &job.id, rec).await {
        return Err(ended(
            step,
            "a newer provision of this job took over".into(),
        ));
    }
    Ok(())
}

/// Run `attempt` once, and once more after `pause_ms` when it failed quickly
/// (see [`RETRY_UNDER_MS`]) — and only if `gate` still lets the run go on
/// (the job live, the record this run's). The second failure carries
/// `attempts: 2`. Pure policy over its closures, so the tests drive it.
pub async fn retry_once<T, F, Fut, G, GFut>(
    step: &str,
    pause_ms: u64,
    mut attempt: F,
    mut gate: G,
) -> Result<(T, u32), StepFailure>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, StepFailure>>,
    G: FnMut() -> GFut,
    GFut: std::future::Future<Output = Result<(), String>>,
{
    match attempt().await {
        Ok(v) => Ok((v, 1)),
        Err(f) if f.ended || f.ms >= RETRY_UNDER_MS => Err(f),
        Err(f) => {
            tracing::info!("[workbench] provision retries {step} after: {}", f.error);
            tokio::time::sleep(std::time::Duration::from_millis(pause_ms)).await;
            gate().await.map_err(|why| ended(step, why))?;
            match attempt().await {
                Ok(v) => Ok((v, 2)),
                Err(mut f2) => {
                    f2.attempts = 2;
                    Err(f2)
                }
            }
        }
    }
}

/// [`retry_once`] for a step of this run.
async fn with_retry<T, F, Fut>(
    pg: &PgPool,
    job: &ProvisionJob,
    run_id: &str,
    step: &str,
    attempt: F,
) -> Result<(T, u32), StepFailure>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, StepFailure>>,
{
    retry_once(step, RETRY_PAUSE_MS, attempt, || {
        may_continue(pg, &job.id, run_id)
    })
    .await
}

async fn steps(
    deps: &WorkbenchDeps,
    job: &ProvisionJob,
    rec: &mut Record,
    scrub: &Scrub,
) -> Result<(), StepFailure> {
    let pg = &deps.pg;
    let run_id = rec.run_id.clone();
    let container = managed_container(pg, &job.department).await;
    let workdir = job.workdir();
    // References, so each attempt's `async move` copies them, not the strings.
    let (container, workdir) = (&container, &workdir);
    let slot_err = |step: &str| {
        failure(
            step,
            std::time::Instant::now(),
            format!("not a job id: {:?}", job.id),
            String::new(),
        )
    };
    let harness_dir = wh::job_harness_dir(&job.id).ok_or_else(|| slot_err("harness"))?;
    let agent_dir = wh::job_agent_dir(&job.id).ok_or_else(|| slot_err("harness"))?;
    let launcher = wh::job_launcher(&job.id).ok_or_else(|| slot_err("harness"))?;

    // sandbox
    begin(pg, job, rec, scrub, "sandbox").await?;
    let t = std::time::Instant::now();
    let shape = docker_exec_as(
        container,
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
        1,
        if stale.is_some() {
            "sandbox answers, but was created by an older render"
        } else {
            "sandbox answers and is current"
        },
    );

    // clone
    begin(pg, job, rec, scrub, "clone").await?;
    let t = std::time::Instant::now();
    let (out, attempts) = with_retry(pg, job, &run_id, "clone", || async move {
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
        docker_exec_as(
            container,
            Some(devenv::AGENT_USER),
            &["sh", "-c", CLONE_SH, "sh", workdir, &url, &job.branch],
            None,
            CLONE_TIMEOUT_MS,
        )
        .await
        .map(|(o, _)| o)
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
        })
    })
    .await?;
    rec.ok("clone", elapsed_ms(t), attempts, out.trim());

    // toolchain
    begin(pg, job, rec, scrub, "toolchain").await?;
    let t = std::time::Instant::now();
    let (mut toolchain, attempts) = with_retry(pg, job, &run_id, "toolchain", || async move {
        let t = std::time::Instant::now();
        devenv::prepare_env(pg, &job.department, workdir)
            .await
            .map_err(|e| {
                let (why, out) = reason_and_output(&e);
                failure(
                    "toolchain",
                    t,
                    format!("the repo's toolchain did not install: {why}"),
                    out,
                )
            })
    })
    .await?;
    // The project env carries the repo's secrets: never written for a job
    // that ended while its toolchain installed.
    may_continue(pg, &job.id, &rec.run_id)
        .await
        .map_err(|why| ended("toolchain", why))?;
    let env_vars = devenv::deliver_job_env(pg, &deps.sb, &job.department, workdir, &job.repo)
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
            devenv::job_env_answer(workdir, env_vars),
        );
    }
    rec.ok(
        "toolchain",
        elapsed_ms(t),
        attempts,
        &toolchain_note(&toolchain),
    );
    let talaria_app = toolchain["talariaApp"] == json!(true);
    // The toolchain answer goes to the agent (prepare_env): scrubbed too.
    rec.toolchain = Some(scrub.value(&toolchain));

    // harness
    begin(pg, job, rec, scrub, "harness").await?;
    let t = std::time::Instant::now();
    let (runtime, attempts) = with_retry(pg, job, &run_id, "harness", || async move {
        let t = std::time::Instant::now();
        devenv::ensure_harness_runtime_noted(pg, &job.department)
            .await
            .map_err(|e| {
                let (why, out) = reason_and_output(&e);
                failure("harness", t, why, out)
            })
    })
    .await?;
    let jm = job_model(pg, &job.agent_id, job.task_id.as_deref())
        .await
        .map_err(|e| failure("harness", t, e, String::new()))?;
    let (runtime, fell_back) = runtime;
    if let Some(note) = fell_back {
        rec.warnings.push(note);
    }
    let text = wh::launcher_script(&job.id, workdir, &runtime, jm.gateway)
        .ok_or_else(|| slot_err("harness"))?;
    may_continue(pg, &job.id, &rec.run_id)
        .await
        .map_err(|why| ended("harness", why))?;
    docker_exec_as(
        container,
        Some(devenv::AGENT_USER),
        &[
            "sh",
            "-c",
            &wh::job_harness_script(),
            "sh",
            &harness_dir,
            &job.session_dir(),
            &format!("{workdir}/.git"),
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
        let kit = devenv::install_app_kit(pg, &job.department, workdir, session.as_deref())
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
        "agentDir": agent_dir,
        "sessionDir": job.session_dir(),
        "gateway": jm.gateway,
        "model": jm.model,
        // What the lines will say: handed out FROM here, so they name the
        // model this preflight checked, not one resolved again later.
        "modelArg": jm.arg,
        "envPrefix": jm.env_prefix,
    }));
    rec.ok(
        "harness",
        elapsed_ms(t),
        attempts,
        &format!("{} on bun {}", runtime.omp, runtime.bun),
    );

    // preflight — of the model the job's lines will name (job_model, the
    // same resolution harness_answer uses), not some other one.
    begin(pg, job, rec, scrub, "preflight").await?;
    let t = std::time::Instant::now();
    let model_check = if jm.gateway {
        match jm.model.as_deref() {
            Some(m) => m.to_string(),
            None => {
                return Err(failure(
                    "preflight",
                    t,
                    "no model resolves for this job's harness — no Workbench model role is set (an admin sets the code roles on /models), and neither the ticket nor the agent picks one".into(),
                    String::new(),
                ));
            }
        }
    } else {
        String::new()
    };
    let verdict = preflight(
        container,
        &launcher,
        &agent_dir,
        &model_check,
        &runtime.bun_path,
        runtime.omp_version(),
    )
    .await;
    match verdict {
        Ok(v) => {
            rec.ok(
                "preflight",
                elapsed_ms(t),
                1,
                &format!(
                    "{v} starts; agent dir writable{}",
                    if jm.gateway {
                        format!("; {model_check} listed; gateway accepts the key")
                    } else {
                        String::new()
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
    omp_version: &str,
) -> Result<String, (String, String)> {
    let script = wh::preflight_script(bun_path, omp_version);
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

    fn quick_fail(step: &str) -> StepFailure {
        failure(
            step,
            std::time::Instant::now(),
            format!("{step} broke"),
            String::new(),
        )
    }

    #[tokio::test]
    async fn a_quick_failure_is_retried_once_and_a_slow_or_ended_one_is_not() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let ok_gate = || async { Ok::<(), String>(()) };
        // Succeeds first time: one attempt.
        let (v, n) = retry_once("clone", 0, || async { Ok::<_, StepFailure>(7) }, ok_gate)
            .await
            .unwrap();
        assert_eq!((v, n), (7, 1));
        // Fails quickly, then works: two attempts, and the gate was asked.
        let calls = AtomicU32::new(0);
        let gates = AtomicU32::new(0);
        let (_, n) = retry_once(
            "clone",
            0,
            || async {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(quick_fail("clone"))
                } else {
                    Ok(())
                }
            },
            || async {
                gates.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(gates.load(Ordering::SeqCst), 1);
        // Fails twice: the failure says it was retried.
        let f = retry_once(
            "toolchain",
            0,
            || async { Err::<(), _>(quick_fail("toolchain")) },
            ok_gate,
        )
        .await
        .unwrap_err();
        assert_eq!(f.attempts, 2);
        // A failure that took RETRY_UNDER_MS is not doubled.
        let slow = AtomicU32::new(0);
        let f = retry_once(
            "clone",
            0,
            || async {
                slow.fetch_add(1, Ordering::SeqCst);
                let mut f = quick_fail("clone");
                f.ms = RETRY_UNDER_MS;
                Err::<(), _>(f)
            },
            ok_gate,
        )
        .await
        .unwrap_err();
        assert_eq!((f.attempts, slow.load(Ordering::SeqCst)), (1, 1));
        // Taken over (or the job ended) between attempts: no second attempt.
        let tries = AtomicU32::new(0);
        let f = retry_once(
            "clone",
            0,
            || async {
                tries.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(quick_fail("clone"))
            },
            || async { Err("a newer provision of this job took over".to_string()) },
        )
        .await
        .unwrap_err();
        assert!(f.ended && f.error.contains("took over"), "{f:?}");
        assert_eq!(
            tries.load(Ordering::SeqCst),
            1,
            "a run taken over does not retry"
        );
    }

    #[test]
    fn scrub_values_come_from_secrets_not_from_config() {
        let repo_env = "CI='true'\nNODE_ENV='production'\nBRANCH='main'\nNPM_TOKEN='npm_abcdefghijklmnop'\nSHORT_KEY='abc'\nDATABASE_URL='postgres://app:pw-secret-9@db/app'\n";
        let fleet = "LLM_WORKBENCH_API_KEY=tlk_fleetkeyvalue123\nAGENT_DNS_1=1.1.1.1\nTZ=Europe/Amsterdam\n";
        let agent = vec!["ANYNAME=agent-secret-value\nTINY=xy\n".to_string()];
        let scrub = Scrub::from_sources(Some(repo_env), Some(fleet), &agent);
        for secret in [
            "npm_abcdefghijklmnop",
            "postgres://app:pw-secret-9@db/app",
            "tlk_fleetkeyvalue123",
            "agent-secret-value",
        ] {
            assert!(
                scrub.known.iter().any(|k| k == secret),
                "{secret} is taken out"
            );
        }
        for config in [
            "true",
            "production",
            "main",
            "abc",
            "1.1.1.1",
            "Europe/Amsterdam",
            "xy",
        ] {
            assert!(
                !scrub.known.iter().any(|k| k == config),
                "{config} is not a secret"
            );
        }
        let text =
            scrub.text("deploying main to production with CI=true; token npm_abcdefghijklmnop");
        assert!(
            text.contains("main") && text.contains("production") && text.contains("true"),
            "{text}"
        );
        assert!(!text.contains("npm_abcdefghijklmnop"), "{text}");
    }

    #[test]
    fn the_toolchain_answer_is_scrubbed_value_by_value_and_keeps_its_shape() {
        // Even a scrub that knew `true` (the old text scrub took any value
        // of 3+ characters) cannot touch a JSON boolean: only strings are
        // scrubbed.
        let scrub = Scrub::with(vec!["true".into(), "s3cr3t-value-xyz".into()]);
        let tc = json!({
            "talariaApp": true,
            "sccache": true,
            "tools": ["bun@1.3.14"],
            "resolved": "bun 1.3.14 ~/mise.toml (fetched with s3cr3t-value-xyz)",
            "projectEnv": { "vars": 2, "path": "/w/.talaria/env" },
        });
        let out = scrub.value(&tc);
        assert_eq!(out["talariaApp"], json!(true));
        assert_eq!(out["sccache"], json!(true));
        assert_eq!(out["projectEnv"]["vars"], json!(2));
        assert_eq!(out["tools"], json!(["bun@1.3.14"]));
        assert!(
            !out["resolved"]
                .as_str()
                .unwrap()
                .contains("s3cr3t-value-xyz")
        );
        // And the answer stays an object prepare_env can return.
        assert!(out.is_object());
    }

    #[test]
    fn only_a_step_that_was_retried_says_so() {
        let mut r = Record::start(1);
        r.fail(&quick_fail("preflight"), 2);
        let once = next_step(&r.to_json(2));
        assert!(!once.contains("already retried"), "{once}");
        let mut r = Record::start(1);
        let mut f = quick_fail("clone");
        f.attempts = 2;
        r.fail(&f, 2);
        let twice = next_step(&r.to_json(2));
        assert!(
            twice.contains("FAILED at `clone` (already retried once)"),
            "{twice}"
        );
    }

    #[test]
    fn a_ready_record_whose_model_moved_is_provisioned_again() {
        let jm = |arg: Option<&str>, prefix: &str| JobModel {
            gateway: true,
            model: arg.map(|a| a.trim_start_matches("talaria/").to_string()),
            arg: arg.map(str::to_string),
            env_prefix: prefix.to_string(),
            roles: wh::OmpRoles::default(),
            plan: None,
        };
        let ready =
            json!({ "state": "ready", "harness": { "modelArg": "talaria/m1", "envPrefix": "" } });
        assert!(!model_moved(Some(&ready), &jm(Some("talaria/m1"), "")));
        assert!(
            model_moved(Some(&ready), &jm(Some("talaria/m2"), "")),
            "a re-pin"
        );
        assert!(
            model_moved(Some(&ready), &jm(Some("talaria/m1"), "PI_SMOL_MODEL=x ")),
            "a role moved"
        );
        assert!(
            model_moved(Some(&ready), &jm(None, "")),
            "no model any more"
        );
        let older = json!({ "state": "ready", "harness": { "model": "m1" } });
        assert!(
            !model_moved(Some(&older), &jm(Some("talaria/m2"), "")),
            "not second-guessed"
        );
        let failed = json!({ "state": "failed", "harness": { "modelArg": "talaria/m1" } });
        assert!(!model_moved(Some(&failed), &jm(Some("talaria/m2"), "")));
    }

    #[test]
    fn a_record_runs_then_lands_ready_with_every_step() {
        let mut r = Record::start(1_000);
        assert_eq!(r.to_json(1_000)["state"], "running");
        assert_eq!(r.to_json(1_000)["step"], "sandbox");
        for s in STEPS {
            r.begin(s);
            r.ok(s, 5, 1, "fine");
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
        let mut f = failure(
            "toolchain",
            std::time::Instant::now(),
            "the repo's toolchain did not install: mise ERROR".into(),
            long.clone(),
        );
        f.attempts = 2;
        r.fail(&f, 3_000);
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
    fn next_says_what_to_do_in_every_state_and_never_to_repair_or_move_the_ticket() {
        let ready = next_step(&json!({ "state": "ready" }));
        assert!(ready.contains("exactly as given"), "{ready}");
        let running = next_step(&json!({ "state": "running", "step": "toolchain" }));
        assert!(
            running.contains("job_status") && running.contains("toolchain"),
            "{running}"
        );
        let failed = next_step(&json!({ "state": "failed", "step": "preflight" }));
        assert!(
            failed.contains("FAILED at `preflight`") && failed.contains("report_problem"),
            "{failed}"
        );
        assert!(failed.contains("not yours to repair"), "{failed}");
        // One retry the agent may ask for, then report — never a ticket move.
        assert!(
            failed.contains("ONCE with prepare_env") && failed.contains("retry: true"),
            "{failed}"
        );
        assert!(
            failed.contains("report_outcome with errorMessage"),
            "{failed}"
        );
        for state in ["ready", "running", "failed"] {
            for step in ["clone", "toolchain", "preflight"] {
                let t = next_step(&json!({ "state": state, "step": step })).to_lowercase();
                assert!(!t.contains("blocked") && !t.contains("triage"), "{t}");
            }
        }
        let repo = next_step(&json!({ "state": "failed", "step": "toolchain" }));
        assert!(
            repo.contains(".talaria/workbench.toml") && repo.contains("prepare_env"),
            "{repo}"
        );
        assert!(next_step(&Value::Null).contains("job_status"));
    }

    #[test]
    fn job_status_reprovisions_only_what_needs_it() {
        let now = 10_000_000;
        let omp = format!("omp/{}", wh::OMP_VERSION);
        assert!(needs_provision(None, now, false), "never provisioned");
        let alive = json!({ "state": "running", "heartbeatMs": now - 5_000 });
        assert!(
            !needs_provision(Some(&alive), now, true),
            "a live run is never doubled"
        );
        let dead = json!({ "state": "running", "heartbeatMs": now - STALE_AFTER_MS - 1 });
        assert!(
            needs_provision(Some(&dead), now, false),
            "a dead run is taken over"
        );
        let ready = json!({ "state": "ready", "harness": { "omp": omp } });
        assert!(!needs_provision(Some(&ready), now, true));
        let old = json!({ "state": "ready", "harness": { "omp": "omp/18.2.4" } });
        assert!(
            !needs_provision(Some(&old), now, false),
            "a running job keeps the release it started on: a newer one adopted since never moves it"
        );
        let failed = json!({ "state": "failed", "step": "clone" });
        assert!(
            !needs_provision(Some(&failed), now, false),
            "a failure stays until asked"
        );
        assert!(
            needs_provision(Some(&failed), now, true),
            "and is provisioned again when asked"
        );
    }

    #[test]
    fn what_a_step_printed_is_scrubbed_before_anyone_reads_it() {
        let scrub = Scrub::with(vec!["s3cr3t-project-value".into()]);
        let mut r = Record::start(1);
        r.ok("toolchain", 1, 1, "fetched with s3cr3t-project-value");
        let f = failure(
            "toolchain",
            std::time::Instant::now(),
            "401 from https://bot:hunter2token@registry.example/pkg".into(),
            "npm ERR! 401 Authorization: Bearer npm_abcdefghijklmnopqrstuvwxyz0123\nusing s3cr3t-project-value".into(),
        );
        r.warnings.push("appKit: token=tlk_abcdefghijklmnop".into());
        r.fail(&f, 2);
        r.scrub(&scrub);
        let j = r.to_json(2).to_string();
        for leaked in [
            "s3cr3t-project-value",
            "hunter2token",
            "npm_abcdefghijklmnopqrstuvwxyz0123",
            "tlk_abcdefghijklmnop",
        ] {
            assert!(!j.contains(leaked), "{leaked} leaked: {j}");
        }
        assert!(j.contains("401"), "the reason survives: {j}");
    }

    #[test]
    fn dotenv_values_read_what_env_file_writes() {
        let body = "A='plain'\nB='it'\\''s'\n# note\nC=\"dq\"\n\nD=bare\n";
        assert_eq!(dotenv_values(body), vec!["plain", "it's", "dq", "bare"]);
    }

    #[test]
    fn the_wire_hides_the_run_id_and_heartbeat() {
        let w = wire(&Record::start(1).to_json(5));
        assert!(w.get("runId").is_none() && w.get("heartbeatMs").is_none());
        for key in [
            "state",
            "step",
            "steps",
            "error",
            "output",
            "warnings",
            "harness",
            "startedAt",
            "finishedAt",
            "next",
        ] {
            assert!(w.get(key).is_some(), "{key} is part of the wire");
        }
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
        // A clone cut off before it had a HEAD (the old in-place clone) is
        // not a checkout: moved aside, cloned again. A temp dir an earlier
        // attempt left is removed and never becomes the workdir.
        let cut = tmp.join("jobs/cut");
        std::fs::create_dir_all(&cut).unwrap();
        run(&["init", "-q"], &cut);
        let stray = tmp.join("jobs/.cut.clone.999");
        std::fs::create_dir_all(stray.join(".git")).unwrap();
        let healed = sh(&cut);
        assert!(
            healed.status.success(),
            "{}",
            String::from_utf8_lossy(&healed.stderr)
        );
        assert!(String::from_utf8_lossy(&healed.stderr).contains("no HEAD aside"));
        assert!(String::from_utf8_lossy(&healed.stdout).starts_with("cloned agent/x at "));
        assert!(!stray.exists(), "an earlier attempt's temp clone is gone");
        // A branch that does not exist fails, and leaves no workdir behind.
        let none = tmp.join("jobs/none");
        let out = Command::new("sh")
            .args(["-c", CLONE_SH, "sh"])
            .arg(&none)
            .arg(origin.to_str().unwrap())
            .arg("no-such-branch")
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(
            !none.join(".git").exists(),
            "a failed clone is never a checkout"
        );
        let leftovers: Vec<_> = std::fs::read_dir(tmp.join("jobs"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".clone."))
            .collect();
        assert!(leftovers.is_empty(), "no temp clone left behind");
        // Something appears in the workdir while the clone runs (a git that
        // writes there after cloning stands in for it): the clone is not
        // nested inside it, and the script fails rather than reporting success.
        let shim = tmp.join("shim");
        std::fs::create_dir_all(&shim).unwrap();
        let real_git = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let intruded = tmp.join("jobs/intruded");
        {
            use std::os::unix::fs::PermissionsExt;
            let g = shim.join("git");
            std::fs::write(
                &g,
                format!(
                    "#!/bin/sh\n{} \"$@\"; rc=$?\nif [ \"$1\" = clone ]; then mkdir -p {d} && touch {d}/stray; fi\nexit $rc\n",
                    real_git.trim(),
                    d = intruded.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&g, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!(
            "{}:{}",
            shim.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = Command::new("sh")
            .args(["-c", CLONE_SH, "sh"])
            .arg(&intruded)
            .arg(origin.to_str().unwrap())
            .arg("agent/x")
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("gained files"));
        assert!(!intruded.join(".git").exists(), "not a checkout");
        let names: Vec<String> = std::fs::read_dir(&intruded)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["stray".to_string()], "nothing nested in it");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
