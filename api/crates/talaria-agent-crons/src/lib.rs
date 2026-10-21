// Native Hermes crons, managed by Talaria. Every agent's gateway runs Hermes'
// own scheduler (ticks /opt/data/cron/jobs.json every 60s), so jobs live and
// fire INSIDE the agent — Talaria is just the control surface: it reads
// jobs.json for truth and drives the `hermes cron` CLI for mutations through
// the running container (docker exec). Requires the container to be up.
//
// METERING CAVEAT: a cron fires the agent's own
// loop on the agent's own config, i.e. the PERSONA gateway key, which the
// gateway leaves unmetered because a Talaria flow normally writes that turn's
// ledger row. A cron has no flow, so its spend reaches no ledger. Not fixable
// from here: `hermes cron create` takes no model/provider/credential, and the
// scheduler runs inside the single `hermes gateway run` process — so at the
// gateway a cron turn is byte-identical to a persona turn.

use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_fleet_docker::{docker_exec, managed_container};
use talaria_gateway::settings::get_setting;

const JOBS_PATH: &str = "/opt/data/cron/jobs.json";

async fn agent_for(pg: &PgPool, def_id: &str) -> Result<(String, String), String> {
    let row: Option<(String, String)> = sqlx::query_as(
        "select department, slug from agent_defs where id = $1::uuid and managed and enabled",
    )
    .bind(def_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    row.ok_or_else(|| "not a running managed agent".to_string())
}

/// The job as Talaria shows it — every field defaulted from the raw
/// jobs.json entry.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// Human schedule as Hermes displays it (cron expr or "every 2h").
    pub schedule: String,
    pub enabled: bool,
    pub state: String,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
}

pub async fn list_cron_jobs(pg: &PgPool, def_id: &str) -> Result<Vec<CronJob>, String> {
    let (department, _) = agent_for(pg, def_id).await?;
    let name = managed_container(pg, &department).await;
    let (stdout, _) = match docker_exec(&name, &["cat", JOBS_PATH], 30_000).await {
        Ok(ok) => ok,
        Err(e) => {
            if e.to_lowercase().contains("no such file") {
                (r#"{"jobs":[]}"#.to_string(), String::new())
            } else {
                return Err(format!("cannot read crons from {name}: {e}"));
            }
        }
    };
    let raw = serde_json::from_str::<serde_json::Value>(&stdout).map_err(|e| e.to_string())?;
    let jobs = raw
        .get("jobs")
        .and_then(|j| j.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(jobs
        .iter()
        .map(|j| {
            let str_field = |key: &str| -> Option<String> {
                j.get(key).and_then(|v| v.as_str()).map(String::from)
            };
            CronJob {
                id: str_field("id").unwrap_or_default(),
                // j.name ?? j.id — ONLY a missing/null name falls to the id;
                // an empty string is a present value and stays.
                name: match j.get("name") {
                    Some(serde_json::Value::String(n)) => n.clone(),
                    _ => str_field("id").unwrap_or_default(),
                },
                prompt: str_field("prompt").unwrap_or_default(),
                schedule: j
                    .get("schedule_display")
                    .and_then(|s| s.as_str())
                    .map(String::from)
                    .or_else(|| {
                        j.get("schedule").and_then(|s| {
                            s.get("display").and_then(|d| d.as_str()).map(String::from)
                        })
                    })
                    .or_else(|| {
                        j.get("schedule")
                            .and_then(|s| s.get("expr").and_then(|e| e.as_str()).map(String::from))
                    })
                    .unwrap_or_default(),
                enabled: j.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true),
                state: str_field("state").unwrap_or_else(|| "scheduled".into()),
                next_run_at: str_field("next_run_at"),
                last_run_at: str_field("last_run_at"),
                last_status: str_field("last_status"),
                last_error: str_field("last_error"),
            }
        })
        .collect())
}

// Hermes is the schedule validator ('30m', 'every 2h', or a 5-field cron) and
// its error comes back verbatim; we only block flag injection and newlines.
fn assert_safe(value: &str, what: &str) -> Result<(), String> {
    if value.starts_with('-') {
        return Err(format!("{what} cannot start with \"-\""));
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(format!("{what} cannot contain newlines"));
    }
    Ok(())
}

// ── Frequency floor ─────────────────────────────────────────────────────────
// A cron job is an agent turn, and an agent turn is LLM spend. `* * * * *`
// used to be accepted verbatim: 1,440 turns a day, per agent, forever — the
// cheapest way in the product to run up an unbounded bill, and the schedule a
// well-meaning "check this often" request lands on. The floor is the coarse
// guard; the gateway budget is the fine one.
const DEFAULT_MIN_INTERVAL_MINUTES: f64 = 5.0;

pub async fn cron_floor_minutes(pg: &PgPool) -> f64 {
    get_setting(
        pg,
        "cron_min_interval_minutes",
        serde_json::json!(DEFAULT_MIN_INTERVAL_MINUTES),
    )
    .await
    .as_f64()
    .unwrap_or(DEFAULT_MIN_INTERVAL_MINUTES)
}

fn unit_minutes(unit: char) -> Option<f64> {
    match unit {
        's' => Some(1.0 / 60.0),
        'm' => Some(1.0),
        'h' => Some(60.0),
        'd' => Some(1440.0),
        _ => None,
    }
}

/// Expand one comma-separated cron field into the values it fires on
/// (cronFieldValues). An unparseable field expands to NOTHING — the caller
/// reads that as "can't tell" and stays permissive. Values stay f64 because
/// the expansion accumulates floats (`v += step` with a fractional
/// step lands on 1.5, 4.5, …) and the gap arithmetic runs on them.
fn cron_field_values(field: &str, max: i64) -> Vec<f64> {
    // A set, as a sorted vec: f64 isn't Ord, so the dedup is by equality
    // over sorted neighbors (values are finite and ≥ 0 here).
    let mut out: Vec<f64> = Vec::new();
    for part in field.split(',') {
        let (spec, step_raw) = match part.split_once('/') {
            Some((s, r)) => (s, r),
            None => (part, ""),
        };
        // stepRaw ? Number(stepRaw) : 1 — unparseable/absent means no `1`
        // bailout: '' reads falsy, and NaN fails isFinite below.
        let step: f64 = if step_raw.is_empty() {
            1.0
        } else {
            step_raw.parse::<f64>().unwrap_or(f64::NAN)
        };
        if !step.is_finite() || step < 1.0 {
            return Vec::new();
        }
        let (lo, hi) = if spec.is_empty() || spec == "*" {
            (0.0, max as f64)
        } else {
            let (a, b) = match spec.split_once('-') {
                Some((a, b)) => (a, Some(b)),
                None => (spec, None),
            };
            let parse = |s: &str| -> Result<f64, ()> {
                // An empty spec never reaches here except as lo of "‑5",
                // which parses; NaN fails below.
                s.parse::<f64>().map_err(|_| ()).or(Ok(f64::NAN))
            };
            let lo = match parse(a) {
                Ok(v) if v.is_finite() => v,
                _ => return Vec::new(),
            };
            let hi = match b.map(parse) {
                Some(Ok(v)) if v.is_finite() => v,
                Some(_) => return Vec::new(),
                None => lo,
            };
            (lo, hi)
        };
        if lo < 0.0 || hi > max as f64 || lo > hi {
            return Vec::new();
        }
        let mut v = lo;
        while v <= hi {
            out.push(v);
            v += step;
        }
    }
    out.sort_by(f64::total_cmp);
    out.dedup();
    out
}

/// The SHORTEST gap between two firings of a schedule, in minutes — the
/// number a floor has to be compared against. None when we can't tell
/// (Hermes stays the validator for anything exotic; we only refuse what we
/// can prove is too fast).
pub fn min_interval_minutes(schedule: &str) -> Option<f64> {
    let s = schedule.trim().to_lowercase();

    // Interval form: "30m", "every 2h", "90s".
    let iv = regex::Regex::new(
        r"^(?:every\s+)?(\d+)\s*(s|sec|secs|second|seconds|m|min|mins|minute|minutes|h|hr|hrs|hour|hours|d|day|days)$",
    )
    .unwrap();
    if let Some(caps) = iv.captures(&s) {
        let n: f64 = caps[1].parse().ok()?;
        let unit = caps[2].chars().next()?;
        return unit_minutes(unit).map(|u| n * u);
    }

    // 5-field cron: minute hour dom mon dow.
    let fields: Vec<&str> = s.split_whitespace().collect();
    if fields.len() != 5 {
        return None;
    }
    let minutes = cron_field_values(fields[0], 59);
    if minutes.is_empty() {
        return None;
    }
    // One minute value an hour → at most hourly, whatever the other fields say.
    if minutes.len() == 1 {
        return Some(60.0);
    }
    // wrap past the hour
    let mut gap = 60.0 - minutes[minutes.len() - 1] + minutes[0];
    for i in 1..minutes.len() {
        gap = gap.min(minutes[i] - minutes[i - 1]);
    }
    Some(gap)
}

async fn assert_schedule_allowed(pg: &PgPool, schedule: &str) -> Result<(), String> {
    let Some(gap) = min_interval_minutes(schedule) else {
        return Ok(());
    };
    let floor = cron_floor_minutes(pg).await;
    if floor > 0.0 && gap < floor {
        // Numbers print whole here: 5 stays "5", 5.5 "5.5" — f64 Display
        // does the same for this range.
        let every = if gap < 1.0 {
            format!("{}s", (gap * 60.0).round())
        } else {
            format!("{gap}m")
        };
        return Err(format!(
            "schedule fires every {every}, faster than the {floor}m minimum an admin set — ask them to lower the floor or pick a slower schedule"
        ));
    }
    Ok(())
}

pub async fn create_cron_job(
    pg: &PgPool,
    def_id: &str,
    name: &str,
    schedule: &str,
    prompt: &str,
) -> Result<Option<String>, String> {
    let (department, _) = agent_for(pg, def_id).await?;
    let name = name.trim();
    let schedule = schedule.trim();
    let prompt = prompt.trim();
    if name.is_empty() || schedule.is_empty() || prompt.is_empty() {
        return Err("name, schedule, and prompt are required".to_string());
    }
    assert_safe(name, "name")?;
    assert_safe(schedule, "schedule")?;
    assert_schedule_allowed(pg, schedule).await?;
    if prompt.starts_with('-') {
        return Err("prompt cannot start with \"-\"".to_string());
    }
    let container = managed_container(pg, &department).await;
    let (stdout, _) = docker_exec(
        &container,
        &[
            "hermes",
            "cron",
            "create",
            "--name",
            name,
            "--deliver",
            "local",
            schedule,
            prompt,
        ],
        30_000,
    )
    .await?;
    let re = regex::Regex::new(r"Created job:\s*([a-f0-9]+)").unwrap();
    Ok(re
        .captures(&stdout)
        .map(|c| c[1].to_string())
        .filter(|id| !id.is_empty()))
}

fn job_id_ok(job_id: &str) -> bool {
    let Ok(re) = regex::Regex::new("^[a-f0-9]{6,32}$") else {
        unreachable!()
    };
    re.is_match(job_id)
}

/// In-place edit via `hermes cron edit` — name/schedule/prompt, any subset.
pub async fn edit_cron_job(
    pg: &PgPool,
    def_id: &str,
    job_id: &str,
    name: Option<&str>,
    schedule: Option<&str>,
    prompt: Option<&str>,
) -> Result<(), String> {
    if !job_id_ok(job_id) {
        return Err("bad job id".to_string());
    }
    let (department, _) = agent_for(pg, def_id).await?;
    let mut args: Vec<String> = vec!["hermes".into(), "cron".into(), "edit".into(), job_id.into()];
    // TRIM, THEN GUARD — the same order as `create_cron_job`. The value that
    // reaches argv is the trimmed one, so the guard has to look at it: a
    // prompt of " --schedule=* * * * *" passed the leading-dash check on
    // its untrimmed self and reached Hermes as an option.
    if let Some(name) = name {
        let name = name.trim();
        assert_safe(name, "name")?;
        args.push("--name".into());
        args.push(name.to_string());
    }
    if let Some(schedule) = schedule {
        let schedule = schedule.trim();
        assert_safe(schedule, "schedule")?;
        assert_schedule_allowed(pg, schedule).await?;
        args.push("--schedule".into());
        args.push(schedule.to_string());
    }
    if let Some(prompt) = prompt {
        let prompt = prompt.trim();
        if prompt.starts_with('-') {
            return Err("prompt cannot start with \"-\"".to_string());
        }
        args.push("--prompt".into());
        args.push(prompt.to_string());
    }
    if args.len() == 4 {
        return Err("nothing to edit".to_string());
    }
    let container = managed_container(pg, &department).await;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    docker_exec(&container, &refs, 30_000).await.map(|_| ())
}

async fn job_action(pg: &PgPool, def_id: &str, job_id: &str, action: &str) -> Result<(), String> {
    if !job_id_ok(job_id) {
        return Err("bad job id".to_string());
    }
    let (department, _) = agent_for(pg, def_id).await?;
    let container = managed_container(pg, &department).await;
    docker_exec(&container, &["hermes", "cron", action, job_id], 30_000)
        .await
        .map(|_| ())
}

pub async fn remove_cron_job(pg: &PgPool, def_id: &str, job_id: &str) -> Result<(), String> {
    job_action(pg, def_id, job_id, "remove").await
}
pub async fn pause_cron_job(pg: &PgPool, def_id: &str, job_id: &str) -> Result<(), String> {
    job_action(pg, def_id, job_id, "pause").await
}
pub async fn resume_cron_job(pg: &PgPool, def_id: &str, job_id: &str) -> Result<(), String> {
    job_action(pg, def_id, job_id, "resume").await
}
/// Queue the job for the next scheduler tick (≤60s).
pub async fn run_cron_job(pg: &PgPool, def_id: &str, job_id: &str) -> Result<(), String> {
    job_action(pg, def_id, job_id, "run").await
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetCronAgent {
    pub id: String,
    pub slug: String,
    pub display_name: String,
    pub jobs: Vec<CronJob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Crons across the whole managed fleet; a down container is reported, not
/// fatal.
pub async fn list_fleet_crons(pg: &PgPool) -> Result<Vec<FleetCronAgent>, sqlx::Error> {
    let defs: Vec<(String, String, String)> = sqlx::query_as(
        "select id::text, slug, display_name from agent_defs \
         where managed and enabled order by slug",
    )
    .fetch_all(pg)
    .await?;
    // join_all across agents — each list_cron_jobs is a round-trip to
    // that agent's container, and sequential turns N containers' answers into
    // a+b+… instead of max(a,b). join_all keeps the defs' slug order; a failed
    // agent still reports per-agent, never fatal.
    let out: Vec<FleetCronAgent> = futures_util::future::join_all(defs.into_iter().map(
        |(id, slug, display_name)| async move {
            match list_cron_jobs(pg, &id).await {
                Ok(jobs) => FleetCronAgent {
                    id,
                    slug,
                    display_name,
                    jobs,
                    error: None,
                },
                Err(e) => FleetCronAgent {
                    id,
                    slug,
                    display_name,
                    jobs: Vec::new(),
                    error: Some(e),
                },
            }
        },
    ))
    .await;
    Ok(out)
}

/// The per-agent result of a fleet-wide create ({agentId, ok, error?}).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetCronResult {
    pub agent_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Create the same job on many agents. When the schedule is a cron expression
/// with a plain numeric minute, each agent is staggered by `stagger_minutes`
/// so the fleet doesn't hit the shared LLM at the same instant (the stack's
/// long-standing convention); interval schedules ("every 2h") pass through.
pub async fn create_fleet_crons(
    pg: &PgPool,
    agent_ids: &[String],
    name: &str,
    schedule: &str,
    prompt: &str,
    stagger_minutes: Option<i64>,
) -> Result<Vec<FleetCronResult>, sqlx::Error> {
    let stagger = stagger_minutes.unwrap_or(2);
    let m = regex::Regex::new(r"^(\d{1,2})((?:\s+\S+){4})$").unwrap();
    let caps = m.captures(schedule.trim());
    let mut results = Vec::new();
    for (i, agent_id) in agent_ids.iter().enumerate() {
        let schedule_i = match &caps {
            Some(c) => {
                let minute: i64 = c[1].parse().unwrap_or(0);
                format!("{}{}", (minute + i as i64 * stagger).rem_euclid(60), &c[2])
            }
            None => schedule.to_string(),
        };
        match create_cron_job(pg, agent_id, name, &schedule_i, prompt).await {
            Ok(_) => results.push(FleetCronResult {
                agent_id: agent_id.clone(),
                ok: true,
                error: None,
            }),
            Err(e) => results.push(FleetCronResult {
                agent_id: agent_id.clone(),
                ok: false,
                error: Some(e),
            }),
        }
    }
    Ok(results)
}

// ── Schedules a conversation owns ───────────────────────────────────────────
// Everything above creates a job for an AGENT: it fires, the agent answers
// itself, and nothing reports. A job created from a CONVERSATION is that
// thread's standing request ("check this every morning"), so its turns have to
// come back into the thread — and `conversation_schedules` is the only pointer
// that can carry them. jobs.json lives inside the agent and knows nothing
// about Talaria's conversations; all a finished cron turn carries back out is
// the task id its scheduler stamped on it, `cron:<job>:<execution>`.

/// One `conversation_schedules` row — what Talaria knows about a cron it
/// created from a thread, before the live state from the container is merged
/// on top of it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRow {
    pub job_id: String,
    pub conversation_id: String,
    pub name: String,
    pub schedule: String,
    /// ISO, like every other timestamp this api serves.
    pub created_at: String,
}

/// The managed, enabled def that answers for a model. A conversation names its
/// agent by model and every `hermes cron` call needs the def id, so this is
/// the one translation both doors of the thread-schedule feature go through.
async fn managed_def_for_model(pg: &PgPool, model: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "select id::text from agent_defs where model = $1 and managed and enabled limit 1",
    )
    .bind(model)
    .fetch_optional(pg)
    .await
}

/// Create a cron on this conversation's agent and record the thread as its
/// listener. Returns the job id Hermes assigned.
pub async fn schedule_from_conversation(
    pg: &PgPool,
    conversation_id: &str,
    created_by: &str,
    name: &str,
    schedule: &str,
    prompt: &str,
) -> Result<String, String> {
    let model: Option<String> =
        sqlx::query_scalar("select agent_model from conversations where id = $1::uuid")
            .bind(conversation_id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    // A thread with no agent, a thread that is gone, and a thread whose agent
    // was retired all land on the same sentence: the only thing the caller can
    // do about any of them is pick a different thread.
    let model = model.unwrap_or_default();
    let Some(def_id) = managed_def_for_model(pg, &model)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Err("this conversation's agent is not a running managed agent".to_string());
    };
    // Every refusal the fleet door answers with — the flag/newline guards, the
    // frequency floor, Hermes' own schedule parser — this door answers with
    // too, verbatim, because it is the same call.
    let Some(job) = create_cron_job(pg, &def_id, name, schedule, prompt).await? else {
        return Err("the agent did not report a job id".to_string());
    };
    // The trimmed text, so the row and jobs.json agree about what was asked
    // for: create_cron_job trims before handing it to Hermes.
    let written = sqlx::query(
        "insert into conversation_schedules \
           (agent_model, job_id, conversation_id, name, schedule, created_by) \
         values ($1, $2, $3::uuid, $4, $5, $6::uuid)",
    )
    .bind(&model)
    .bind(&job)
    .bind(conversation_id)
    .bind(name.trim())
    .bind(schedule.trim())
    .bind(created_by)
    .execute(pg)
    .await;
    if let Err(e) = written {
        // The job is already in the container's jobs.json by now, and a job
        // with no row pointing at a conversation is spend nobody reads: it
        // fires forever, reaches the cron-output door, finds no listener and
        // is dropped. Take it back out rather than leave it ticking. A failed
        // undo is only logged — the caller is owed the write that failed, not
        // the cleanup that followed it, and the job is still visible on the
        // agent's own cron list.
        if let Err(undo) = remove_cron_job(pg, &def_id, &job).await {
            tracing::error!("[crons] orphan job {job} on {model} could not be removed: {undo}");
        }
        return Err(e.to_string());
    }
    Ok(job)
}

/// Take a thread's schedule away: the job goes out of the agent's own
/// scheduler and the row that made the thread its listener goes with it.
/// The row is deleted even when the container cannot be reached — a row
/// pointing at a job that no longer fires lists forever with nothing to
/// show; the job that outlives its row is the one `schedule_from_conversation`
/// guards against, and here the person asked for exactly that removal.
pub async fn unschedule_from_conversation(
    pg: &PgPool,
    conversation_id: &str,
    job_id: &str,
) -> Result<(), String> {
    let row: Option<(String,)> = sqlx::query_as(
        "select agent_model from conversation_schedules \
         where conversation_id = $1::uuid and job_id = $2",
    )
    .bind(conversation_id)
    .bind(job_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    let Some((model,)) = row else {
        return Err("this thread has no such schedule".to_string());
    };
    let removed = match managed_def_for_model(pg, &model)
        .await
        .map_err(|e| e.to_string())?
    {
        Some(def_id) => remove_cron_job(pg, &def_id, job_id).await,
        None => Err("this conversation's agent is not a running managed agent".to_string()),
    };
    if let Err(e) = &removed {
        tracing::warn!("[crons] {conversation_id}: job {job_id} stays in the container: {e}");
    }
    sqlx::query(
        "delete from conversation_schedules where conversation_id = $1::uuid and job_id = $2",
    )
    .bind(conversation_id)
    .bind(job_id)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Take every job a thread scheduled out of its agent's scheduler — the
/// hard-delete / archive counterpart of `schedule_from_conversation`. A
/// `conversation_schedules` row cascades away with its conversation, but
/// the job it points at lives in the container's jobs.json and would keep
/// firing forever: unmetered spend that reaches the cron-output door, finds
/// no listener and is dropped. Best-effort per job, like the create path's
/// undo: a container that cannot be reached is logged and the next job is
/// still tried. Returns the job ids that were removed. Call it BEFORE the
/// row delete (the rows are how the jobs are found); the rows themselves
/// are the caller's to delete or cascade.
///
/// Call site (owned elsewhere): `talaria-routes-comms`'s conversation
/// DELETE, before `delete_conversation` / `archive_conversation`.
pub async fn remove_jobs_for_conversation(pg: &PgPool, conversation_id: &str) -> Vec<String> {
    let rows: Vec<(String, String)> = match sqlx::query_as(
        "select agent_model, job_id from conversation_schedules \
         where conversation_id = $1::uuid",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(
                "[crons] {conversation_id}: schedules could not be read, jobs stay: {e}"
            );
            return Vec::new();
        }
    };
    let mut removed = Vec::new();
    for (model, job_id) in rows {
        let def_id = match managed_def_for_model(pg, &model).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                tracing::warn!(
                    "[crons] {conversation_id}: job {job_id} on {model} stays — not a running managed agent"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!("[crons] {conversation_id}: job {job_id} on {model} stays: {e}");
                continue;
            }
        };
        match remove_cron_job(pg, &def_id, &job_id).await {
            Ok(()) => removed.push(job_id),
            Err(e) => tracing::warn!(
                "[crons] {conversation_id}: job {job_id} stays in the container: {e}"
            ),
        }
    }
    removed
}

/// Pause (or resume) every schedule a thread owns, for archive and restore:
/// an archived thread is one nobody reads, so its jobs stop spending until
/// it comes back. The rows stay — the schedule list still shows them, and a
/// restore resumes exactly the set that was paused. Best-effort per job,
/// like the removal above. Returns how many jobs the container took the
/// change for.
pub async fn set_jobs_paused_for_conversation(
    pg: &PgPool,
    conversation_id: &str,
    paused: bool,
) -> usize {
    let rows = match schedules_for_conversation(pg, conversation_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[crons] {conversation_id}: schedules read failed: {e}");
            return 0;
        }
    };
    let mut changed = 0;
    for row in rows {
        let model: Option<String> = sqlx::query_scalar(
            "select agent_model from conversation_schedules \
             where conversation_id = $1::uuid and job_id = $2",
        )
        .bind(conversation_id)
        .bind(&row.job_id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten();
        let def_id = match model {
            Some(model) => managed_def_for_model(pg, &model).await.ok().flatten(),
            None => None,
        };
        let Some(def_id) = def_id else {
            tracing::warn!(
                "[crons] {conversation_id}: job {} has no running managed agent to pause",
                row.job_id
            );
            continue;
        };
        let done = if paused {
            pause_cron_job(pg, &def_id, &row.job_id).await
        } else {
            resume_cron_job(pg, &def_id, &row.job_id).await
        };
        match done {
            Ok(()) => changed += 1,
            Err(e) => tracing::warn!(
                "[crons] {conversation_id}: job {} could not be {}: {e}",
                row.job_id,
                if paused { "paused" } else { "resumed" }
            ),
        }
    }
    changed
}

/// Which conversation, if any, is listening to this agent's job — the lookup
/// the agent's own call back makes, keyed the way the row is keyed.
pub async fn conversation_for_job(
    pg: &PgPool,
    agent_model: &str,
    job_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "select conversation_id::text from conversation_schedules \
         where agent_model = $1 and job_id = $2",
    )
    .bind(agent_model)
    .bind(job_id)
    .fetch_optional(pg)
    .await
}

/// What this thread has scheduled, oldest first.
pub async fn schedules_for_conversation(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Vec<ScheduleRow>, sqlx::Error> {
    // created_at crosses as epoch millis and is formatted on this side: sqlx
    // carries no timestamptz decoder here by design (the same idiom as
    // agent_keys.last_used_at).
    type Row = (String, String, String, String, i64);
    let rows: Vec<Row> = sqlx::query_as(
        "select job_id, conversation_id::text, name, schedule, \
                (extract(epoch from created_at) * 1000)::bigint \
         from conversation_schedules where conversation_id = $1::uuid \
         order by created_at",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(job_id, conversation_id, name, schedule, created_ms)| ScheduleRow {
                job_id,
                conversation_id,
                name,
                schedule,
                created_at: epoch_ms_to_iso(created_ms),
            },
        )
        .collect())
}

/// One model's live jobs, as the container reports them — the half of a
/// thread's schedule list that only jobs.json knows (still ticking? when did
/// it last run?). Keyed by model, because that is how a conversation names its
/// agent; a down container is an Err the caller may degrade past.
pub async fn live_jobs_for_model(pg: &PgPool, agent_model: &str) -> Result<Vec<CronJob>, String> {
    let Some(def_id) = managed_def_for_model(pg, agent_model)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Err("this conversation's agent is not a running managed agent".to_string());
    };
    list_cron_jobs(pg, &def_id).await
}

/// The task id a cron turn carries, split back into the job and the execution
/// it names: `cron:<job>:<execution>`. None for anything else — a persona
/// turn's task id, an id too short to be a job id, a fourth segment — because
/// the door behind this refuses what it cannot recognise rather than guess
/// which thread a turn belongs to.
pub fn job_id_of_task(task_id: &str) -> Option<(String, String)> {
    let mut parts = task_id.split(':');
    let kind = parts.next()?;
    let job = parts.next()?;
    let execution = parts.next()?;
    if parts.next().is_some() || kind != "cron" || !job_id_ok(job) {
        return None;
    }
    // Execution ids are hex too, but any length — Hermes picks the width.
    if execution.is_empty()
        || !execution
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    {
        return None;
    }
    Some((job.to_string(), execution.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_forms() {
        assert_eq!(min_interval_minutes("30m"), Some(30.0));
        assert_eq!(min_interval_minutes("every 2h"), Some(120.0));
        assert_eq!(min_interval_minutes("90s"), Some(1.5));
        assert_eq!(min_interval_minutes("every  2h "), Some(120.0));
        assert_eq!(min_interval_minutes("bogus"), None);
        // 4 fields is not a cron we can judge.
        assert_eq!(min_interval_minutes("* * * *"), None);
    }

    #[test]
    fn cron_minute_gaps() {
        // One minute value an hour → hourly at most.
        assert_eq!(min_interval_minutes("5 * * * *"), Some(60.0));
        // Every five minutes.
        assert_eq!(min_interval_minutes("*/5 * * * *"), Some(5.0));
        // Wrap past the hour: 58 and 0 are two minutes apart.
        assert_eq!(min_interval_minutes("0,58 * * * *"), Some(2.0));
        // Lists and ranges: 0,15,30,45 → 15.
        assert_eq!(min_interval_minutes("0-45/15 * * * *"), Some(15.0));
        // Unparseable minute field → can't tell.
        assert_eq!(min_interval_minutes("a * * * *"), None);
        assert_eq!(min_interval_minutes("5-2 * * * *"), None);
        // Every minute — the schedule the floor exists to refuse.
        assert_eq!(min_interval_minutes("* * * * *"), Some(1.0));
    }

    #[test]
    fn flag_and_newline_refusals() {
        assert_eq!(
            assert_safe("--flag", "name").unwrap_err(),
            "name cannot start with \"-\""
        );
        assert_eq!(
            assert_safe("two\nlines", "prompt").unwrap_err(),
            "prompt cannot contain newlines"
        );
        assert!(assert_safe("plain text", "name").is_ok());
    }

    #[test]
    fn job_ids_are_lowercase_hex() {
        assert!(job_id_ok("abc123"));
        assert!(job_id_ok("a".repeat(32).as_str()));
        assert!(!job_id_ok("ABC123")); // uppercase is not [a-f]
        assert!(!job_id_ok("abc12")); // too short
        assert!(!job_id_ok("a".repeat(33).as_str()));
    }

    #[test]
    fn cron_task_ids_split_into_job_and_execution() {
        assert_eq!(
            job_id_of_task("cron:abc123:f00dface"),
            Some(("abc123".to_string(), "f00dface".to_string()))
        );
        // A persona turn's task id names no job.
        assert_eq!(job_id_of_task("task:abc123:f00d"), None);
        assert_eq!(job_id_of_task("abc123"), None);
        // Too short to be a job id — the same grammar `hermes cron` prints.
        assert_eq!(job_id_of_task("cron:abc12:f00d"), None);
        // Trailing garbage: a fourth segment, a non-hex execution, nothing.
        assert_eq!(job_id_of_task("cron:abc123:f00d:extra"), None);
        assert_eq!(job_id_of_task("cron:abc123:f00d!"), None);
        assert_eq!(job_id_of_task("cron:abc123:"), None);
        assert_eq!(job_id_of_task("cron:abc123"), None);
    }
}
