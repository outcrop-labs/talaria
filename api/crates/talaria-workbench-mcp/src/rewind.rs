use serde_json::{Map, Value, json};
use sqlx::{FromRow, PgPool};

use talaria_agent_auth::AgentSubject;
use talaria_git_hosts::GitHost;
use talaria_runs_store::{LINEAGE_COLS, RunLineage};
use talaria_tasks::{ResolvedTaskId, resolve_task_id};

use crate::teardown::{Teardown, job_workdir, spawn_teardown};
use crate::{
    AgentCtx, CallOutcome, HarnessAnswer, JOB_COLS, JobRow, WorkbenchActor, WorkbenchDeps,
    WorkbenchJob, devenv, harness_answer, hosts, live_session, log_ticket, sync_agent_budget,
    thrown, ticket_arg,
};

pub(crate) struct RewindAttempt {
    run_id: String,
    lineage: RunLineage,
}

pub(crate) enum RewindStart {
    Answered(CallOutcome),
    /// The attempt's job has its PR open, so this is an ordinary revise round.
    OrdinaryRevise,
}

pub(crate) async fn attempt_of_call(
    pg: &PgPool,
    model: &str,
    args: &Map<String, Value>,
) -> Option<RewindAttempt> {
    let raw = args.get("taskId").and_then(Value::as_str)?.trim();
    let Ok(ResolvedTaskId::One(task_id)) = resolve_task_id(pg, raw).await else {
        return None;
    };
    let run_id = live_session(pg, model, &task_id).await?;
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "select {LINEAGE_COLS} from runs \
         where id = $1::uuid and rewound_from_run_id is not null"
    )))
    .bind(&run_id)
    .fetch_optional(pg)
    .await
    .ok()??;
    let lineage = RunLineage::from_row(&row).ok()?;
    Some(RewindAttempt { run_id, lineage })
}

pub(crate) fn superseded_refusal(job_id: &str, current: Option<&str>) -> String {
    match current {
        Some(current) => format!(
            "job {job_id} was superseded by a rewind and takes no more changes — this work continues in job {current}"
        ),
        None => format!("job {job_id} was superseded by a rewind and takes no more changes"),
    }
}

/// Retire every job of the lineage the attempt replaced, along with the job its
/// checkpoint names, which a revise round may have moved to a run outside the
/// lineage. Jobs an earlier rewind retired are repointed, so `superseded_by`
/// always names the current job. Repeats safely: a job already retired is
/// stopped once, and `current_job` is never touched.
pub async fn supersede_jobs(
    pg: &PgPool,
    department: &str,
    lineage: &RunLineage,
    current_job: &str,
) -> Result<(), sqlx::Error> {
    let retired: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "with lineage as ( \
           select id from runs where id = $1::uuid or lineage_id = $1::uuid \
         ), prior as ( \
           select j.id, j.status from workbench_jobs j \
           where j.id <> $4::uuid \
             and j.status in ('started', 'queued', 'awaiting_approval', 'pr_open', 'superseded') \
             and (j.run_id in (select id from lineage) \
                  or j.id in (select c.job_id from run_checkpoints c \
                              where c.run_id = $2::uuid and c.seq = $3)) \
           for update of j \
         ) \
         update workbench_jobs j set \
           status = 'superseded', superseded_by = $4::uuid, queued_reason = null, \
           updated_at = case when prior.status = 'superseded' then j.updated_at else now() end \
         from prior where j.id = prior.id \
         returning j.id::text, prior.status, j.task_id::text",
    )
    .bind(&lineage.lineage_id)
    .bind(&lineage.rewound_from_run_id)
    .bind(lineage.rewound_from_seq)
    .bind(current_job)
    .fetch_all(pg)
    .await?;
    for (job_id, was, task_id) in retired {
        if was == "superseded" {
            continue;
        }
        if was == "queued"
            && let Some(task_id) = &task_id
        {
            talaria_work_wait::clear_waiting(pg, task_id).await;
        }
        spawn_teardown(pg.clone(), department.into(), job_id, Teardown::Stop);
    }
    Ok(())
}

pub(crate) async fn start_job(
    deps: &WorkbenchDeps,
    subject: &AgentSubject,
    agent: &AgentCtx,
    args: &Map<String, Value>,
    attempt: &RewindAttempt,
) -> RewindStart {
    let pg = &deps.pg;
    let task_id = match ticket_arg(pg, subject, args).await {
        Ok(id) => id,
        Err(error) => return RewindStart::Answered(CallOutcome::Fail(error)),
    };
    let row: Result<Option<JobRow>, sqlx::Error> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {JOB_COLS} from workbench_jobs \
         where run_id = $1::uuid and agent_id = $2::uuid \
         order by created_at desc limit 1"
    )))
    .bind(&attempt.run_id)
    .bind(&agent.id)
    .fetch_optional(pg)
    .await;
    let job = match row {
        Ok(Some(row)) => WorkbenchJob::of(row),
        Ok(None) => {
            return RewindStart::Answered(CallOutcome::Fail(format!(
                "this rewind attempt (run {}) has no job allocated — a rewind attempt works only the job its rewind restored",
                attempt.run_id
            )));
        }
        Err(e) => return RewindStart::Answered(thrown(format!("job read: {e}"))),
    };
    match job.status.as_str() {
        "started" => {}
        "pr_open" => return RewindStart::OrdinaryRevise,
        other => {
            return RewindStart::Answered(CallOutcome::Fail(format!(
                "this rewind attempt's job {} is {other} — a rewind attempt works only the job its rewind restored",
                job.id
            )));
        }
    }
    if let Err(e) = supersede_jobs(pg, &agent.department, &attempt.lineage, &job.id).await {
        return RewindStart::Answered(thrown(format!("job update: {e}")));
    }
    let host = match hosts::host(pg, &deps.sb, &job.host_id).await {
        Ok(host) => host,
        Err(e) => return RewindStart::Answered(CallOutcome::Fail(e)),
    };
    let base = match hosts::effective_base(pg, &host, &job.repo).await {
        Ok(base) => base,
        Err(e) => return RewindStart::Answered(thrown(e)),
    };
    let HarnessAnswer {
        harnesses,
        session_dir,
        model,
        models,
        coding,
    } = match harness_answer(pg, &job.id, &agent.id, task_id.as_deref()).await {
        Ok(answer) => answer,
        Err(e) => return RewindStart::Answered(thrown(e)),
    };
    let Some(workdir) = job_workdir(&job.id) else {
        return RewindStart::Answered(thrown(format!("not a job id: {:?}", job.id)));
    };
    let runtime = devenv::ensure_harness_runtime(pg, &agent.department).await;
    let attempt_no = attempt.lineage.attempt_no;
    let branch = &job.branch;
    log_ticket(
        pg,
        task_id.as_deref(),
        &WorkbenchActor::Agent(subject.clone()),
        &format!(
            "workbench job resumed for rewind attempt {attempt_no}: {} @ {branch}",
            job.repo
        ),
    )
    .await;
    let mut value = Map::new();
    value.insert("jobId".into(), json!(job.id));
    value.insert("host".into(), json!(job.host_id));
    value.insert("repo".into(), json!(job.repo));
    value.insert("branch".into(), json!(branch));
    value.insert("base".into(), json!(base));
    value.insert("resumed".into(), json!(true));
    value.insert("attempt".into(), json!(attempt_no));
    value.insert("status".into(), json!(job.status));
    value.insert("workdir".into(), json!(workdir));
    value.insert("sessionDir".into(), json!(session_dir));
    value.insert("cloneUrl".into(), json!(host.clone_url(&job.repo)));
    if let Err(why) = &runtime {
        value.insert(
            "harnessRuntime".into(),
            json!({ "ok": false, "error": why, "next": "call doctor; if bun still cannot be provisioned, report_problem — do not hand-code the ticket" }),
        );
    }
    value.insert("effort".into(), json!(job.effort));
    value.insert(
        "model".into(),
        model.map(Value::String).unwrap_or(Value::Null),
    );
    value.insert("models".into(), models);
    if let Some(plan) = coding {
        value.insert("codingAccount".into(), plan);
    }
    value.insert("harnesses".into(), Value::Array(harnesses));
    value.insert(
        "rules".into(),
        json!(format!(
            "This is rewind attempt {attempt_no}. {workdir} already holds the files as they stood at the step you were rewound to, on {branch}: do not clone over it. Any other job id, branch or workdir earlier in this conversation is retired, and calls naming a retired job are refused. Work ONLY here, and never touch {base}. Call prepare_env with this jobId first (it writes this project's env to {workdir}/{env_file}). Oh My Pi starts fresh, so give it the context it needs. Push to {branch}, then finish_job.",
            env_file = devenv::JOB_ENV_FILE,
        )),
    );
    sync_agent_budget(pg, &agent.department, &agent.id).await;
    RewindStart::Answered(CallOutcome::Ok(Value::Object(value)))
}
