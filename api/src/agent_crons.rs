// Native Hermes crons, managed by Talaria. Every agent's gateway runs Hermes'
// own scheduler (ticks /opt/data/cron/jobs.json every 60s), so jobs live and
// fire INSIDE the agent — Talaria is just the control surface: it reads
// jobs.json for truth and drives the `hermes cron` CLI for mutations through
// the running container (docker exec). Requires the container to be up.
// Port of ui/src/server/agent-crons.ts.
//
// METERING CAVEAT (kept from the TS module): a cron fires the agent's own
// loop on the agent's own config, i.e. the PERSONA gateway key, which the
// gateway leaves unmetered because a Talaria flow normally writes that turn's
// ledger row. A cron has no flow, so its spend reaches no ledger. Not fixable
// from here: `hermes cron create` takes no model/provider/credential, and the
// scheduler runs inside the single `hermes gateway run` process — so at the
// gateway a cron turn is byte-identical to a persona turn.

use sqlx::PgPool;

use crate::fleet_docker::{docker_exec, managed_container};
use crate::gateway::settings::get_setting;

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
/// jobs.json entry exactly as the TS mapper does.
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
    let raw = serde_json::from_str::<serde_json::Value>(&stdout)
        .map_err(|e| e.to_string())?;
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
/// JS's Set holds the floats it accumulated (`v += step` with a fractional
/// step lands on 1.5, 4.5, …) and the gap arithmetic runs on them.
fn cron_field_values(field: &str, max: i64) -> Vec<f64> {
    // A JS Set, as a sorted vec: f64 isn't Ord, so the dedup is by equality
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
            match step_raw.parse::<f64>() {
                Ok(v) => v,
                Err(_) => f64::NAN,
            }
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
                // JS Number('') is 0, but an empty spec never reaches here
                // except as lo of "‑5", which parses; NaN fails below.
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
/// can prove is too fast). Port of minIntervalMinutes.
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

/// Refuse a schedule we can PROVE fires faster than the floor lets it.
async fn assert_schedule_allowed(pg: &PgPool, schedule: &str) -> Result<(), String> {
    let Some(gap) = min_interval_minutes(schedule) else {
        return Ok(());
    };
    let floor = cron_floor_minutes(pg).await;
    if floor > 0.0 && gap < floor {
        // JS template literals print numbers whole: 5 stays "5", 5.5 "5.5" —
        // Rust's f64 Display does the same for this range.
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
    let mut args: Vec<String> = vec![
        "hermes".into(),
        "cron".into(),
        "edit".into(),
        job_id.into(),
    ];
    if let Some(name) = name {
        assert_safe(name, "name")?;
        args.push("--name".into());
        args.push(name.trim().to_string());
    }
    if let Some(schedule) = schedule {
        assert_safe(schedule, "schedule")?;
        assert_schedule_allowed(pg, schedule).await?;
        args.push("--schedule".into());
        args.push(schedule.trim().to_string());
    }
    if let Some(prompt) = prompt {
        if prompt.starts_with('-') {
            return Err("prompt cannot start with \"-\"".to_string());
        }
        args.push("--prompt".into());
        args.push(prompt.trim().to_string());
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
    let mut out = Vec::new();
    for (id, slug, display_name) in defs {
        match list_cron_jobs(pg, &id).await {
            Ok(jobs) => out.push(FleetCronAgent {
                id,
                slug,
                display_name,
                jobs,
                error: None,
            }),
            Err(e) => out.push(FleetCronAgent {
                id,
                slug,
                display_name,
                jobs: Vec::new(),
                error: Some(e),
            }),
        }
    }
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
}
