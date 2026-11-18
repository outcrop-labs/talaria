use serde_json::{Map, Value, json};
use sqlx::{FromRow, PgPool};

use talaria_agent_auth::AgentSubject;
use talaria_runs_store::{LINEAGE_COLS, RunLineage};
use talaria_tasks::{ResolvedTaskId, resolve_task_id};

use crate::teardown::{Teardown, job_workdir, spawn_teardown};
use crate::{
    AgentCtx, CallOutcome, HarnessAnswer, JOB_COLS, JobRow, START_PROVISION_WAIT_MS,
    WorkbenchActor, WorkbenchDeps, WorkbenchJob, devenv, harness_answer, hosts, live_session,
    log_ticket, provision, sync_agent_budget, thrown, ticket_arg,
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

/// Retire every other job this agent holds on the ticket. The key is the ticket
/// and not the lineage because a revise round moves a job to a run outside its
/// lineage. Jobs an earlier rewind retired are repointed, so `superseded_by`
/// always names the current job. Repeats safely: a job already retired is
/// stopped once, and `current_job` is never touched.
pub async fn supersede_jobs(
    pg: &PgPool,
    department: &str,
    current_job: &str,
) -> Result<(), sqlx::Error> {
    let retired: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "with prior as ( \
           select j.id, j.status from workbench_jobs j \
           join workbench_jobs current_job on current_job.id = $1::uuid \
           where j.id <> current_job.id \
             and j.task_id = current_job.task_id and j.agent_id = current_job.agent_id \
             and j.status in ('started', 'queued', 'awaiting_approval', 'pr_open', 'superseded') \
           for update of j \
         ) \
         update workbench_jobs j set \
           status = 'superseded', superseded_by = $1::uuid, queued_reason = null, \
           updated_at = case when prior.status = 'superseded' then j.updated_at else now() end \
         from prior where j.id = prior.id \
         returning j.id::text, prior.status, j.task_id::text",
    )
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
    if let Err(e) = supersede_jobs(pg, &agent.department, &job.id).await {
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
    let pj = match provision::ProvisionJob::load(pg, &job.id).await {
        Ok(Some(pj)) => pj,
        Ok(None) => return RewindStart::Answered(CallOutcome::Fail("job is not started".into())),
        Err(e) => return RewindStart::Answered(thrown(e)),
    };
    if let Err(e) = provision::kick(deps, &pj).await {
        return RewindStart::Answered(thrown(e));
    }
    let provision_state = provision::wait(pg, &job.id, START_PROVISION_WAIT_MS).await;
    let ready = provision::is_ready(provision_state.as_ref());
    let HarnessAnswer {
        harnesses,
        session_dir,
        model,
        models,
        coding,
    } = match harness_answer(
        pg,
        &job.id,
        &agent.id,
        task_id.as_deref(),
        provision_state.as_ref(),
    )
    .await
    {
        Ok(answer) => answer,
        Err(e) => return RewindStart::Answered(thrown(e)),
    };
    let Some(workdir) = job_workdir(&job.id) else {
        return RewindStart::Answered(thrown(format!("not a job id: {:?}", job.id)));
    };
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
    if let Some(state) = &provision_state {
        value.insert("provision".into(), provision::wire(state));
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
    let rules = if ready {
        value.insert("harnesses".into(), Value::Array(harnesses));
        format!(
            "This is rewind attempt {attempt_no}. {workdir} already holds the files as they stood at the step you were rewound to, on {branch}: Talaria kept that checkout, wrote this project's env (if it has one) to {workdir}/{env_file}, and checked the toolchains and Oh My Pi for this job. Do NOT clone, install, upgrade or wrap anything. Any other job id, branch or workdir earlier in this conversation is retired, and calls naming a retired job are refused. Work ONLY here, and never touch {base}. Oh My Pi starts fresh, so give it the context it needs. Push to {branch}, then finish_job.",
            env_file = devenv::JOB_ENV_FILE,
        )
    } else {
        provision_state
            .as_ref()
            .map(provision::next_step)
            .unwrap_or_else(|| provision::next_step(&Value::Null))
    };
    value.insert("rules".into(), json!(rules));
    sync_agent_budget(pg, &agent.department, &agent.id).await;
    RewindStart::Answered(CallOutcome::Ok(Value::Object(value)))
}
