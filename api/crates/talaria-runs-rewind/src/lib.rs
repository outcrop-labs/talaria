// A REWIND, as a durable run.
//
// Rewinding a work session to one of its ledger steps makes a new attempt: a
// second work-session run on the same ticket and agent whose Hermes session
// is the source's conversation cut at that step, and whose job checkout is the
// step's snapshot. The rewind is its own `runs` row of kind `run-rewind`, so
// it gets the lease, restart recovery and an error column from the run plane:
// a restart mid-rewind resumes at the stage it reached, and a rewind that
// fails is a failed `run-rewind` row with its reason and NEVER an empty or
// finished work session. The attempt row appears only at the last stage, once
// the conversation and the tree have been verified.
//
// THE STAGES run in order and each is safe to repeat after a crash:
//
//   validate  the step is restorable, its commit exists, the agent answers,
//             the ticket can take the work, and a session slot is free. The
//             pair's dispatch hold is placed here, and the plan every later
//             stage reads is fixed here.
//   stop      the source run is cancelled.
//   quiet     the agent reports the source's turn over, within the stop window.
//   fork      the source session is forked under the attempt's run id.
//   cut       the fork is cut after the step's tool result row.
//   restore   the snapshot commit is checked out into the attempt's workdir.
//   verify    the fork ends at that row and the checkout's tree is the step's.
//   start     the ticket is moved if it sat in review, the attempt and its job
//             are inserted in one transaction with the hold lifted, the
//             source's jobs are retired, and the attempt is driven.
//
// WHAT DECIDES AND WHAT ACTS are kept apart. `validate` is pure and the step
// is written against the `RewindWorld` trait, so the tests drive every stage,
// every refusal and a crash after each stage with no database or container.
// `world::Real` is the one real implementation.

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use talaria_runs_define::{
    Authority, RunDefinition, RunRow, RunState, RunStepContext, StepResult, is_terminal,
    register_run,
};
use talaria_runs_store::{CheckpointKind, RunCheckpoint};
use talaria_runs_work_session::{RewindInput as AttemptBinding, WorkSessionInput};

pub mod world;
pub use world::{
    Checkout, Cut, Fork, Quiet, Reservation, RewindWorld, Source, SourceJob, Ticket, TicketColumn,
};

/// The registry key and the `kind` column. Stable forever.
pub const REWIND_KIND: &str = "run-rewind";

const LOG: &str = "[run-rewind]";

/// What a person asked for: this step of this run. The ticket, agent and
/// board ride along so the audience and the hold need no second read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindInput {
    pub source_run_id: String,
    pub seq: i64,
    /// The person's user id: the ticket move is logged as their action.
    pub requested_by: String,
    pub task_id: String,
    pub agent_model: String,
    pub board_id: String,
}

/// The stage the next step runs. `validate` is the null checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Stop,
    Quiet,
    Fork,
    Cut,
    Restore,
    Verify,
    Start,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Stage::Stop => "stop",
            Stage::Quiet => "quiet",
            Stage::Fork => "fork",
            Stage::Cut => "cut",
            Stage::Restore => "restore",
            Stage::Verify => "verify",
            Stage::Start => "start",
        }
    }
}

/// Everything the stages after `validate` need, fixed once. A crash re-runs a
/// stage from the same plan, so no stage can drift from another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindPlan {
    pub attempt_run_id: String,
    pub generation: u32,
    pub attempt_no: i32,
    pub lineage_id: String,
    /// The turn the attempt starts at: the step's own, so the budget carries.
    pub turn: i64,
    pub tool_call_id: String,
    pub commit: String,
    pub tree: String,
    pub source_job_id: String,
    /// The git checkout the snapshot was taken in, inside the container.
    pub source_root: String,
    pub job_id: String,
    pub workdir: String,
    /// The attempt's checkout: `workdir` plus the source root's place under
    /// the source workdir, so the agent finds the files where it left them.
    pub root: String,
    pub branch: String,
    pub repo: String,
    pub host_id: String,
    pub agent_id: String,
    pub effort: String,
    pub job_plan: String,
    pub origin_url: String,
    pub department: String,
    pub board_name: Option<String>,
    /// The working column a ticket in review goes back to.
    pub move_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindCheckpoint {
    pub stage: Stage,
    pub plan: RewindPlan,
}

/// What `validate` decided from the source reads, before any side effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    pub row: RunCheckpoint,
    pub tool_call_id: String,
    pub commit: String,
    pub tree: String,
    pub turn: i64,
    pub job: SourceJob,
    pub lineage_id: String,
    pub ticket_ref: String,
    pub title: String,
    pub board_name: Option<String>,
    pub move_to: Option<String>,
}

/// The refusals, in the plan's order: the step, then the ticket. Pure.
pub fn validate(source: &Source) -> Result<Validated, String> {
    let row = source
        .row
        .as_ref()
        .ok_or_else(|| format!("run {} has no ledger row {}", source.run_id, source.seq))?;
    if row.kind != CheckpointKind::Step {
        return Err(format!(
            "ledger row {} is a turn boundary, not a step — only a step can be rewound to",
            row.seq
        ));
    }
    if !row.restorable {
        return Err(format!(
            "step {} cannot be restored: {}",
            row.seq,
            row.unrestorable_reason
                .as_deref()
                .unwrap_or("the step has no snapshot")
        ));
    }
    let (Some(tool_call_id), Some(commit), Some(tree), Some(job_id)) = (
        row.tool_call_id.clone(),
        row.workspace_commit.clone(),
        row.workspace_tree.clone(),
        row.job_id.clone(),
    ) else {
        return Err(format!(
            "step {} is marked restorable but names no tool call, commit, tree or job",
            row.seq
        ));
    };
    let job = source
        .job
        .as_ref()
        .filter(|job| job.id == job_id)
        .cloned()
        .ok_or_else(|| format!("step {}'s job {job_id} is gone", row.seq))?;
    let ticket = source
        .ticket
        .as_ref()
        .ok_or_else(|| "the ticket is gone".to_string())?;
    let move_to = match &ticket.column {
        TicketColumn::Working => None,
        TicketColumn::Review => Some(ticket.active_key.clone().ok_or_else(|| {
            format!(
                "the ticket is in review (\"{}\") and the board has no working column to move it back to",
                ticket.status
            )
        })?),
        TicketColumn::Done => {
            return Err(format!(
                "the ticket is done (\"{}\") — a finished ticket is not rewound",
                ticket.status
            ));
        }
        TicketColumn::Other => {
            return Err(format!(
                "the ticket is in \"{}\", which is not a column an agent works in — move it first",
                ticket.status
            ));
        }
    };
    Ok(Validated {
        row: row.clone(),
        tool_call_id,
        commit,
        tree,
        turn: row.turn.unwrap_or(1).max(1),
        job,
        lineage_id: source.lineage.lineage_id.clone(),
        ticket_ref: ticket.ticket_ref.clone().unwrap_or_default(),
        title: ticket.title.clone(),
        board_name: source.input.board_name.clone(),
        move_to,
    })
}

/// The attempt's input: the source's ticket, agent and board, the reserved
/// generation, and the binding its first prompt reads.
pub fn attempt_input(
    source_run_id: &str,
    source: &WorkSessionInput,
    plan: &RewindPlan,
) -> WorkSessionInput {
    WorkSessionInput {
        task_id: source.task_id.clone(),
        agent_model: source.agent_model.clone(),
        board_id: source.board_id.clone(),
        board_name: source.board_name.clone(),
        generation: plan.generation as i64,
        rewind: Some(AttemptBinding {
            from_run_id: source_run_id.to_string(),
            turn: plan.turn,
            attempt_no: plan.attempt_no,
            job_id: plan.job_id.clone(),
            branch: plan.branch.clone(),
            workdir: plan.root.clone(),
        }),
    }
}

fn as_input(raw: &Value) -> Result<RewindInput, String> {
    serde_json::from_value(raw.clone())
        .map_err(|_| "run-rewind run has no readable input (sourceRunId / seq)".into())
}

fn as_checkpoint(raw: &Value) -> Result<Option<RewindCheckpoint>, String> {
    if raw.is_null() {
        return Ok(None);
    }
    serde_json::from_value(raw.clone())
        .map(Some)
        .map_err(|e| format!("run-rewind checkpoint could not be read: {e}"))
}

fn next(stage: Stage, plan: RewindPlan) -> StepResult {
    StepResult::Next {
        checkpoint: json!(RewindCheckpoint { stage, plan }),
        phase: Some(stage.label().to_string()),
    }
}

/// ONE STAGE. A failure anywhere ends the run with its reason, after the
/// world has taken back what the rewind had placed.
pub async fn rewind_step(
    ctx: RunStepContext,
    world: &dyn RewindWorld,
) -> Result<StepResult, String> {
    let input = as_input(&ctx.input)?;
    let cp = as_checkpoint(&ctx.checkpoint)?;
    let rewind_id = ctx.run.id.clone();
    let outcome = stage(&ctx, &input, cp.as_ref(), world).await;
    match outcome {
        Ok(result) => Ok(result),
        Err(why) => {
            if ctx.signal.is_aborted() {
                return Err(why);
            }
            tracing::warn!("{LOG} {rewind_id}: failed: {why}");
            world
                .fail(
                    &rewind_id,
                    &input,
                    cp.as_ref().map(|c| &c.plan),
                    cp.as_ref().map(|c| c.stage),
                )
                .await;
            Err(why)
        }
    }
}

async fn stage(
    ctx: &RunStepContext,
    input: &RewindInput,
    cp: Option<&RewindCheckpoint>,
    world: &dyn RewindWorld,
) -> Result<StepResult, String> {
    let Some(cp) = cp else {
        let source = world.source(input).await?;
        let validated = validate(&source)?;
        let source_root = world
            .reach(&input.agent_model, &validated.job.id, &validated.commit)
            .await?;
        let reservation = world
            .reserve(&ctx.run.id, input, &source, &validated, &source_root)
            .await?;
        let plan = RewindPlan {
            attempt_run_id: reservation.attempt_run_id,
            generation: reservation.generation,
            attempt_no: reservation.attempt_no,
            lineage_id: validated.lineage_id,
            turn: validated.turn,
            tool_call_id: validated.tool_call_id,
            commit: validated.commit,
            tree: validated.tree,
            source_job_id: validated.job.id.clone(),
            source_root,
            job_id: reservation.job_id,
            workdir: reservation.workdir,
            root: reservation.root,
            branch: reservation.branch,
            repo: validated.job.repo,
            host_id: validated.job.host_id,
            agent_id: validated.job.agent_id,
            effort: validated.job.effort,
            job_plan: validated.job.plan,
            origin_url: reservation.origin_url,
            department: reservation.department,
            board_name: validated.board_name,
            move_to: validated.move_to,
        };
        (ctx.log)(format!(
            "attempt {} will continue from step {} at turn {} in job {}",
            plan.attempt_no, input.seq, plan.turn, plan.job_id
        ));
        return Ok(next(Stage::Stop, plan));
    };
    let plan = cp.plan.clone();
    match cp.stage {
        Stage::Stop => {
            world.cancel_source(&input.source_run_id).await?;
            Ok(next(Stage::Quiet, plan))
        }
        Stage::Quiet => match world.quiet(&input.source_run_id).await {
            Quiet::Confirmed => Ok(next(Stage::Fork, plan)),
            Quiet::Unconfirmed(why) => Err(format!(
                "the agent was not quiet within the stop window ({why}) — the old attempt stays stopped; \
                 rewind again once its turn has ended"
            )),
        },
        Stage::Fork => {
            match world
                .fork(
                    &input.agent_model,
                    &input.source_run_id,
                    &plan.attempt_run_id,
                )
                .await?
            {
                Fork::Created => {}
                Fork::Exists { parent_session_id }
                    if parent_session_id.as_deref() == Some(input.source_run_id.as_str()) => {}
                Fork::Exists { .. } => {
                    return Err(format!(
                        "the agent already holds a session {} that is not a fork of this run",
                        plan.attempt_run_id
                    ));
                }
            }
            Ok(next(Stage::Cut, plan))
        }
        Stage::Cut => {
            let cut = world
                .cut(&input.agent_model, &plan.attempt_run_id, &plan.tool_call_id)
                .await?;
            if cut.last_tool_call_id.as_deref() != Some(plan.tool_call_id.as_str()) {
                return Err(format!(
                    "the cut left the fork ending at {:?}, not at tool call {}",
                    cut.last_tool_call_id, plan.tool_call_id
                ));
            }
            (ctx.log)(format!("cut {} rows to {}", cut.was, cut.now));
            Ok(next(Stage::Restore, plan))
        }
        Stage::Restore => {
            let tree = world.restore(&input.agent_model, &plan).await?;
            if tree != plan.tree {
                return Err(format!(
                    "the restored checkout has tree {tree}, not the step's {}",
                    plan.tree
                ));
            }
            Ok(next(Stage::Verify, plan))
        }
        Stage::Verify => {
            let tail = world
                .session_tail(&input.agent_model, &plan.attempt_run_id)
                .await?;
            if tail.as_deref() != Some(plan.tool_call_id.as_str()) {
                return Err(format!(
                    "the fork ends at {tail:?}, not at tool call {}",
                    plan.tool_call_id
                ));
            }
            let checkout = world.checkout(&input.agent_model, &plan.root).await?;
            if checkout.tree != plan.tree || !checkout.clean || checkout.branch != plan.branch {
                return Err(format!(
                    "the checkout at {} is not the step's: tree {} on {}{}",
                    plan.root,
                    checkout.tree,
                    checkout.branch,
                    if checkout.clean {
                        ""
                    } else {
                        ", with uncommitted changes"
                    }
                ));
            }
            Ok(next(Stage::Start, plan))
        }
        Stage::Start => {
            world.start(&ctx.run.id, input, &plan).await?;
            Ok(StepResult::Done {
                result: json!({
                    "attemptRunId": plan.attempt_run_id,
                    "attemptNo": plan.attempt_no,
                    "lineageId": plan.lineage_id,
                    "jobId": plan.job_id,
                    "branch": plan.branch,
                    "turn": plan.turn,
                }),
            })
        }
    }
}

/// Whether a source run may be forked without tearing a turn: terminal and
/// settled, or confirmed quiet by the agent. The world's `quiet` applies it.
pub fn settled(state: RunState, finished_ms: Option<i64>, now_ms: i64, settle_ms: i64) -> bool {
    is_terminal(state) && finished_ms.is_some_and(|at| now_ms - at >= settle_ms)
}

// ── The definition ───────────────────────────────────────────────────────────

fn audience(run: &RunRow) -> Authority {
    match run
        .input
        .get("boardId")
        .and_then(Value::as_str)
        .filter(|b| !b.is_empty())
    {
        Some(board_id) => Authority::Board {
            board_id: board_id.to_string(),
        },
        None => Authority::Nobody,
    }
}

static ARMED: OnceLock<Arc<dyn RewindWorld>> = OnceLock::new();

pub fn arm_rewind_step(world: Arc<dyn RewindWorld>) {
    let _ = ARMED.set(world);
}

pub fn rewind_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: REWIND_KIND.into(),
            label: "Rewind".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(world) = ARMED.get().cloned() else {
                        return Err(
                            "run-rewind steps are armed by the scheduler's boot wiring; this step \
                             was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    rewind_step(ctx, world.as_ref()).await
                })
            }),
            audience: Arc::new(audience),
            // The stop window is 30 s and a restore is a local clone; two
            // minutes is the whole rewind with room, and it is also the
            // lease, so a crashed rewind resumes two minutes later.
            max_step_ms: 120_000,
            idle_step_ms: None,
            max_attempts: 3,
        })
    })
}

#[cfg(test)]
mod tests;
