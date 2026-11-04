// Every edge a rewind touches, behind one trait so the stage machine can be
// driven with no database or container. `Real` is the one implementation the
// boot wiring arms.

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde_json::Value;
use sqlx::{FromRow, PgPool};

use talaria_fleet_docker::rewind as container;
use talaria_fleet_docker::rewind::SessionLease;
use talaria_gateway::fleet_chat::{self, SessionAt};
use talaria_runs_define::is_terminal;
use talaria_runs_run::RunDeps;
use talaria_runs_store::{
    AttemptSeed, CancelOutcome, LINEAGE_COLS, PgRunStore, RunCheckpoint, RunLineage, RunStore,
    insert_attempt, run_checkpoints,
};
use talaria_runs_work_session::{
    AgentStop, WORK_SESSION_KIND, WorkSessionInput, seeded_send, session_run_id, stop_agent_turn,
};
use talaria_state::AppState;
use talaria_statuses::status_meta;
use talaria_tasks_types::{TaskActor, TaskDeps, TaskPatch};
use talaria_work_dispatch::MAX_SESSIONS_PER_TICKET_AGENT;
use talaria_work_dispatch::holds::{lock_pair, place_hold};
use talaria_workbench_mcp::rewind::supersede_jobs;
use talaria_workbench_mcp::teardown::job_workdir;

use crate::{RewindInput, RewindPlan, Validated, attempt_input};

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
    /// The agent no longer knows the run while the session still holds its
    /// turn's lease: the turn died with the process, and nothing can stop it.
    TurnLost {
        lease_ends_in_secs: u64,
    },
}

/// What the budget's last answers amount to. Hermes forgets its runs when its
/// process dies and keeps the lease on disk until it expires.
pub fn quiet_end(why: String, lease: Option<SessionLease>) -> Quiet {
    match lease {
        Some(SessionLease::Held { ends_in_secs }) if why.contains("run_not_found") => {
            Quiet::TurnLost {
                lease_ends_in_secs: ends_in_secs,
            }
        }
        Some(SessionLease::Held { .. }) => Quiet::Unconfirmed(format!(
            "{why}; the agent still holds a turn on the session"
        )),
        _ => Quiet::Unconfirmed(why),
    }
}

/// A generation with no run row, as a rewind reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Free,
    Spent,
    /// A fork of one of this pair's sessions: only a rewind makes one, and with
    /// no run row it is what a rewind that could not reach the agent to clean
    /// up left behind.
    Leftover {
        parent: String,
    },
}

pub fn slot_of(session: &SessionAt, pair_sessions: &[String]) -> Slot {
    match session {
        SessionAt::Absent => Slot::Free,
        SessionAt::Held {
            parent: Some(parent),
        } if pair_sessions.contains(parent) => Slot::Leftover {
            parent: parent.clone(),
        },
        SessionAt::Held { .. } => Slot::Spent,
    }
}

/// The attempt row as the world reads it: enough to tell this rewind's own
/// attempt from a session claimed by someone else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptRow {
    pub run_id: String,
    pub job_id: String,
}

/// Where a rewind may take the ticket, read at validate and again at start:
/// a working column stays, review goes back to the working column, and done
/// or any other column refuses. A person's later move is never overwritten.
pub fn column_move(ticket: &Ticket) -> Result<Option<String>, String> {
    match &ticket.column {
        TicketColumn::Working => Ok(None),
        TicketColumn::Review => ticket.active_key.clone().map(Some).ok_or_else(|| {
            format!(
                "the ticket is in review (\"{}\") and the board has no working column to move it back to",
                ticket.status
            )
        }),
        TicketColumn::Done => Err(format!(
            "the ticket is done (\"{}\") — a finished ticket is not rewound",
            ticket.status
        )),
        TicketColumn::Other => Err(format!(
            "the ticket is in \"{}\", which is not a column an agent works in — move it first",
            ticket.status
        )),
    }
}

/// What one round of the quiet stage concludes. The agent's own word that the
/// turn ended, or no turn at all, is quiet; so is a session with no turn
/// lease in the agent's store. Anything else keeps polling until the budget.
pub fn quiet_from(stop: &AgentStop, idle: Option<bool>) -> Option<Quiet> {
    match stop {
        AgentStop::Ended(_) | AgentStop::NoTurn => Some(Quiet::Confirmed),
        AgentStop::Unconfirmed(_) if idle == Some(true) => Some(Quiet::Confirmed),
        AgentStop::Unconfirmed(_) => None,
    }
}

/// A path the container reported is used only when it is the workdir or
/// sits beneath it, spelled plainly.
pub fn contained(workdir: &str, root: &str) -> bool {
    if root.is_empty() || root.contains('\n') || root.split('/').any(|part| part == "..") {
        return false;
    }
    root == workdir
        || root
            .strip_prefix(workdir)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub trait RewindWorld: Send + Sync {
    fn source<'a>(&'a self, input: &'a RewindInput) -> BoxFuture<'a, Result<Source, String>>;
    /// The ticket as it is now.
    fn ticket<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Result<Option<Ticket>, String>>;
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
    /// The attempt row under the plan's run id, when one exists.
    fn attempt<'a>(&'a self, run_id: &'a str) -> BoxFuture<'a, Result<Option<AttemptRow>, String>>;
    /// Move the ticket to `move_to` when given, insert the attempt and its
    /// job with the hold lifted, retire the source's jobs, and drive the
    /// attempt. Called once, for an attempt that does not exist yet.
    fn start<'a>(
        &'a self,
        rewind_id: &'a str,
        input: &'a RewindInput,
        plan: &'a RewindPlan,
        move_to: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), String>>;
    /// Lift this rewind's hold. Best effort; the run is already failing.
    fn lift_hold<'a>(&'a self, rewind_id: &'a str) -> BoxFuture<'a, ()>;
    /// Take back the checkout and, when `forked`, the session a failed rewind
    /// placed. Only for an attempt known not to exist. Best effort: a fork it
    /// cannot reach is taken back by the next rewind of the pair.
    fn discard<'a>(
        &'a self,
        input: &'a RewindInput,
        plan: &'a RewindPlan,
        forked: bool,
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
            let ticket = self.ticket(&session.task_id).await?;
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

    fn ticket<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Result<Option<Ticket>, String>> {
        async move {
            let pg = self.pg();
            let Some(task) = talaria_tasks::get_task(pg, task_id)
                .await
                .map_err(|e| format!("ticket read: {e}"))?
            else {
                return Ok(None);
            };
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
            Ok(Some(Ticket {
                ticket_ref: task.ticket_ref.clone(),
                title: task.title.clone(),
                status: task.status.clone(),
                column,
                active_key: meta.active_key.clone().or(meta.assigned_key.clone()),
            }))
        }
        .boxed()
    }

    fn attempt<'a>(&'a self, run_id: &'a str) -> BoxFuture<'a, Result<Option<AttemptRow>, String>> {
        async move {
            let row: Option<(String, Option<String>)> = sqlx::query_as(
                "select id::text, input->'rewind'->>'jobId' from runs where id = $1::uuid",
            )
            .bind(run_id)
            .fetch_optional(self.pg())
            .await
            .map_err(|e| format!("run read: {e}"))?;
            Ok(row.map(|(run_id, job_id)| AttemptRow {
                run_id,
                job_id: job_id.unwrap_or_default(),
            }))
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
                .map_err(|e| e.to_string())?;
            let root = container::git_root(pg, agent_model, &workdir).await?;
            if !contained(&workdir, &root) {
                return Err(format!(
                    "the container reported a checkout at {root:?}, outside {workdir}"
                ));
            }
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
                let pair_sessions: Vec<String> = (0..MAX_SESSIONS_PER_TICKET_AGENT)
                    .map(|generation| session_run_id(task_id, agent_model, generation))
                    .collect();
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
                    match slot_of(&fleet_chat::session_at(agent_model, &id).await?, &pair_sessions) {
                        Slot::Free => {}
                        Slot::Spent => continue,
                        Slot::Leftover { parent } => {
                            if let Err(e) =
                                container::discard_session(pg, agent_model, &id, &parent).await
                            {
                                tracing::warn!("{LOG} {rewind_id}: a failed rewind's fork stays: {e}");
                                continue;
                            }
                        }
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
            // the cancel, and the stop then lands on it. Until the budget
            // ends, the stop is sent again and the agent's store is asked
            // whether the session holds a turn lease at all. A Talaria row
            // that reads finished proves nothing about the agent.
            let deadline = tokio::time::Instant::now() + QUIET_BUDGET;
            let session = run_id;
            let agent_model: Option<String> =
                sqlx::query_scalar("select input->>'agentModel' from runs where id = $1::uuid")
                    .bind(run_id)
                    .fetch_optional(self.pg())
                    .await
                    .ok()
                    .flatten();
            loop {
                let stop =
                    match tokio::time::timeout_at(deadline, stop_agent_turn(self.pg(), run_id))
                        .await
                    {
                        Ok(stop) => stop,
                        Err(_) => AgentStop::Unconfirmed("no answer inside the stop window".into()),
                    };
                let lease = match &agent_model {
                    Some(model) => container::session_lease(self.pg(), model, session)
                        .await
                        .ok(),
                    None => None,
                };
                if let Some(verdict) = quiet_from(&stop, lease.map(|l| l == SessionLease::Idle)) {
                    return verdict;
                }
                let why = match stop {
                    AgentStop::Unconfirmed(why) => why,
                    _ => unreachable!("a confirmed stop has returned"),
                };
                if tokio::time::Instant::now() + QUIET_POLL >= deadline {
                    return quiet_end(why, lease);
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
        move_to: Option<&'a str>,
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
                if deps
                    .store
                    .get(&plan.attempt_run_id)
                    .await
                    .map_err(|e| format!("run read: {e}"))?
                    .is_some()
                {
                    return Err(format!(
                        "session {} was claimed while the rewind ran",
                        plan.attempt_run_id
                    ));
                }
                if let Some(status) = move_to {
                    let redis = self.state.redis().await.ok();
                    let task_deps = TaskDeps::from_route(pg.clone(), redis);
                    talaria_tasks::update_task(
                        &task_deps,
                        &input.task_id,
                        TaskPatch {
                            status: Some(status.to_string()),
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

    fn lift_hold<'a>(&'a self, rewind_id: &'a str) -> BoxFuture<'a, ()> {
        async move {
            if let Err(e) =
                sqlx::query("delete from work_dispatch_holds where rewind_run_id = $1::uuid")
                    .bind(rewind_id)
                    .execute(self.pg())
                    .await
            {
                tracing::warn!("{LOG} {rewind_id}: the hold was not lifted: {e}");
            }
        }
        .boxed()
    }

    fn discard<'a>(
        &'a self,
        input: &'a RewindInput,
        plan: &'a RewindPlan,
        forked: bool,
    ) -> BoxFuture<'a, ()> {
        async move {
            let pg = self.pg();
            if let Err(e) = container::remove_checkout(pg, &input.agent_model, &plan.workdir).await
            {
                tracing::warn!(
                    "{LOG} {}: {} was not removed: {e}",
                    input.source_run_id,
                    plan.workdir
                );
            }
            if forked
                && let Err(e) = container::discard_session(
                    pg,
                    &input.agent_model,
                    &plan.attempt_run_id,
                    &input.source_run_id,
                )
                .await
            {
                tracing::warn!(
                    "{LOG} {}: the forked session stays: {e}",
                    input.source_run_id
                );
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
