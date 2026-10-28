// THE PULL: a finished cron run whose report never reached its thread.
//
// Every five minutes, for each managed agent that has a thread listening to
// one of its jobs, three reads on the agent's own API server (the same door
// and bearer as its cron REST):
//
//   GET /api/jobs?include_disabled=true   the jobs' current names and prompts
//   GET /api/sessions?source=cron&limit=… its recent cron sessions, newest first
//   GET /api/sessions/{id}/messages       only for a session that needs landing
//
// A session is the pull's to land when it has ENDED (and settled: a push is
// sent at the turn's end, before Hermes closes the session, so two minutes is
// past any push that is going to arrive), it began after the thread started
// listening and within the look-back, it belongs to a listened job, and no
// row in the thread already carries it.
//
// WHICH JOB. Hermes's session listing does not name the job: the session id is
// a digest (`local-` + sha256 of the profile, the cron principal and
// `cron:<job>:<execution>`), and there is no executions route. The title is
// what it carries — `"<job name, whitespace folded, first 60 chars> · <Mon DD
// HH:MM>"` (`cron/scheduler.py`), with `cron <job id>` as Hermes's fallback.
// So a session is matched by that name against EVERY job the agent has, not
// only the listened ones: a fleet-wide job that shares a thread job's name
// must not have its runs landed in that thread. When more than one job has
// the name, the run's first user message (the scheduler's preamble, then the
// job's prompt verbatim) decides — the longest prompt it contains — and a run
// still ambiguous after that is left alone rather than guessed into a thread.
// The run is landed only when the job it belongs to is a listened one.
//
// ALREADY LANDED means either a row carrying the session id (a push since this
// shipped, or an earlier pull), or a row of the same job that landed while
// the session was running. The second covers a push that sent no session id
// and every report pushed before session ids were recorded, so the first pass
// after an upgrade lands nothing twice. A job's runs never overlap (Hermes
// holds one fire claim per job), so a row of that job inside that window is
// that run's.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_auth::now_ms;
use talaria_agent_crons::JobListener;
use talaria_gateway::fleet_chat::{Method, agent_request};
use talaria_state::AppState;

use crate::{CronReport, LOG, Via, land, pull_marker};

/// The pull's cadence: a lost report lands within five minutes of its run
/// settling, and an idle install pays two small reads per listening agent.
pub const PULL_EVERY_MS: u64 = 5 * 60_000;
/// How long after a session ends before the pull may land it. The push goes
/// out at the turn's end, before Hermes closes the session, and its POST is
/// bounded at 4 s — two minutes is past any push that is still coming.
pub const SETTLE_MS: i64 = 2 * 60_000;
/// How far back the pull looks. A run that ended longer ago than this while
/// the api was down is not landed: a day-old digest is not news, and the
/// listing is bounded anyway.
pub const LOOKBACK_MS: i64 = 24 * 60 * 60_000;
/// Sessions asked for per agent (Hermes caps the page at 200). With the
/// five-minute frequency floor, a hundred cron sessions is hours of a busy
/// agent's schedule, far past one pull's interval.
const SESSIONS_PAGE: u32 = 100;
/// The bound on one read. Hermes answers these from its own SQLite in
/// milliseconds; this is a stuck container, not a slow query.
const READ_WINDOW: Duration = Duration::from_secs(20);
/// Slack on the time comparisons: the thread's listener row is written just
/// after Hermes creates the job, and the push lands just after its turn.
const SLACK_MS: i64 = 30_000;

/// One of the agent's jobs, as the pull matches a session against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentJob {
    pub id: String,
    /// The job's name as Hermes holds it now (for a listened job Hermes no
    /// longer lists, the listener row's copy).
    pub name: String,
    /// The job's prompt, when Hermes lists the job — the tiebreak.
    pub prompt: Option<String>,
}

/// A thread listening to one of the agent's jobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub job_id: String,
    pub conversation_id: String,
    pub created_by: Option<String>,
    /// When the thread started listening, epoch ms.
    pub listening_since_ms: i64,
}

/// One finished cron session from the agent's listing.
#[derive(Debug, Clone, PartialEq)]
pub struct CronSession {
    pub id: String,
    pub title: String,
    pub started_ms: i64,
    pub ended_ms: i64,
}

/// Hermes stores session times as epoch SECONDS (a float). A number or a
/// numeric string reads; anything else is None.
fn epoch_ms(v: Option<&Value>) -> Option<i64> {
    let secs = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (secs.is_finite() && secs > 0.0).then_some((secs * 1000.0) as i64)
}

/// A session id the pull may put in a URL path: Hermes's `local-<hex>`, or
/// a legacy `cron_<job>_<stamp>`. Anything else is not addressed at all.
fn session_id_ok(id: &str) -> bool {
    (1..=160).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The ended cron sessions in a `/api/sessions` page. A row without an id,
/// a start or an end is not one the pull can land.
pub fn ended_sessions(page: &Value) -> Vec<CronSession> {
    let Some(rows) = page.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_str)?;
            if !session_id_ok(id) {
                return None;
            }
            Some(CronSession {
                id: id.to_string(),
                title: row
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                started_ms: epoch_ms(row.get("started_at"))?,
                ended_ms: epoch_ms(row.get("ended_at"))?,
            })
        })
        .collect()
}

/// The job name as Hermes puts it into a cron session's title: whitespace
/// runs folded to one space, the first 60 characters, trimmed
/// (`" ".join(job_name.split())[:60].strip()`).
pub fn title_base(name: &str) -> String {
    let folded = name.split_whitespace().collect::<Vec<_>>().join(" ");
    folded
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Does this session title name this job? The run title is `<base> · <Mon
/// DD HH:MM>`; Hermes's fallbacks are `cron <job id>` and `cron <job id>
/// <six chars of the session id>`.
pub fn title_names_job(title: &str, job_id: &str, name: &str) -> bool {
    let title = title.trim();
    let base = title.rsplit_once(" · ").map_or(title, |(base, _)| base);
    let fallback = format!("cron {job_id}");
    if base == fallback || base.starts_with(&format!("{fallback} ")) {
        return true;
    }
    let want = title_base(name);
    !want.is_empty() && base == want
}

/// Which of the agent's jobs a session's title names.
#[derive(Debug, Clone, PartialEq)]
pub enum Match<'a> {
    /// Exactly one job has the name.
    One(&'a AgentJob),
    /// More than one does: the prompt decides (`by_prompt`).
    Ambiguous(Vec<&'a AgentJob>),
    /// No job has it (a job since deleted), or the session is outside the
    /// window (still settling, or older than the look-back).
    None,
}

/// The jobs a settled, recent session can belong to, by its title.
pub fn match_session<'a>(s: &CronSession, jobs: &'a [AgentJob], now_ms: i64) -> Match<'a> {
    if s.ended_ms > now_ms - SETTLE_MS || s.ended_ms < now_ms - LOOKBACK_MS {
        return Match::None;
    }
    let hits: Vec<&AgentJob> = jobs
        .iter()
        .filter(|j| title_names_job(&s.title, &j.id, &j.name))
        .collect();
    match hits.len() {
        0 => Match::None,
        1 => Match::One(hits[0]),
        _ => Match::Ambiguous(hits),
    }
}

/// The listener a matched run lands with: the job must be listened to, and
/// the run must have begun after the thread started listening.
pub fn listener_for<'a>(
    job: &AgentJob,
    s: &CronSession,
    listeners: &'a [Listener],
) -> Option<&'a Listener> {
    listeners
        .iter()
        .find(|l| l.job_id == job.id && s.started_ms >= l.listening_since_ms - SLACK_MS)
}

/// A message row's text: a string, or the text parts of a content array.
fn content_text(row: &Value) -> String {
    match row.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// From a `/api/sessions/{id}/messages` page: the first user message (what
/// the scheduler sent, prompt included) and the last non-blank assistant
/// text (the run's report — what `post_llm_call` hands the push). Rows come
/// oldest first.
pub fn session_texts(page: &Value) -> (Option<String>, Option<String>) {
    let Some(rows) = page.get("data").and_then(Value::as_array) else {
        return (None, None);
    };
    let visible = |row: &&Value| row.get("display_kind").and_then(Value::as_str) != Some("hidden");
    let role = |row: &Value, want: &str| row.get("role").and_then(Value::as_str) == Some(want);
    let first_user = rows
        .iter()
        .filter(visible)
        .find(|row| role(row, "user"))
        .map(content_text);
    let last_answer = rows
        .iter()
        .filter(visible)
        .filter(|row| role(row, "assistant"))
        .map(content_text)
        .rfind(|text| !text.trim().is_empty());
    (first_user, last_answer)
}

/// The tiebreak for jobs that share a name: the job whose prompt the run was
/// sent. The scheduler sends its preamble and then the prompt verbatim, so a
/// run contains its own job's prompt; when one job's prompt is a prefix or
/// part of another's, the run of the longer one contains both, so the
/// longest contained prompt wins. Two contained prompts of the same length,
/// or none, is no answer.
pub fn by_prompt<'a>(hits: &[&'a AgentJob], first_user: &str) -> Option<&'a AgentJob> {
    let contained: Vec<(&'a AgentJob, usize)> = hits
        .iter()
        .copied()
        .filter_map(|j| {
            let p = j.prompt.as_deref()?.trim();
            (!p.is_empty() && first_user.contains(p)).then(|| (j, p.chars().count()))
        })
        .collect();
    let longest = contained.iter().map(|(_, n)| *n).max()?;
    let mut top = contained.into_iter().filter(|(_, n)| *n == longest);
    let (winner, _) = top.next()?;
    top.next().is_none().then_some(winner)
}

/// Is this run already in the thread? A row carrying the session id, or a
/// row of the same job that landed while the session ran (see the header).
pub(crate) async fn delivered(
    pg: &PgPool,
    l: &Listener,
    s: &CronSession,
) -> Result<bool, sqlx::Error> {
    let job_marker = format!("cron:{}:%", l.job_id);
    let hit: Option<i32> = sqlx::query_scalar(
        "select 1 from messages \
         where conversation_id = $1::uuid \
           and (metadata->>'cronSession' = $2 \
                or (metadata->>'marker' like $3 \
                    and created_at >= to_timestamp($4::double precision / 1000) \
                    and created_at <= to_timestamp($5::double precision / 1000))) \
         limit 1",
    )
    .bind(&l.conversation_id)
    .bind(&s.id)
    .bind(&job_marker)
    .bind((s.started_ms - SLACK_MS) as f64)
    .bind((s.ended_ms + SLACK_MS) as f64)
    .fetch_optional(pg)
    .await?;
    Ok(hit.is_some())
}

/// Does a row in any of these threads already carry the session id?
pub(crate) async fn landed_in_any(
    pg: &PgPool,
    threads: &[&str],
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    let threads: Vec<String> = threads.iter().map(|t| t.to_string()).collect();
    let hit: Option<i32> = sqlx::query_scalar(
        "select 1 from messages \
         where conversation_id = any($1::uuid[]) and metadata->>'cronSession' = $2 \
         limit 1",
    )
    .bind(&threads)
    .bind(session_id)
    .fetch_optional(pg)
    .await?;
    Ok(hit.is_some())
}

/// An agent-side read that must answer 2xx with json.
async fn read(model: &str, path: &str) -> Result<Value, String> {
    let answer = agent_request(model, Method::GET, path, None, READ_WINDOW).await?;
    if (200..300).contains(&answer.status) {
        Ok(answer.body)
    } else {
        Err(format!("{path} answered {}", answer.status))
    }
}

/// This agent's listeners, and every job it has: Hermes's own list, plus any
/// listened job it no longer lists (under the listener row's name), so a
/// stale row still claims its own runs and nothing else.
async fn agent_jobs(pg: &PgPool, model: &str) -> Result<(Vec<Listener>, Vec<AgentJob>), String> {
    type Row = (String, String, Option<String>, String, i64);
    let rows: Vec<Row> = sqlx::query_as(
        "select job_id, conversation_id::text, created_by::text, name, \
                (extract(epoch from created_at) * 1000)::bigint \
         from conversation_schedules where agent_model = $1",
    )
    .bind(model)
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let body = read(model, "/api/jobs?include_disabled=true").await?;
    let mut jobs: Vec<AgentJob> = body
        .get("jobs")
        .and_then(Value::as_array)
        .map(|jobs| {
            jobs.iter()
                .filter_map(|j| {
                    Some(AgentJob {
                        id: j.get("id")?.as_str()?.to_string(),
                        name: j
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        prompt: j.get("prompt").and_then(Value::as_str).map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut listeners = Vec::with_capacity(rows.len());
    for (job_id, conversation_id, created_by, name, since) in rows {
        if !jobs.iter().any(|j| j.id == job_id) {
            jobs.push(AgentJob {
                id: job_id.clone(),
                name,
                prompt: None,
            });
        }
        listeners.push(Listener {
            job_id,
            conversation_id,
            created_by,
            listening_since_ms: since,
        });
    }
    Ok((listeners, jobs))
}

/// What one pass did, for the scheduler's log line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PullReport {
    /// Reports landed that the push never delivered.
    pub landed: usize,
    /// Agents whose API server did not answer (stopped, or restarting).
    pub unreachable: Vec<String>,
    /// Sessions more than one same-named job could own, left alone.
    pub ambiguous: usize,
}

/// Pull one agent's lost reports into `report`.
async fn pull_agent(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    model: &str,
    report: &mut PullReport,
) -> Result<(), String> {
    let (listeners, jobs) = agent_jobs(pg, model).await?;
    if listeners.is_empty() {
        return Ok(());
    }
    let page = read(
        model,
        &format!("/api/sessions?source=cron&limit={SESSIONS_PAGE}"),
    )
    .await?;
    let now = now_ms();
    for session in ended_sessions(&page) {
        // The job, and the run's messages when the tiebreak already read them.
        let (job, mut messages) = match match_session(&session, &jobs, now) {
            Match::None => continue,
            Match::One(job) => (job, None),
            Match::Ambiguous(hits) => {
                // Only worth a read when a listened job is among them, and
                // the run is not already in one of their threads (the push
                // stamped it): the session id says so without the read, and
                // exactly — the window check below is per job, and two jobs
                // of one name can run side by side.
                let threads: Vec<&str> = hits
                    .iter()
                    .filter_map(|j| listener_for(j, &session, &listeners))
                    .map(|l| l.conversation_id.as_str())
                    .collect();
                if threads.is_empty()
                    || landed_in_any(pg, &threads, &session.id)
                        .await
                        .map_err(|e| e.to_string())?
                {
                    continue;
                }
                let page = read(model, &messages_path(&session.id)).await?;
                let (first_user, _) = session_texts(&page);
                let Some(job) = by_prompt(&hits, first_user.as_deref().unwrap_or("")) else {
                    tracing::debug!(
                        "{LOG} {model}: session {} could belong to {} jobs named alike; leaving it",
                        session.id,
                        hits.len()
                    );
                    report.ambiguous += 1;
                    continue;
                };
                (job, Some(page))
            }
        };
        // Another job's run (a fleet-wide one), or this thread's job before
        // the thread was listening: not the pull's to land.
        let Some(listener) = listener_for(job, &session, &listeners) else {
            continue;
        };
        if delivered(pg, listener, &session)
            .await
            .map_err(|e| e.to_string())?
        {
            continue;
        }
        let page = match messages.take() {
            Some(page) => page,
            None => read(model, &messages_path(&session.id)).await?,
        };
        let (_, answer) = session_texts(&page);
        // A run that ended with no answer (it died after a tool call, or
        // never reached the model) has no report to land; the push had
        // nothing to send either.
        let Some(text) = answer else {
            continue;
        };
        let landed = land(
            pg,
            redis.clone(),
            &JobListener {
                conversation_id: listener.conversation_id.clone(),
                created_by: listener.created_by.clone(),
            },
            CronReport {
                agent_model: model,
                marker: pull_marker(&job.id, &session.id),
                session_id: Some(&session.id),
                via: Via::Pull,
                text: &text,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        if landed.fresh {
            tracing::warn!(
                "{LOG} {model}: job {} session {} ended without its report reaching thread {} — \
                 pulled it in (message {})",
                job.id,
                session.id,
                listener.conversation_id,
                landed.message_id
            );
            report.landed += 1;
        }
    }
    Ok(())
}

/// The messages read for one session (ids are checked by `ended_sessions`
/// before they get here, so the path is always a plain segment).
fn messages_path(session_id: &str) -> String {
    format!("/api/sessions/{session_id}/messages")
}

/// One pass over every agent with a listening thread. An agent that does not
/// answer is skipped (it is stopped, or restarting; its sessions are still
/// there next pass); a database failure ends the pass.
pub async fn pull_pass(state: &AppState) -> Result<PullReport, String> {
    let models: Vec<String> = sqlx::query_scalar(
        "select distinct cs.agent_model from conversation_schedules cs \
         join agent_defs d on d.model = cs.agent_model and d.managed and d.enabled \
         order by 1",
    )
    .fetch_all(&state.pg)
    .await
    .map_err(|e| e.to_string())?;
    let mut report = PullReport::default();
    if models.is_empty() {
        return Ok(report);
    }
    let redis = state.redis().await.ok();
    for model in models {
        if let Err(e) = pull_agent(&state.pg, redis.clone(), &model, &mut report).await {
            tracing::debug!("{LOG} {model}: pull skipped: {e}");
            report.unreachable.push(model);
        }
    }
    Ok(report)
}

pub fn cron_pull_job_spec(state: AppState) -> talaria_scheduler::JobSpec {
    let state = Arc::new(state);
    talaria_scheduler::JobSpec {
        name: talaria_scheduler::JobName::CronOutputPull,
        every_ms: PULL_EVERY_MS,
        first_run_delay_ms: Some(3 * 60_000),
        max_run_ms: Some(4 * 60_000),
        // Leased, not per-instance: the input is each agent's session store
        // and the threads, which every instance reaches. The landing lock is
        // what makes a racing second instance a no-op anyway.
        per_instance: false,
        run: Arc::new(move || {
            let state = state.clone();
            Box::pin(async move {
                let report = pull_pass(&state).await?;
                // Quiet unless something landed: an ambiguous run is logged
                // per session at debug, and would otherwise repeat this line
                // every pass for the whole look-back.
                Ok((report.landed > 0).then(|| {
                    format!(
                        "landed {} cron report(s) the push lost{}",
                        report.landed,
                        if report.ambiguous > 0 {
                            format!(
                                " ({} session(s) two jobs could own, left alone)",
                                report.ambiguous
                            )
                        } else {
                            String::new()
                        }
                    )
                }))
            })
        }),
    }
}

pub fn register_cron_pull_job(state: AppState) {
    talaria_scheduler::register_job(cron_pull_job_spec(state));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(id: &str, name: &str, prompt: Option<&str>) -> AgentJob {
        AgentJob {
            id: id.into(),
            name: name.into(),
            prompt: prompt.map(String::from),
        }
    }

    fn listener(job: &str, since: i64) -> Listener {
        Listener {
            job_id: job.into(),
            conversation_id: format!("conv-{job}"),
            created_by: None,
            listening_since_ms: since,
        }
    }

    fn session(title: &str, started: i64, ended: i64) -> CronSession {
        CronSession {
            id: "local-aa".into(),
            title: title.into(),
            started_ms: started,
            ended_ms: ended,
        }
    }

    #[test]
    fn the_title_base_is_hermes_own_fold() {
        assert_eq!(title_base("Morning digest"), "Morning digest");
        assert_eq!(title_base("  Morning \n\t digest  "), "Morning digest");
        // Sixty characters, then trimmed: a cut that lands after a space
        // does not leave it.
        let long = format!("{} tail", "x".repeat(59));
        assert_eq!(title_base(&long), "x".repeat(59));
        assert_eq!(title_base(&"é".repeat(70)).chars().count(), 60);
    }

    #[test]
    fn a_run_title_names_its_job() {
        // The title a v0.22.4 run carried, read back from a live agent.
        assert!(title_names_job(
            "Morning digest · Oct 28 00:15",
            "e4db6afde70e",
            "Morning digest"
        ));
        assert!(title_names_job(
            "Morning digest · Oct 28 00:15",
            "e4db6afde70e",
            "  Morning   digest "
        ));
        assert!(!title_names_job(
            "Evening digest · Oct 28 00:15",
            "e4db6afde70e",
            "Morning digest"
        ));
        // Hermes's fallbacks name the job id itself.
        assert!(title_names_job(
            "cron e4db6afde70e",
            "e4db6afde70e",
            "Morning digest"
        ));
        assert!(title_names_job(
            "cron e4db6afde70e 9b8a7f",
            "e4db6afde70e",
            "Morning digest"
        ));
        assert!(!title_names_job(
            "cron e4db6afde70f",
            "e4db6afde70e",
            "Morning digest"
        ));
        // A name with the separator in it still matches on the last one.
        assert!(title_names_job(
            "A · B · Oct 28 00:15",
            "e4db6afde70e",
            "A · B"
        ));
        // An empty name never matches an empty title.
        assert!(!title_names_job("", "e4db6afde70e", ""));
    }

    // The listing a live v0.22.4 agent answered (`GET /api/sessions?source=cron`),
    // trimmed to the fields the pull reads, plus the rows it must refuse.
    #[test]
    fn sessions_read_as_epoch_seconds_and_unended_ones_are_left() {
        let page = json!({"object": "list", "limit": 100, "offset": 0, "has_more": false, "data": [
            {"id": "local-1f5952520d293f4e0f01789aa25a395b41e8f061f32af23a93058f77be710f57",
             "source": "cron", "user_id": "cron-owner", "model": "mock-model",
             "title": "Morning digest · Oct 28 00:17", "started_at": 1793146636.258725,
             "ended_at": 1793146637.381781, "end_reason": "cron_complete", "message_count": 2},
            {"id": "local-bb", "title": "Morning digest · Oct 28 00:20", "started_at": 1793146800.0, "ended_at": null},
            {"id": "../etc", "title": "Morning digest", "started_at": 1.0, "ended_at": 2.0},
            {"title": "no id", "started_at": 1.0, "ended_at": 2.0},
            {"id": "local-cc", "title": "Morning digest", "started_at": "1793146900", "ended_at": "1793146960.5"},
        ]});
        let s = ended_sessions(&page);
        assert_eq!(s.len(), 2);
        assert_eq!(
            s[0].id,
            "local-1f5952520d293f4e0f01789aa25a395b41e8f061f32af23a93058f77be710f57"
        );
        assert_eq!(s[0].started_ms, 1_793_146_636_258);
        assert_eq!(s[0].ended_ms, 1_793_146_637_381);
        assert_eq!(s[1].id, "local-cc");
        assert_eq!(s[1].ended_ms, 1_793_146_960_500);
        assert!(ended_sessions(&json!({"error": "nope"})).is_empty());
    }

    #[test]
    fn a_session_matches_only_inside_its_window() {
        let now = 1_793_150_000_000;
        let jobs = vec![job("e4db6afde70e", "Morning digest", None)];
        let title = "Morning digest · Oct 28 00:17";
        // Settled and recent: matched.
        assert_eq!(
            match_session(&session(title, now - 600_000, now - 540_000), &jobs, now),
            Match::One(&jobs[0])
        );
        // Ended a minute ago: a push may still be on its way.
        assert_eq!(
            match_session(&session(title, now - 120_000, now - 60_000), &jobs, now),
            Match::None
        );
        // Older than the look-back.
        assert_eq!(
            match_session(
                &session(
                    title,
                    now - LOOKBACK_MS - 120_000,
                    now - LOOKBACK_MS - 60_000
                ),
                &jobs,
                now
            ),
            Match::None
        );
    }

    #[test]
    fn a_run_lands_only_for_a_listener_that_was_listening() {
        let now = 1_793_150_000_000;
        let j = job("e4db6afde70e", "Morning digest", None);
        let listeners = vec![listener("e4db6afde70e", now - 3_600_000)];
        let title = "Morning digest · Oct 28 00:17";
        assert!(
            listener_for(
                &j,
                &session(title, now - 600_000, now - 540_000),
                &listeners
            )
            .is_some()
        );
        // Began before the thread listened.
        assert!(
            listener_for(
                &j,
                &session(title, now - 7_200_000, now - 7_140_000),
                &listeners
            )
            .is_none()
        );
        // A job nobody listens to.
        let other = job("aaaaaaaaaaaa", "Morning digest", None);
        assert!(
            listener_for(
                &other,
                &session(title, now - 600_000, now - 540_000),
                &listeners
            )
            .is_none()
        );
    }

    #[test]
    fn jobs_with_one_name_are_told_apart_by_the_prompt() {
        let now = 1_793_150_000_000;
        // A thread's job, a fleet-wide job of the same name nobody listens
        // to, and an unrelated one.
        let jobs = vec![
            job("aaaaaaaaaaaa", "Digest", Some("Summarise the inbox.")),
            job("bbbbbbbbbbbb", "Digest", Some("Summarise the board.")),
            job("cccccccccccc", "Other", Some("Something else.")),
        ];
        let s = session("Digest · Oct 28 00:15", now - 600_000, now - 540_000);
        let Match::Ambiguous(hits) = match_session(&s, &jobs, now) else {
            panic!("two jobs share the name");
        };
        assert_eq!(hits.len(), 2);
        // What the scheduler sent: its preamble, a blank line, the prompt.
        let sent =
            "[IMPORTANT: You are running as a scheduled cron job. …]\n\nSummarise the board.";
        assert_eq!(
            by_prompt(&hits, sent).map(|j| j.id.as_str()),
            Some("bbbbbbbbbbbb")
        );
        assert_eq!(by_prompt(&hits, "something else entirely"), None);
        // One prompt inside another: the run of the longer one names both,
        // and the longer one is its job.
        let nested = [
            job("aaaaaaaaaaaa", "Digest", Some("Summarise.")),
            job("bbbbbbbbbbbb", "Digest", Some("Summarise. Then post it.")),
        ];
        let refs: Vec<&AgentJob> = nested.iter().collect();
        assert_eq!(
            by_prompt(&refs, "[preamble]\n\nSummarise. Then post it.").map(|j| j.id.as_str()),
            Some("bbbbbbbbbbbb")
        );
        assert_eq!(
            by_prompt(&refs, "[preamble]\n\nSummarise.").map(|j| j.id.as_str()),
            Some("aaaaaaaaaaaa")
        );
        // Two distinct prompts of one length, both in the text: no answer.
        let twins = [
            job("aaaaaaaaaaaa", "Digest", Some("Do A.")),
            job("bbbbbbbbbbbb", "Digest", Some("Do B.")),
        ];
        let refs: Vec<&AgentJob> = twins.iter().collect();
        assert_eq!(by_prompt(&refs, "Do A. Do B."), None);
        // A job with no prompt known (Hermes no longer lists it) never wins.
        let gone = [
            job("aaaaaaaaaaaa", "Digest", None),
            job("bbbbbbbbbbbb", "Digest", None),
        ];
        let refs: Vec<&AgentJob> = gone.iter().collect();
        assert_eq!(by_prompt(&refs, "anything"), None);
    }

    // The messages page a live v0.22.4 agent answered for that run
    // (`GET /api/sessions/{id}/messages`), the preamble cut short.
    #[test]
    fn the_report_is_the_last_assistant_text() {
        let page = json!({"object": "list",
        "session_id": "local-9293f53853ffe78bf4b664009d75fcad3fa5e24bfbd343a63bb952ce4a64038c",
        "pagination": {"limit": 500, "offset": 0, "order": "latest", "returned": 2},
        "data": [
            {"role": "user", "content": "[IMPORTANT: You are running as a scheduled cron job. …]\n\nSummarise the overnight tickets for the fu-crons proof.", "display_kind": null},
            {"role": "assistant", "content": "REPORT 2: o schedule another job.] Summarise the overnight tickets for the fu-crons proof.", "display_kind": null, "finish_reason": "stop"},
        ]});
        let (first, last) = session_texts(&page);
        assert!(
            first
                .unwrap()
                .ends_with("Summarise the overnight tickets for the fu-crons proof.")
        );
        assert_eq!(
            last.as_deref(),
            Some(
                "REPORT 2: o schedule another job.] Summarise the overnight tickets for the fu-crons proof."
            )
        );
        // A tool loop: the report is the last assistant text, not a tool-call
        // row or a hidden compaction carrier.
        let looped = json!({"data": [
            {"role": "user", "content": "[cron] Summarise the inbox."},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]},
            {"role": "tool", "content": "3 unread"},
            {"role": "assistant", "content": "You have 3 unread threads."},
            {"role": "assistant", "content": "", "display_kind": "hidden"},
        ]});
        assert_eq!(
            session_texts(&looped).1.as_deref(),
            Some("You have 3 unread threads.")
        );
        // Text parts read as text.
        let parts = json!({"data": [
            {"role": "user", "content": [{"type": "text", "text": "go"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "done "}, {"type": "text", "text": "now"}]},
        ]});
        assert_eq!(session_texts(&parts).1.as_deref(), Some("done now"));
        // A run that never answered.
        let none =
            json!({"data": [{"role": "user", "content": "go"}, {"role": "tool", "content": "x"}]});
        assert_eq!(session_texts(&none).1, None);
    }
}
