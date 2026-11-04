// Every edge a rewind touches, behind one trait so the stage machine can be
// driven with no database or container. `Real` is the one implementation the
// boot wiring arms.

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde_json::Value;
use sqlx::{FromRow, PgPool};

use talaria_agent_auth::{iso_to_epoch_ms, now_ms};
use talaria_fleet_docker::rewind as container;
use talaria_gateway::fleet_chat;
use talaria_runs_define::is_terminal;
use talaria_runs_run::RunDeps;
use talaria_runs_store::{
    AttemptSeed, CancelOutcome, LINEAGE_COLS, PgRunStore, RunCheckpoint, RunLineage, RunStore,
    insert_attempt, run_checkpoints,
};
use talaria_runs_work_session::{
    AgentStop, INTERRUPTED_TURN_SETTLE_MS, WORK_SESSION_KIND, WorkSessionInput, seeded_send,
    session_run_id, stop_agent_turn,
};
use talaria_state::AppState;
use talaria_statuses::status_meta;
use talaria_tasks_types::{TaskActor, TaskDeps, TaskPatch};
use talaria_work_dispatch::MAX_SESSIONS_PER_TICKET_AGENT;
use talaria_work_dispatch::holds::{lock_pair, place_hold};
use talaria_workbench_mcp::rewind::supersede_jobs;
use talaria_workbench_mcp::teardown::job_workdir;

use crate::{RewindInput, RewindPlan, Stage, Validated, attempt_input, settled};

pub use talaria_fleet_docker::rewind::{Checkout, Cut};
pub use talaria_gateway::fleet_chat::Fork;

const LOG: &str = "[run-rewind]";

/// The whole quiet stage, the plan's budget for a live turn to end.
const QUIET_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);
const QUIET_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// Where the ticket sits, as the rewind's rule reads the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketColumn {
    Working,
    Review,
    Done,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ticket {
    pub ticket_ref: Option<String>,
    pub title: String,
    pub status: String,
    pub column: TicketColumn,
    /// The board's working column, for a ticket coming back from review.
    pub active_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct SourceJob {
    pub id: String,
    pub repo: String,
    pub host_id: String,
    pub agent_id: String,
    pub effort: String,
    pub plan: String,
}

/// The source as `validate` reads it. `None` where a read found nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub run_id: String,
    pub seq: i64,
    pub input: WorkSessionInput,
    pub lineage: RunLineage,
    pub row: Option<RunCheckpoint>,
    pub ticket: Option<Ticket>,
    pub job: Option<SourceJob>,
}

/// What `reserve` fixed under the pair's lock.
#[derive(Debug, Clone, PartialEq)]
pub struct Reservation {
    pub attempt_run_id: String,
    pub generation: u32,
    pub attempt_no: i32,
    pub job_id: String,
    pub workdir: String,
    pub root: String,
    pub branch: String,
    pub origin_url: String,
    pub department: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quiet {
    Confirmed,
    Unconfirmed(String),
}

pub trait RewindWorld: Send + Sync {
    fn source<'a>(&'a self, input: &'a RewindInput) -> BoxFuture<'a, Result<Source, String>>;
    /// The agent answers, the cut's symbols are there, and the commit is in
    /// the source job's checkout. Answers that checkout's root.
    fn reach<'a>(
        &'a self,
        agent_model: &'a str,
        source_job_id: &'a str,
        commit: &'a str,
    ) -> BoxFuture<'a, Result<String, String>>;
    /// Place the pair's hold and fix the attempt's generation, number, job and
    /// branch, under the pair's lock.
    fn reserve<'a>(
        &'a self,
        rewind_id: &'a str,
        input: &'a RewindInput,
        source: &'a Source,
        validated: &'a Validated,
        source_root: &'a str,
    ) -> BoxFuture<'a, Result<Reservation, String>>;
    fn cancel_source<'a>(&'a self, run_id: &'a str) -> BoxFuture<'a, Result<(), String>>;
    fn quiet<'a>(&'a self, run_id: &'a str) -> BoxFuture<'a, Quiet>;
    fn fork<'a>(
        &'a self,
        agent_model: &'a str,
        source_id: &'a str,
        attempt_id: &'a str,
    ) -> BoxFuture<'a, Result<Fork, String>>;
    fn cut<'a>(
        &'a self,
        agent_model: &'a str,
        session_id: &'a str,
        tool_call_id: &'a str,
    ) -> BoxFuture<'a, Result<Cut, String>>;
    /// Answers the restored checkout's tree id.
    fn restore<'a>(
        &'a self,
        agent_model: &'a str,
        plan: &'a RewindPlan,
    ) -> BoxFuture<'a, Result<String, String>>;
    /// The tool call id of the session's last row, when that row is a tool
    /// result.
    fn session_tail<'a>(
        &'a self,
        agent_model: &'a str,
        session_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>>;
    fn checkout<'a>(
        &'a self,
        agent_model: &'a str,
        root: &'a str,
    ) -> BoxFuture<'a, Result<Checkout, String>>;
    /// Move the ticket if the plan says so, insert the attempt and its job
    /// with the hold lifted, retire the source's jobs, and drive the attempt.
    fn start<'a>(
        &'a self,
        rewind_id: &'a str,
        input: &'a RewindInput,
        plan: &'a RewindPlan,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Take back what a failed rewind placed: the hold, the checkout, the
    /// forked session's rows. Best effort; the run is already failing.
    fn fail<'a>(
        &'a self,
        rewind_id: &'a str,
        input: &'a RewindInput,
        plan: Option<&'a RewindPlan>,
        stage: Option<Stage>,
    ) -> BoxFuture<'a, ()>;
}

// ── The real world ───────────────────────────────────────────────────────────

pub struct Real {
    pub state: AppState,
}

impl Real {
    fn pg(&self) -> &PgPool {
        &self.state.pg
    }

    async fn run_deps(&self) -> Result<RunDeps, String> {
        let redis = self
            .state
            .redis()
            .await
            .map_err(|e| format!("redis unavailable: {e}"))?;
        let rt = talaria_realtime::RealtimeDeps::publish_only(Some(redis.clone()));
        Ok(talaria_runs_decide::assembly::real_run_deps(
            self.pg().clone(),
            redis,
            rt,
        ))
    }
}

/// The attempt's root: the source root's place under the source workdir,
/// under the attempt's workdir.
pub fn attempt_root(source_workdir: &str, source_root: &str, workdir: &str) -> String {
    match source_root.strip_prefix(source_workdir) {
        Some(rest) => format!("{workdir}{rest}"),
        None => workdir.to_string(),
    }
}

impl RewindWorld for Real {
    fn source<'a>(&'a self, input: &'a RewindInput) -> BoxFuture<'a, Result<Source, String>> {
        async move {
            let pg = self.pg();
            let store = PgRunStore::new(pg.clone());
            let run = store
                .get(&input.source_run_id)
                .await
                .map_err(|e| format!("run read: {e}"))?
                .filter(|run| run.kind == WORK_SESSION_KIND)
                .ok_or_else(|| format!("run {} is not a work session", input.source_run_id))?;
            let session: WorkSessionInput = serde_json::from_value(run.input.clone())
                .map_err(|_| format!("run {} has no readable input", run.id))?;
            let lineage_row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "select {LINEAGE_COLS} from runs where id = $1::uuid"
            )))
            .bind(&run.id)
            .fetch_one(pg)
            .await
            .map_err(|e| format!("lineage read: {e}"))?;
            let lineage = RunLineage::from_row(&lineage_row).map_err(|e| format!("lineage read: {e}"))?;
            let row = run_checkpoints(pg, &run.id)
                .await
                .map_err(|e| format!("ledger read: {e}"))?
                .into_iter()
                .find(|row| row.seq == input.seq);
            let job = match row.as_ref().and_then(|r| r.job_id.clone()) {
                Some(job_id) => sqlx::query_as::<_, SourceJob>(
                    "select id::text as id, repo, host_id, agent_id::text as agent_id, effort, plan \
                     from workbench_jobs where id = $1::uuid",
                )
                .bind(&job_id)
                .fetch_optional(pg)
                .await
                .map_err(|e| format!("job read: {e}"))?,
                None => None,
            };
            let ticket = match talaria_tasks::get_task(pg, &session.task_id)
                .await
                .map_err(|e| format!("ticket read: {e}"))?
            {
                Some(task) => {
                    let meta = status_meta(pg, &task.board_id)
                        .await
                        .map_err(|e| format!("board read: {e}"))?;
                    let column = if meta.terminal(&task.status) {
                        TicketColumn::Done
                    } else if meta.review_keys.contains(&task.status) {
                        TicketColumn::Review
                    } else if meta.working_keys.contains(&task.status) {
                        TicketColumn::Working
                    } else {
                        TicketColumn::Other
                    };
                    Some(Ticket {
                        ticket_ref: task.ticket_ref.clone(),
                        title: task.title.clone(),
                        status: task.status.clone(),
                        column,
                        active_key: meta.active_key.clone().or(meta.assigned_key.clone()),
                    })
                }
                None => None,
            };
            Ok(Source {
                run_id: run.id,
                seq: input.seq,
                input: session,
                lineage,
                row,
                ticket,
                job,
            })
        }
        .boxed()
    }

    fn reach<'a>(
        &'a self,
        agent_model: &'a str,
        source_job_id: &'a str,
        commit: &'a str,
    ) -> BoxFuture<'a, Result<String, String>> {
        async move {
            let pg = self.pg();
            let workdir = job_workdir(source_job_id)
                .ok_or_else(|| format!("not a job id: {source_job_id:?}"))?;
            container::probe(pg, agent_model)
                .await
                .map_err(|e| format!("the agent cannot rewind: {e}"))?;
            let root = container::git_root(pg, agent_model, &workdir).await?;
            if !container::commit_exists(pg, agent_model, &root, commit).await? {
                return Err(format!("snapshot commit {commit} is not in {root}"));
            }
            Ok(root)
        }
        .boxed()
    }

    fn reserve<'a>(
        &'a self,
        rewind_id: &'a str,
        _input: &'a RewindInput,
        source: &'a Source,
        validated: &'a Validated,
        source_root: &'a str,
    ) -> BoxFuture<'a, Result<Reservation, String>> {
        async move {
            let pg = self.pg();
            let task_id = &source.input.task_id;
            let agent_model = &source.input.agent_model;
            let department = container::department_of(pg, agent_model).await?;
            let sb = self
                .state
                .secretbox()
                .await
                .map_err(|e| format!("the secretbox did not load: {e}"))?;
            let host = talaria_git_hosts::host(pg, &sb, &validated.job.host_id).await?;
            let origin_url = talaria_git_hosts::GitHost::clone_url(&host, &validated.job.repo);
            let store = PgRunStore::new(pg.clone());
            let mut lock = lock_pair(pg, task_id, agent_model)
                .await
                .map_err(|e| format!("pair lock: {e}"))?;
            let outcome = async {
                let placed = place_hold(lock.conn(), task_id, agent_model, rewind_id)
                    .await
                    .map_err(|e| format!("hold write: {e}"))?;
                if !placed {
                    return Err("another rewind is already in flight on this ticket and agent".to_string());
                }
                let mut free = None;
                for generation in 0..MAX_SESSIONS_PER_TICKET_AGENT {
                    let id = session_run_id(task_id, agent_model, generation);
                    match store.get(&id).await.map_err(|e| format!("run read: {e}"))? {
                        Some(row) if row.id == source.run_id || is_terminal(row.state) => continue,
                        Some(row) => {
                            return Err(format!(
                                "session {} is still live on this ticket and agent — stop it before rewinding",
                                row.id
                            ));
                        }
                        None => {}
                    }
                    // A session a failed rewind left under this id would be
                    // taken for the fork of this one.
                    if fleet_chat::session_exists(agent_model, &id).await? {
                        continue;
                    }
                    free = Some((generation, id));
                    break;
                }
                let Some((generation, attempt_run_id)) = free else {
                    return Err(format!(
                        "this ticket and agent have had {MAX_SESSIONS_PER_TICKET_AGENT} sessions — no slot is free for an attempt"
                    ));
                };
                let attempt_no: i32 = sqlx::query_scalar(
                    "select coalesce(max(attempt_no), 0) + 1 from runs \
                     where coalesce(lineage_id, id) = $1::uuid",
                )
                .bind(&validated.lineage_id)
                .fetch_one(lock.conn())
                .await
                .map_err(|e| format!("attempt count: {e}"))?;
                let job_id = uuid::Uuid::new_v4().to_string();
                let workdir = job_workdir(&job_id).expect("a fresh uuid is a job id");
                let source_workdir = job_workdir(&validated.job.id).expect("validated job id");
                let branch = talaria_workbench_mcp::job_branch_name(
                    pg,
                    &validated.job.agent_id,
                    &talaria_git_hosts::RepoRef {
                        host_id: validated.job.host_id.clone(),
                        repo: validated.job.repo.clone(),
                    },
                    &validated.ticket_ref,
                    &validated.title,
                    Some(attempt_no),
                )
                .await;
                Ok(Reservation {
                    attempt_run_id,
                    generation,
                    attempt_no,
                    root: attempt_root(&source_workdir, source_root, &workdir),
                    job_id,
                    workdir,
                    branch,
                    origin_url,
                    department,
                })
            }
            .await;
            lock.release().await;
            outcome
        }
        .boxed()
    }

    fn cancel_source<'a>(&'a self, run_id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        async move {
            let deps = self.run_deps().await?;
            match talaria_runs_run::cancel_run(run_id, Some("rewound".into()), &deps)
                .await
                .map_err(|e| format!("cancel: {e}"))?
            {
                CancelOutcome::Cancelled { .. } | CancelOutcome::Terminal { .. } => Ok(()),
                CancelOutcome::Missing => Err(format!("run {run_id} is gone")),
            }
        }
        .boxed()
    }

    fn quiet<'a>(&'a self, run_id: &'a str) -> BoxFuture<'a, Quiet> {
        async move {
            // One unconfirmed answer is not final: a turn names itself after
            // the cancel, and the stop then lands on it. The row's id and the
            // agent's word are re-read until the budget runs out.
            let deadline = tokio::time::Instant::now() + QUIET_BUDGET;
            let store = PgRunStore::new(self.pg().clone());
            loop {
                let stop =
                    tokio::time::timeout_at(deadline, stop_agent_turn(self.pg(), run_id)).await;
                let why = match stop {
                    Ok(AgentStop::Ended(_) | AgentStop::NoTurn) => return Quiet::Confirmed,
                    Ok(AgentStop::Unconfirmed(why)) => why,
                    Err(_) => "no answer inside the stop window".to_string(),
                };
                let run = store.get(run_id).await.ok().flatten();
                let settled_now = run.is_some_and(|run| {
                    settled(
                        run.state,
                        run.finished_at.as_deref().and_then(iso_to_epoch_ms),
                        now_ms(),
                        INTERRUPTED_TURN_SETTLE_MS,
                    )
                });
                if settled_now {
                    return Quiet::Confirmed;
                }
                if tokio::time::Instant::now() + QUIET_POLL >= deadline {
                    return Quiet::Unconfirmed(why);
                }
                tokio::time::sleep(QUIET_POLL).await;
            }
        }
        .boxed()
    }

    fn fork<'a>(
        &'a self,
        agent_model: &'a str,
        source_id: &'a str,
        attempt_id: &'a str,
    ) -> BoxFuture<'a, Result<Fork, String>> {
        fleet_chat::fork_session(agent_model, source_id, attempt_id).boxed()
    }

    fn cut<'a>(
        &'a self,
        agent_model: &'a str,
        session_id: &'a str,
        tool_call_id: &'a str,
    ) -> BoxFuture<'a, Result<Cut, String>> {
        container::cut_session(self.pg(), agent_model, session_id, tool_call_id).boxed()
    }

    fn restore<'a>(
        &'a self,
        agent_model: &'a str,
        plan: &'a RewindPlan,
    ) -> BoxFuture<'a, Result<String, String>> {
        async move {
            container::restore_checkout(
                self.pg(),
                agent_model,
                &container::Restore {
                    source_root: &plan.source_root,
                    root: &plan.root,
                    commit: &plan.commit,
                    branch: &plan.branch,
                    origin_url: &plan.origin_url,
                },
            )
            .await
        }
        .boxed()
    }

    fn session_tail<'a>(
        &'a self,
        agent_model: &'a str,
        session_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        async move {
            let rows = fleet_chat::session_messages(agent_model, session_id)
                .await
                .ok_or_else(|| {
                    format!("the agent did not answer the rows of session {session_id}")
                })?;
            Ok(tail_tool_call(&rows))
        }
        .boxed()
    }

    fn checkout<'a>(
        &'a self,
        agent_model: &'a str,
        root: &'a str,
    ) -> BoxFuture<'a, Result<Checkout, String>> {
        container::read_checkout(self.pg(), agent_model, root).boxed()
    }

    fn start<'a>(
        &'a self,
        rewind_id: &'a str,
        input: &'a RewindInput,
        plan: &'a RewindPlan,
    ) -> BoxFuture<'a, Result<(), String>> {
        async move {
            let pg = self.pg();
            let deps = self.run_deps().await?;
            let source = deps
                .store
                .get(&input.source_run_id)
                .await
                .map_err(|e| format!("run read: {e}"))?
                .ok_or_else(|| format!("run {} is gone", input.source_run_id))?;
            let source_input: WorkSessionInput = serde_json::from_value(source.input.clone())
                .map_err(|_| format!("run {} has no readable input", source.id))?;
            let lock = lock_pair(pg, &input.task_id, &input.agent_model)
                .await
                .map_err(|e| format!("pair lock: {e}"))?;
            let outcome = async {
                if let Some(row) = deps
                    .store
                    .get(&plan.attempt_run_id)
                    .await
                    .map_err(|e| format!("run read: {e}"))?
                {
                    // This rewind already started it and died before saying so.
                    if row.input["rewind"]["jobId"].as_str() == Some(plan.job_id.as_str()) {
                        return Ok(());
                    }
                    return Err(format!(
                        "session {} was claimed while the rewind ran",
                        plan.attempt_run_id
                    ));
                }
                if let Some(status) = &plan.move_to {
                    let redis = self.state.redis().await.ok();
                    let task_deps = TaskDeps::from_route(pg.clone(), redis);
                    talaria_tasks::update_task(
                        &task_deps,
                        &input.task_id,
                        TaskPatch {
                            status: Some(status.clone()),
                            ..TaskPatch::default()
                        },
                        &TaskActor::human(input.requested_by.clone()),
                    )
                    .await
                    .map_err(|e| format!("could not move the ticket back to \"{status}\": {}", e.message()))?;
                }
                let mut tx = pg.begin().await.map_err(|e| format!("begin: {e}"))?;
                insert_attempt(
                    &mut tx,
                    &AttemptSeed {
                        id: plan.attempt_run_id.clone(),
                        subject_id: input.task_id.clone(),
                        input: serde_json::to_value(attempt_input(&source.id, &source_input, plan))
                            .map_err(|e| e.to_string())?,
                        phase: format!("rewound to step {} of attempt {}", input.seq, source_attempt(&source.input)),
                        checkpoint: seeded_send(plan.turn),
                        lineage_id: plan.lineage_id.clone(),
                        attempt_no: plan.attempt_no,
                        rewound_from_run_id: input.source_run_id.clone(),
                        rewound_from_seq: input.seq,
                    },
                )
                .await
                .map_err(|e| format!("attempt insert: {e}"))?;
                sqlx::query(
                    "insert into workbench_jobs \
                       (id, agent_id, agent_model, task_id, repo, branch, effort, plan, status, host_id, run_id) \
                     values ($1::uuid, $2::uuid, $3, $4::uuid, $5, $6, $7, $8, 'started', $9, $10::uuid)",
                )
                .bind(&plan.job_id)
                .bind(&plan.agent_id)
                .bind(&input.agent_model)
                .bind(&input.task_id)
                .bind(&plan.repo)
                .bind(&plan.branch)
                .bind(&plan.effort)
                .bind(&plan.job_plan)
                .bind(&plan.host_id)
                .bind(&plan.attempt_run_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("job insert: {e}"))?;
                sqlx::query(
                    "delete from work_dispatch_holds \
                     where task_id = $1::uuid and agent_model = $2 and rewind_run_id = $3::uuid",
                )
                .bind(&input.task_id)
                .bind(&input.agent_model)
                .bind(rewind_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("hold lift: {e}"))?;
                // The person asked for the work to go on; their earlier stop
                // of this pair is lifted with it.
                sqlx::query(
                    "delete from work_dispatch_blocks where task_id = $1::uuid and agent_model = $2",
                )
                .bind(&input.task_id)
                .bind(&input.agent_model)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("block lift: {e}"))?;
                tx.commit().await.map_err(|e| format!("commit: {e}"))?;
                Ok(())
            }
            .await;
            lock.release().await;
            outcome?;
            if let Err(e) = supersede_jobs(pg, &plan.department, &plan.job_id).await {
                tracing::warn!("{LOG} {rewind_id}: the source's jobs were not retired: {e}");
            }
            let _ = talaria_tasks::log_activity(
                pg,
                &input.task_id,
                &input.requested_by,
                "dispatch",
                &format!(
                    "rewound to step {} — attempt {} continues from there in job {} on {}",
                    input.seq, plan.attempt_no, plan.job_id, plan.branch
                ),
            )
            .await;
            let attempt = plan.attempt_run_id.clone();
            tokio::spawn(async move {
                if let Err(e) = talaria_runs_run::drive(&attempt, &deps).await {
                    tracing::error!("{LOG} detached drive of attempt {attempt} threw: {e}");
                }
            });
            Ok(())
        }
        .boxed()
    }

    fn fail<'a>(
        &'a self,
        rewind_id: &'a str,
        input: &'a RewindInput,
        plan: Option<&'a RewindPlan>,
        stage: Option<Stage>,
    ) -> BoxFuture<'a, ()> {
        async move {
            let pg = self.pg();
            if let Err(e) =
                sqlx::query("delete from work_dispatch_holds where rewind_run_id = $1::uuid")
                    .bind(rewind_id)
                    .execute(pg)
                    .await
            {
                tracing::warn!("{LOG} {rewind_id}: the hold was not lifted: {e}");
            }
            let Some(plan) = plan else { return };
            if let Err(e) = container::remove_checkout(pg, &input.agent_model, &plan.workdir).await
            {
                tracing::warn!("{LOG} {rewind_id}: {} was not removed: {e}", plan.workdir);
            }
            let forked = matches!(
                stage,
                Some(Stage::Cut | Stage::Restore | Stage::Verify | Stage::Start)
            );
            if forked
                && let Err(e) =
                    container::clear_session(pg, &input.agent_model, &plan.attempt_run_id).await
            {
                tracing::warn!("{LOG} {rewind_id}: the forked session was not cleared: {e}");
            }
        }
        .boxed()
    }
}

fn source_attempt(input: &Value) -> i64 {
    input["rewind"]["attemptNo"].as_i64().unwrap_or(1)
}

/// The last row's tool call id, when the last row is a tool result.
pub fn tail_tool_call(rows: &[Value]) -> Option<String> {
    let last = rows.last()?;
    if last["role"].as_str() != Some("tool") {
        return None;
    }
    last["tool_call_id"].as_str().map(str::to_string)
}
