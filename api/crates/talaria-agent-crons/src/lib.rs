// Native Hermes crons, managed by Talaria. Every agent's gateway runs Hermes'
// own scheduler (a 60s tick over the agent's cron/jobs.json), so jobs live and
// fire INSIDE the agent — Talaria is just the control surface. It reads and
// changes them through Hermes's cron REST on the agent's own API server
// (`/api/jobs`: list, create, update, delete, pause, resume, run), through the
// same door and with the same bearer a chat turn uses — the manifest row's
// key, which the container holds as its API_SERVER_KEY. The container has to
// be up.
//
// NOT `docker exec hermes cron`, which this used to drive. Since v0.22 the
// CLI's `run` executes the whole turn on the caller's thread, so a run-now
// longer than the exec's 30s bound reported a failure while the run carried
// on; REST `run` only marks the job due for the next tick. A mutation is a
// json body now rather than an argv to guard against option injection, and
// the list no longer depends on the on-disk jobs.json format.
//
// METERING: a cron fires the agent's own loop on the agent's own config,
// i.e. the agent's own gateway key — the same key a chat turn uses (`hermes
// cron create` takes no model/provider/credential, and the scheduler runs
// inside the single `hermes gateway run` process). The relay meters every
// call on that key, and tells a cron's apart by the conversation key Hermes
// stamps on each one (`x-talaria-session`, talaria-gateway usage.rs): a
// fire's is `cron_<job id>`, so its spend lands under the agent, source
// `unattended`, per job — reaching the ledger and the agent's budget whether
// or not a chat turn is open at the same moment.

use std::time::Duration;

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_error::store_fault;
use talaria_gateway::fleet_chat::{Method, agent_request};
use talaria_gateway::settings::get_setting;

/// The bound on one `/api/jobs` call. Hermes answers these in milliseconds,
/// but a write can wait up to 30s on the jobs-file lock while another process
/// in the container holds it (`cron/jobs.py` `_JOBS_LOCK_TIMEOUT_SECONDS`) and
/// still apply afterwards — so the window sits past that wait, and a write
/// Hermes went on to make is never reported as failed.
const JOBS_WINDOW: Duration = Duration::from_secs(40);

/// Hermes's cap on a cron prompt over REST (`api_server.py`
/// `_MAX_PROMPT_LENGTH`), counted the way Python counts a str: code points.
/// The routes bound their prompt members by it too, so a long prompt is
/// refused before it reaches the agent rather than by it.
pub const MAX_PROMPT_CHARS: usize = 5_000;

/// The managed, enabled def's model — the key its manifest row (the door and
/// the bearer) is found by.
async fn agent_for(pg: &PgPool, def_id: &str) -> Result<String, String> {
    let model: Option<String> = sqlx::query_scalar(
        "select model from agent_defs where id = $1::uuid and managed and enabled",
    )
    .bind(def_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| store_fault("[crons] agent read failed", e))?;
    model.ok_or_else(|| "not a running managed agent".to_string())
}

/// One `/api/jobs` call on the agent's own API server. `Ok` is the 2xx body.
/// `Err` is a sentence a person can act on: Hermes's own refusal verbatim
/// (its schedule parser, its prompt cap and scan, "Job not found"), which is
/// the one worth reading, as the CLI's stderr was; or why the agent never
/// answered.
async fn jobs_call(
    model: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let answer = agent_request(model, method, path, body.as_ref(), JOBS_WINDOW)
        .await
        .map_err(|e| format!("cannot reach {model}'s scheduler: {e}"))?;
    if (200..300).contains(&answer.status) {
        return Ok(answer.body);
    }
    Err(refusal_of(answer.status, &answer.body))
}

/// Hermes's refusal as a sentence. `/api/jobs` answers `{"error": "…"}`; the
/// auth wall in front of it answers `{"error": {"message": …}}`.
fn refusal_of(status: u16, body: &Value) -> String {
    let error = body.get("error");
    let said = error
        .and_then(Value::as_str)
        .or_else(|| error.and_then(|e| e.get("message")).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match said {
        Some(s) => s.chars().take(500).collect(),
        None => format!("the agent's scheduler answered {status}"),
    }
}

/// The job record a single-job call answers with (`{"job": {…}}`).
fn job_in(body: Value) -> Result<Value, String> {
    match body {
        Value::Object(mut obj) => obj
            .remove("job")
            .filter(Value::is_object)
            .ok_or_else(|| "the agent's scheduler answered without a job".to_string()),
        _ => Err("the agent's scheduler answered without a job".to_string()),
    }
}

/// A job id the way Hermes's REST addresses one (`api_server.py` `_JOB_ID_RE`
/// is `[a-f0-9]{12}`, and `create_job` mints `uuid4().hex[:12]`). Checked
/// here, so a malformed id is refused before the round-trip — and never
/// reaches a URL path.
fn hermes_job_id(job_id: &str) -> bool {
    job_id.len() == 12
        && job_id
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn jobs_path(job_id: &str) -> Result<String, String> {
    if !hermes_job_id(job_id) {
        return Err("bad job id".to_string());
    }
    Ok(format!("/api/jobs/{job_id}"))
}

/// Hermes's own pause signal: `enabled` false, or the paused state.
fn is_paused(job: &Value) -> bool {
    job.get("enabled").and_then(Value::as_bool) == Some(false)
        || job.get("state").and_then(Value::as_str) == Some("paused")
}

/// A job the scheduler fires by itself: not paused, and still scheduled —
/// not a finished one-shot (`completed`) or a schedule that broke (`error`).
fn is_armed(job: &Value) -> bool {
    !is_paused(job)
        && job
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("scheduled")
            == "scheduled"
}

fn prompt_fits(prompt: &str) -> Result<(), String> {
    let n = prompt.chars().count();
    if n > MAX_PROMPT_CHARS {
        return Err(format!(
            "a scheduled prompt can be at most {MAX_PROMPT_CHARS} characters (the agent's own limit); this one is {n}"
        ));
    }
    Ok(())
}

/// The job as Talaria shows it — every field defaulted from the record
/// Hermes answered with.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// Human schedule as Hermes displays it (cron expr or "every 120m").
    pub schedule: String,
    pub enabled: bool,
    pub state: String,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
}

fn job_from(j: &Value) -> CronJob {
    let str_field =
        |key: &str| -> Option<String> { j.get(key).and_then(|v| v.as_str()).map(String::from) };
    CronJob {
        id: str_field("id").unwrap_or_default(),
        // j.name ?? j.id — ONLY a missing/null name falls to the id;
        // an empty string is a present value and stays.
        name: match j.get("name") {
            Some(Value::String(n)) => n.clone(),
            _ => str_field("id").unwrap_or_default(),
        },
        prompt: str_field("prompt").unwrap_or_default(),
        schedule: j
            .get("schedule_display")
            .and_then(|s| s.as_str())
            .map(String::from)
            .or_else(|| {
                j.get("schedule")
                    .and_then(|s| s.get("display").and_then(|d| d.as_str()).map(String::from))
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
}

pub async fn list_cron_jobs(pg: &PgPool, def_id: &str) -> Result<Vec<CronJob>, String> {
    let model = agent_for(pg, def_id).await?;
    // include_disabled: a paused job is `enabled: false`, and Hermes's
    // default list leaves those out.
    let body = jobs_call(&model, Method::GET, "/api/jobs?include_disabled=true", None).await?;
    Ok(body
        .get("jobs")
        .and_then(Value::as_array)
        .map(|jobs| jobs.iter().map(job_from).collect())
        .unwrap_or_default())
}

// ── Frequency floor ─────────────────────────────────────────────────────────
// A cron job is an agent turn, and an agent turn is LLM spend. `* * * * *`
// used to be accepted verbatim: 1,440 turns a day, per agent, forever — the
// cheapest way in the product to run up an unbounded bill, and the schedule a
// well-meaning "check this often" request lands on. The floor is the coarse
// guard; the gateway budget is the fine one.
//
// THE FLOOR JUDGES WHAT HERMES PARSED, AND FAILS CLOSED. It used to read the
// schedule string with a grammar of its own and wave through anything that
// grammar could not read — and Hermes accepts more than it did: a bare unit
// (`every minute` is a one-minute interval), croniter's `0/1` (every minute
// from :00), a 6-field cron whose last field is seconds. Now a job is written
// paused, the floor reads the schedule Hermes stored, and only then does the
// job start; a shape the floor cannot judge is refused, not trusted.
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

/// Expand one cron field into the values it fires on, in croniter's grammar
/// (croniter is what Hermes validates an expression with): a comma list of
/// `*`, `N` and `A-B`, each optionally `/STEP` — where `N/STEP` reads as
/// `N-max/STEP`, so `0/1` is every value. None for anything else, and the
/// caller refuses what it cannot read.
fn cron_field_values(field: &str, max: u32) -> Option<Vec<u32>> {
    let mut out: Vec<u32> = Vec::new();
    for part in field.split(',') {
        let (spec, step) = match part.split_once('/') {
            Some((spec, step)) => (spec, Some(step.parse::<u32>().ok().filter(|s| *s >= 1)?)),
            None => (part, None),
        };
        let (lo, hi) = if spec == "*" {
            (0, max)
        } else if let Some((a, b)) = spec.split_once('-') {
            (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)
        } else {
            let n = spec.parse::<u32>().ok()?;
            (n, if step.is_some() { max } else { n })
        };
        if lo > hi || hi > max {
            return None;
        }
        out.extend((lo..=hi).step_by(step.unwrap_or(1) as usize));
    }
    out.sort_unstable();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

/// The shortest gap, in minutes, between two fires of a 5-field cron
/// expression, read off its minute and hour fields. Exact over a day when the
/// other three fields are `*`; with any of them set it is a LOWER BOUND on
/// the real gap — a day-of-month, month or weekday field only takes fires
/// away, and removing fires never shortens the gap between the ones left.
/// That is the safe direction for a floor.
fn cron_gap_minutes(expr: &str) -> Result<u32, String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!("a {}-field cron expression", fields.len()));
    }
    let (Some(minutes), Some(hours)) = (
        cron_field_values(fields[0], 59),
        cron_field_values(fields[1], 23),
    ) else {
        return Err(format!(
            "a cron expression whose minute or hour field cannot be read (\"{expr}\")"
        ));
    };
    // Minutes of the day it fires on, ascending: hours and minutes both are.
    let fires: Vec<u32> = hours
        .iter()
        .flat_map(|h| minutes.iter().map(move |m| h * 60 + m))
        .collect();
    // Past midnight, into the next day's first fire. One fire a day is 1440.
    let mut gap = 1440 - fires[fires.len() - 1] + fires[0];
    for pair in fires.windows(2) {
        gap = gap.min(pair[1] - pair[0]);
    }
    Ok(gap)
}

/// The shortest gap, in minutes, between two fires of a schedule as Hermes
/// stored it (`cron/jobs.py` `parse_schedule`: `{kind: interval, minutes}`,
/// `{kind: cron, expr}` or `{kind: once, run_at}`). `Ok(None)` is a one-shot,
/// which no floor applies to; `Err` names a shape the floor cannot judge.
pub fn parsed_gap_minutes(schedule: &Value) -> Result<Option<u32>, String> {
    match schedule.get("kind").and_then(Value::as_str) {
        Some("once") => Ok(None),
        Some("interval") => match schedule.get("minutes").and_then(Value::as_u64) {
            Some(m) if m > 0 => Ok(Some(u32::try_from(m).unwrap_or(u32::MAX))),
            _ => Err("an interval with no length".to_string()),
        },
        Some("cron") => cron_gap_minutes(
            schedule
                .get("expr")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .map(Some),
        Some(kind) => Err(format!("a '{kind}' schedule")),
        None => Err("a schedule with no kind".to_string()),
    }
}

/// The floor's verdict on a schedule Hermes parsed. Only asked when a floor is
/// set (`floor > 0`).
fn floor_verdict(schedule: Option<&Value>, floor: f64) -> Result<(), String> {
    match parsed_gap_minutes(schedule.unwrap_or(&Value::Null)) {
        Ok(None) => Ok(()),
        Ok(Some(gap)) if f64::from(gap) >= floor => Ok(()),
        // Numbers print whole here: 5 stays "5", 5.5 "5.5" — f64 Display
        // does the same for this range.
        Ok(Some(gap)) => Err(format!(
            "schedule fires every {gap}m, faster than the {floor}m minimum an admin set — ask them to lower the floor or pick a slower schedule"
        )),
        Err(shape) => Err(format!(
            "schedule is {shape}, which the {floor}m minimum an admin set cannot be checked against — use an interval like 'every 30m' or a 5-field cron expression"
        )),
    }
}

/// Run a change that takes several calls on the agent to its end, whatever
/// happens to the request that asked for it. A guarded create (write paused →
/// judge → start) and a schedule edit (pause → patch → judge → resume, or put
/// back) leave the job paused between their calls, and a request future
/// dropped there — the client hung up — would strand it paused: failing
/// closed, but with nobody told. A spawned task is not dropped with the
/// request, so the sequence finishes. Only the process itself exiting can
/// still cut one short, and that leaves a paused job: no spend, and shown as
/// paused on its list.
async fn detached<T: Send + 'static>(
    work: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
) -> Result<T, String> {
    tokio::spawn(work)
        .await
        .map_err(|e| format!("the change on the agent did not finish: {e}"))?
}

/// Create a job on the agent and return the id Hermes assigned.
///
/// With a floor set, the job is written PAUSED: the floor reads the schedule
/// Hermes parsed, and a job must not tick even once on a schedule the floor
/// then refuses. It starts (`resume`) only once the floor passes it; a
/// refusal, or a start Hermes refuses, takes the paused job back out.
pub async fn create_cron_job(
    pg: &PgPool,
    def_id: &str,
    name: &str,
    schedule: &str,
    prompt: &str,
) -> Result<String, String> {
    let model = agent_for(pg, def_id).await?;
    let name = name.trim();
    let schedule = schedule.trim();
    let prompt = prompt.trim();
    if name.is_empty() || schedule.is_empty() || prompt.is_empty() {
        return Err("name, schedule, and prompt are required".to_string());
    }
    prompt_fits(prompt)?;
    let floor = cron_floor_minutes(pg).await;
    let body = json!({
        "name": name,
        "schedule": schedule,
        "prompt": prompt,
        "deliver": "local",
        "paused": floor > 0.0,
    });
    detached(create_and_start(model, body, floor)).await
}

/// A create's calls on the agent: the write (paused when a floor is set), the
/// floor's verdict on what Hermes parsed, then the start — or the paused job
/// taken back out.
async fn create_and_start(model: String, body: Value, floor: f64) -> Result<String, String> {
    let guarded = floor > 0.0;
    let created = job_in(jobs_call(&model, Method::POST, "/api/jobs", Some(body)).await?)?;
    let Some(id) = created
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| hermes_job_id(id))
        .map(str::to_string)
    else {
        return Err("the agent did not report a job id".to_string());
    };
    if !guarded {
        return Ok(id);
    }
    let started = match floor_verdict(created.get("schedule"), floor) {
        Ok(()) => jobs_call(
            &model,
            Method::POST,
            &format!("/api/jobs/{id}/resume"),
            None,
        )
        .await
        .map(|_| ()),
        Err(refusal) => Err(refusal),
    };
    if let Err(why) = started {
        if let Err(e) = jobs_call(&model, Method::DELETE, &format!("/api/jobs/{id}"), None).await {
            tracing::error!("[crons] refused job {id} on {model} could not be removed: {e}");
        }
        return Err(why);
    }
    Ok(id)
}

/// Edit in place — name, schedule, prompt, any subset.
///
/// A SCHEDULE CHANGE IS JUDGED ON WHAT HERMES PARSED, as a create is, and the
/// job must not tick on a schedule the floor then refuses: an armed job is
/// paused first, patched, judged and resumed. A refusal — the floor's, or
/// Hermes's own at the resume — puts back the fields the edit changed and
/// leaves the job armed as it was. A schedule sent back exactly as Hermes
/// displays it is no change, and is dropped from the patch: a client that
/// sends every field must not re-parse, re-time or re-judge a schedule nobody
/// touched (the app's own edit form sends only the fields that changed).
pub async fn edit_cron_job(
    pg: &PgPool,
    def_id: &str,
    job_id: &str,
    name: Option<&str>,
    schedule: Option<&str>,
    prompt: Option<&str>,
) -> Result<(), String> {
    let path = jobs_path(job_id)?;
    let model = agent_for(pg, def_id).await?;
    let mut patch = serde_json::Map::new();
    for (key, value) in [("name", name), ("prompt", prompt)] {
        if let Some(value) = value.map(str::trim) {
            if value.is_empty() {
                return Err(format!("{key} cannot be blank"));
            }
            patch.insert(key.into(), json!(value));
        }
    }
    if let Some(prompt) = prompt {
        prompt_fits(prompt.trim())?;
    }
    let schedule = schedule.map(str::trim);
    if schedule == Some("") {
        return Err("schedule cannot be blank".to_string());
    }
    let Some(schedule) = schedule else {
        if patch.is_empty() {
            return Err("nothing to edit".to_string());
        }
        return jobs_call(&model, Method::PATCH, &path, Some(Value::Object(patch)))
            .await
            .map(|_| ());
    };
    let floor = cron_floor_minutes(pg).await;
    detached(edit_with_schedule(
        model,
        path,
        patch,
        schedule.to_string(),
        floor,
    ))
    .await
}

/// An edit's calls on the agent once it names a schedule: read the job, drop
/// the schedule if it is Hermes's own display string, then either one PATCH
/// or — with a floor set and the schedule changing — pause, patch, judge, and
/// resume or put back.
async fn edit_with_schedule(
    model: String,
    path: String,
    mut patch: serde_json::Map<String, Value>,
    schedule: String,
    floor: f64,
) -> Result<(), String> {
    let before = job_in(jobs_call(&model, Method::GET, &path, None).await?)?;
    if before.get("schedule_display").and_then(Value::as_str) != Some(schedule.as_str()) {
        patch.insert("schedule".into(), json!(schedule));
    }
    if patch.is_empty() {
        return Ok(());
    }
    if !patch.contains_key("schedule") || floor <= 0.0 {
        return jobs_call(&model, Method::PATCH, &path, Some(Value::Object(patch)))
            .await
            .map(|_| ());
    }

    let armed = is_armed(&before);
    if armed {
        jobs_call(&model, Method::POST, &format!("{path}/pause"), None).await?;
    }
    let patched = jobs_call(
        &model,
        Method::PATCH,
        &path,
        Some(Value::Object(patch.clone())),
    )
    .await
    .and_then(job_in);
    let patched = match patched {
        Ok(job) => job,
        Err(why) => {
            // Nothing changed but the pause.
            if armed && let Err(e) = resume(&model, &path).await {
                return Err(format!("{why} (and the job could not be resumed: {e})"));
            }
            return Err(why);
        }
    };
    let verdict = match floor_verdict(patched.get("schedule"), floor) {
        Ok(()) if armed => resume(&model, &path).await,
        other => other,
    };
    let Err(why) = verdict else {
        return Ok(());
    };
    // The edit cannot stand: the fields it changed go back to what they were
    // (the schedule as Hermes's own parsed record, which it takes as-is).
    let restore: serde_json::Map<String, Value> = patch
        .keys()
        .filter_map(|k| before.get(k).map(|v| (k.clone(), v.clone())))
        .collect();
    if let Err(e) = jobs_call(&model, Method::PATCH, &path, Some(Value::Object(restore))).await {
        tracing::error!("[crons] {path} on {model}: refused edit could not be undone: {e}");
        return Err(format!(
            "{why} (and the job's previous settings could not be put back: {e})"
        ));
    }
    if armed && let Err(e) = resume(&model, &path).await {
        return Err(format!("{why} (and the job could not be resumed: {e})"));
    }
    Err(why)
}

async fn resume(model: &str, path: &str) -> Result<(), String> {
    jobs_call(model, Method::POST, &format!("{path}/resume"), None)
        .await
        .map(|_| ())
}

async fn job_action(
    pg: &PgPool,
    def_id: &str,
    job_id: &str,
    method: Method,
    action: &str,
) -> Result<(), String> {
    let path = jobs_path(job_id)?;
    let model = agent_for(pg, def_id).await?;
    jobs_call(&model, method, &format!("{path}{action}"), None)
        .await
        .map(|_| ())
}

pub async fn remove_cron_job(pg: &PgPool, def_id: &str, job_id: &str) -> Result<(), String> {
    job_action(pg, def_id, job_id, Method::DELETE, "").await
}
pub async fn pause_cron_job(pg: &PgPool, def_id: &str, job_id: &str) -> Result<(), String> {
    job_action(pg, def_id, job_id, Method::POST, "/pause").await
}
pub async fn resume_cron_job(pg: &PgPool, def_id: &str, job_id: &str) -> Result<(), String> {
    job_action(pg, def_id, job_id, Method::POST, "/resume").await
}

/// Queue the job for the scheduler's next tick (≤60s): REST `run` marks it
/// due now and returns — the turn runs on the tick, never inside this call.
///
/// A PAUSED JOB IS REFUSED. Hermes's `run` (`trigger_job`) un-pauses the job
/// as it queues it — `enabled: true`, `state: scheduled` — and nothing pauses
/// it again, so one "run now" would resume a paused job for good (a thread's
/// archive pauses its schedules precisely so they stop spending). Pausing it
/// again after the trigger is no answer: the fire happens on a later tick,
/// and a pause before that tick cancels it. Resume first, then run.
pub async fn run_cron_job(pg: &PgPool, def_id: &str, job_id: &str) -> Result<(), String> {
    let path = jobs_path(job_id)?;
    let model = agent_for(pg, def_id).await?;
    let job = job_in(jobs_call(&model, Method::GET, &path, None).await?)?;
    if is_paused(&job) {
        return Err(
            "this job is paused — resume it first; running a paused job would resume it for good"
                .to_string(),
        );
    }
    jobs_call(&model, Method::POST, &format!("{path}/run"), None)
        .await
        .map(|_| ())
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
/// agent by model and every job call needs the def id, so this is
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
            .map_err(|e| store_fault("[crons] conversation read failed", e))?;
    // A thread with no agent, a thread that is gone, and a thread whose agent
    // was retired all land on the same sentence: the only thing the caller can
    // do about any of them is pick a different thread.
    let model = model.unwrap_or_default();
    let Some(def_id) = managed_def_for_model(pg, &model)
        .await
        .map_err(|e| store_fault("[crons] agent read failed", e))?
    else {
        return Err("this conversation's agent is not a running managed agent".to_string());
    };
    // Every refusal the fleet door answers with — the prompt cap, the
    // frequency floor, Hermes' own schedule parser and prompt scan — this door
    // answers with too, verbatim, because it is the same call.
    let job = create_cron_job(pg, &def_id, name, schedule, prompt).await?;
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
        return Err(store_fault("[crons] schedule row write failed", e));
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
    .map_err(|e| store_fault("[crons] schedule read failed", e))?;
    let Some((model,)) = row else {
        return Err("this thread has no such schedule".to_string());
    };
    let removed = match managed_def_for_model(pg, &model)
        .await
        .map_err(|e| store_fault("[crons] agent read failed", e))?
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
    .map_err(|e| store_fault("[crons] schedule row delete failed", e))?;
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

/// The thread listening to a job, and the person who set the job up — the
/// one who asked for the output, and so the one a landed turn answers to.
#[derive(Debug, Clone)]
pub struct JobListener {
    pub conversation_id: String,
    /// `conversation_schedules.created_by`: None only for a row written
    /// before the column was stamped.
    pub created_by: Option<String>,
}

/// Which conversation, if any, is listening to this agent's job — the lookup
/// the agent's own call back makes, keyed the way the row is keyed — and who
/// scheduled it.
pub async fn conversation_for_job(
    pg: &PgPool,
    agent_model: &str,
    job_id: &str,
) -> Result<Option<JobListener>, sqlx::Error> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "select conversation_id::text, created_by::text from conversation_schedules \
         where agent_model = $1 and job_id = $2",
    )
    .bind(agent_model)
    .bind(job_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(conversation_id, created_by)| JobListener {
        conversation_id,
        created_by,
    }))
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

/// One model's live jobs, as the agent's scheduler reports them — the half of
/// a thread's schedule list only the agent knows (still ticking? when did
/// it last run?). Keyed by model, because that is how a conversation names its
/// agent; a down container is an Err the caller may degrade past.
pub async fn live_jobs_for_model(pg: &PgPool, agent_model: &str) -> Result<Vec<CronJob>, String> {
    let Some(def_id) = managed_def_for_model(pg, agent_model)
        .await
        .map_err(|e| store_fault("[crons] agent read failed", e))?
    else {
        return Err("this conversation's agent is not a running managed agent".to_string());
    };
    list_cron_jobs(pg, &def_id).await
}

/// The job grammar of a cron turn's task id: lowercase hex, 6 to 32 long.
/// Looser than `hermes_job_id` on purpose — this reads ids back out of turns
/// already running, and refusing one here only drops a turn into the void.
fn job_id_ok(job_id: &str) -> bool {
    (6..=32).contains(&job_id.len())
        && job_id
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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

    fn cron(expr: &str) -> Value {
        json!({ "kind": "cron", "expr": expr, "display": expr })
    }

    #[test]
    fn an_interval_is_judged_on_the_minutes_hermes_parsed() {
        // `every minute` is what Hermes stores for a bare unit: an interval
        // of one minute. The old string grammar needed a digit and let it by.
        let every_minute = json!({ "kind": "interval", "minutes": 1, "display": "every 1m" });
        assert_eq!(parsed_gap_minutes(&every_minute), Ok(Some(1)));
        assert_eq!(
            parsed_gap_minutes(&json!({ "kind": "interval", "minutes": 120 })),
            Ok(Some(120))
        );
        // `every 0m` parses in Hermes — and would fire on every tick.
        assert!(parsed_gap_minutes(&json!({ "kind": "interval", "minutes": 0 })).is_err());
        assert!(parsed_gap_minutes(&json!({ "kind": "interval" })).is_err());
        // A one-shot fires once: no floor applies.
        assert_eq!(
            parsed_gap_minutes(&json!({ "kind": "once", "run_at": "2026-10-23T09:00:00+00:00" })),
            Ok(None)
        );
    }

    #[test]
    fn a_cron_gap_reads_the_minute_and_hour_fields() {
        let gap = |expr: &str| parsed_gap_minutes(&cron(expr));
        assert_eq!(gap("* * * * *"), Ok(Some(1)));
        assert_eq!(gap("*/5 * * * *"), Ok(Some(5)));
        // One minute value an hour → hourly.
        assert_eq!(gap("5 * * * *"), Ok(Some(60)));
        // Wrap past the hour: 58 and 0 are two minutes apart.
        assert_eq!(gap("0,58 * * * *"), Ok(Some(2)));
        // Lists and ranges: 0,15,30,45 → 15.
        assert_eq!(gap("0-45/15 * * * *"), Ok(Some(15)));
        // croniter reads `N/STEP` as `N-max/STEP`: `0/1` is every minute.
        // The old parser read it as just minute 0 — hourly — and let it by.
        assert_eq!(gap("0/1 * * * *"), Ok(Some(1)));
        assert_eq!(gap("10/20 * * * *"), Ok(Some(20)));
        // Every minute of one hour, every day.
        assert_eq!(gap("* 9 * * *"), Ok(Some(1)));
        // The hour field counts: daily is a day, twice daily is the shorter
        // of its two gaps, and the wrap runs past midnight.
        assert_eq!(gap("0 9 * * *"), Ok(Some(1440)));
        assert_eq!(gap("0 9,17 * * *"), Ok(Some(480)));
        assert_eq!(gap("0 0,23 * * *"), Ok(Some(60)));
        // A weekday field only removes fires: the day's gap is a floor on it.
        assert_eq!(gap("0 9 * * 1"), Ok(Some(1440)));
        assert_eq!(gap("0 9 * * MON-FRI"), Ok(Some(1440)));
    }

    #[test]
    fn a_cron_the_floor_cannot_read_is_refused() {
        let gap = |expr: &str| parsed_gap_minutes(&cron(expr));
        // 6 fields: croniter's sixth is seconds. Hermes accepts it.
        assert!(gap("* * * * * *").is_err());
        assert!(gap("* * * * * */10").is_err());
        assert!(gap("* * * *").is_err());
        assert!(gap("a * * * *").is_err());
        assert!(gap("5-2 * * * *").is_err());
        assert!(gap("*/0 * * * *").is_err());
        assert!(gap("60 * * * *").is_err());
        assert!(gap("0 24 * * *").is_err());
        assert!(gap("1,,2 * * * *").is_err());
        assert!(parsed_gap_minutes(&json!({ "kind": "weird" })).is_err());
        assert!(parsed_gap_minutes(&Value::Null).is_err());
    }

    #[test]
    fn the_floor_refuses_too_fast_and_unreadable_and_passes_the_rest() {
        let every_minute = json!({ "kind": "interval", "minutes": 1 });
        let refused = floor_verdict(Some(&every_minute), 5.0).unwrap_err();
        assert!(refused.contains("every 1m"), "{refused}");
        assert!(refused.contains("5m minimum"), "{refused}");
        assert!(floor_verdict(Some(&json!({ "kind": "interval", "minutes": 5 })), 5.0).is_ok());
        assert!(floor_verdict(Some(&json!({ "kind": "once" })), 5.0).is_ok());
        let unreadable = floor_verdict(Some(&cron("* * * * * *")), 5.0).unwrap_err();
        assert!(unreadable.contains("6-field"), "{unreadable}");
        assert!(unreadable.contains("cannot be checked"), "{unreadable}");
        // No schedule in the answer at all is unreadable, not a pass.
        assert!(floor_verdict(None, 5.0).is_err());
        // A floor above an hour no longer refuses a daily cron.
        assert!(floor_verdict(Some(&cron("0 9 * * *")), 120.0).is_ok());
        assert!(floor_verdict(Some(&cron("0 * * * *")), 120.0).is_err());
    }

    #[test]
    fn job_ids_are_addressed_in_hermes_grammar() {
        assert!(hermes_job_id("0123456789ab"));
        assert!(!hermes_job_id("0123456789a")); // 11
        assert!(!hermes_job_id("0123456789abc")); // 13
        assert!(!hermes_job_id("0123456789AB")); // uppercase
        assert!(!hermes_job_id("../../jobs.x"));
        assert_eq!(jobs_path("0123456789ab").unwrap(), "/api/jobs/0123456789ab");
        assert_eq!(jobs_path("abc/../x").unwrap_err(), "bad job id");
    }

    #[test]
    fn a_refusal_reads_as_hermes_said_it() {
        assert_eq!(
            refusal_of(400, &json!({ "error": "Invalid schedule 'x'." })),
            "Invalid schedule 'x'."
        );
        // The auth wall's shape.
        assert_eq!(
            refusal_of(
                401,
                &json!({ "error": { "message": "Invalid gateway API key (API_SERVER_KEY)" } })
            ),
            "Invalid gateway API key (API_SERVER_KEY)"
        );
        assert_eq!(
            refusal_of(502, &Value::Null),
            "the agent's scheduler answered 502"
        );
        assert_eq!(
            refusal_of(500, &json!({ "error": "  " })),
            "the agent's scheduler answered 500"
        );
    }

    #[test]
    fn paused_and_armed_read_hermes_own_signals() {
        let job = |enabled: bool, state: &str| json!({ "enabled": enabled, "state": state });
        assert!(is_paused(&job(false, "paused")));
        assert!(is_paused(&job(false, "scheduled")));
        assert!(!is_paused(&job(true, "scheduled")));
        assert!(is_armed(&job(true, "scheduled")));
        assert!(!is_armed(&job(false, "paused")));
        assert!(!is_armed(&job(true, "completed")));
        assert!(!is_armed(&job(true, "error")));
    }

    #[test]
    fn a_prompt_past_the_agents_cap_is_refused_here() {
        assert!(prompt_fits(&"x".repeat(MAX_PROMPT_CHARS)).is_ok());
        let refused = prompt_fits(&"x".repeat(MAX_PROMPT_CHARS + 1)).unwrap_err();
        assert!(refused.contains("5000"), "{refused}");
        // Counted as Python counts a str: one code point each.
        assert!(prompt_fits(&"é".repeat(MAX_PROMPT_CHARS)).is_ok());
    }

    #[test]
    fn a_job_record_maps_onto_the_row_talaria_shows() {
        let j = job_from(&json!({
            "id": "0123456789ab",
            "name": "recap",
            "prompt": "write it",
            "schedule": { "kind": "interval", "minutes": 120, "display": "every 120m" },
            "schedule_display": "every 120m",
            "enabled": false,
            "state": "paused",
            "next_run_at": null,
            "last_status": "ok",
        }));
        assert_eq!(j.id, "0123456789ab");
        assert_eq!(j.schedule, "every 120m");
        assert!(!j.enabled);
        assert_eq!(j.state, "paused");
        assert_eq!(j.next_run_at, None);
        assert_eq!(j.last_status.as_deref(), Some("ok"));
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
        // Too short to be a job id.
        assert_eq!(job_id_of_task("cron:abc12:f00d"), None);
        // Trailing garbage: a fourth segment, a non-hex execution, nothing.
        assert_eq!(job_id_of_task("cron:abc123:f00d:extra"), None);
        assert_eq!(job_id_of_task("cron:abc123:f00d!"), None);
        assert_eq!(job_id_of_task("cron:abc123:"), None);
        assert_eq!(job_id_of_task("cron:abc123"), None);
    }
}
