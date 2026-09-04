// AGENT WORK SESSIONS, as a durable run.
//
// WHAT A SESSION IS. What used to be a `for` loop inside a fire-and-forget
// promise, guarded by a process-local `Set`, is a run: the loop index is the
// checkpoint, the guard is a Redis lease, and a driver that dies is reclaimed
// and re-enters at the turn it left. See work_dispatch's header for the push
// side of that story.
//
// THE STEP MACHINE, and why it has three stages rather than one turn per step.
//   A turn does TWO outward things: it calls a model (billed, and the agent's
//   tools may write to the ticket, push a branch, open a PR) and it writes a
//   line to `task_activity`. The runtime is AT-LEAST-ONCE — a step that ran
//   and did not checkpoint runs again — so those two live in different steps,
//   each checkpointing straight after its own effect:
//
//     (checkpoint null)  brief   → re-check authority, write the dispatch line
//     stage 'send'               → call the model for turn N, CHECKPOINT THE REPLY
//     stage 'record'             → write turn N's activity line, advance to N+1
//     stage 'failed'             → write the failure line, then fail the run
//
//   THE REPLY IS PERSISTED BEFORE IT IS ACTED ON, which is the whole reason
//   `record` is a separate step. Everything the session does with a reply —
//   the activity line, and the DONE/BLOCKED test that decides the next
//   prompt — runs off the checkpoint, so a crash anywhere after the model
//   answered costs a database write and not a second billed turn.
//
// THE ONE EFFECT THIS FILE CANNOT MAKE IDEMPOTENT is the model call itself.
// There is no handle from the far side to carry into the checkpoint — and the
// agent on the other end of a work-session turn is not a completion, it is a
// tool loop that may have already commented on the ticket and pushed a
// branch. So this file does not re-send. `stage_attempt` records the attempt
// at the moment a stage was written; a `send` step that finds a HIGHER
// attempt knows a driver died while this turn was in flight, and it retires
// the turn instead of repeating it — one activity line saying the outcome is
// unknown, a settle interval for anything still running on the far side, and
// on to the next turn against the ticket's LIVE state. A lost turn is cheap.
// A duplicated one is a second agent working the same ticket.
//
// CHECKPOINT SPELLING. The checkpoint column's spelling (`stageAttempt`,
// `lastTail`, `notBefore`) is fixed: rows written by earlier deploys must
// keep resuming, and a renamed field would reset every live session
// mid-work. Pinned by round-trip tests.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::harness::defs::work_session::{dispatch_prompt as dispatch_brief, work_session_harness};
use crate::harness::run::{RunContext, RunLedger, run_harness};
use crate::harness::transport::LedgerSource;
use crate::runs::define::{
    Authority, RunDefinition, RunRow, RunStepContext, StepResult, register_run,
};
use crate::state::AppState;
use crate::{statuses, tasks, workflows};

/// The registry key, and the `kind` column on every row this definition has
/// ever produced. Stable forever — a rename orphans every session mid-flight.
pub const WORK_SESSION_KIND: &str = "work-session";

/// Turn budget: generous enough for real feature work, finite enough that a
/// looping agent can't burn unbounded tokens. The session also ends the
/// moment the ticket leaves the working statuses.
///
/// IT IS IN THE CHECKPOINT, so it SURVIVES A RESUME — without that, a
/// restart mid-session loses the count, the heartbeat re-dispatches, and the
/// agent gets a fresh twelve turns on work it had already spent twelve turns
/// on.
pub const MAX_SESSION_TURNS: i64 = 12;

/// How long a reclaimed session waits before sending its NEXT turn, when the
/// turn it was reclaimed from may still be running.
///
/// The driver dying killed the HTTP request, not the agent: the persona has
/// the prompt and is inside its own tool loop, and the work-session harness's
/// hold says we are willing to wait ten minutes for one of those. Sending the
/// next turn immediately would put two prompts into one session. Two minutes
/// is the compromise the other end of this path already uses (`proxyChat`'s
/// default wait) — long enough that the common case has landed, short enough
/// that a reclaimed session reads as a pause rather than a stall.
pub const INTERRUPTED_TURN_SETTLE_MS: i64 = 120_000;

const LOG: &str = "[work-session]";

// ── The row's identity ───────────────────────────────────────────────────────

/// THE run id for one session, derived and not random.
///
/// `enqueue` deduplicates nothing above the row, so a caller that retries its
/// request — or a second instance handling the same ticket mutation — creates
/// a SECOND run doing the same work. The runtime's answer to that is a
/// deterministic id: the primary key refuses the duplicate, so a double
/// dispatch produces one run and one loser that reads its own rejection as
/// "somebody already has this".
///
/// `generation` is what keeps that claim from becoming a life sentence. The
/// invariant is ONE LIVE SESSION per ticket+agent, not one ever: a ticket
/// that legitimately comes back to the same agent next week needs a new row,
/// and a row whose id is a pure function of (task, agent) has nowhere to put
/// it. So `dispatch_ticket_work` walks generations, skipping the finished
/// ones and standing down on the first live one it finds.
///
/// A NAME-BASED UUID rather than a hash string, because `runs.id` is `uuid`.
/// Version 8 is RFC 9562's "custom" — an honest label for "the bytes are a
/// SHA-256 of a name", which is what they are; v5 would claim SHA-1.
///
/// PINNED by fixed vectors, not re-derived here (which would test the
/// derivation against itself): two processes deriving different ids from the
/// same ticket would silently split one session into two.
pub fn session_run_id(task_id: &str, agent_model: &str, generation: u32) -> String {
    let digest = Sha256::digest(format!(
        "{WORK_SESSION_KIND}\u{0}{task_id}\u{0}{agent_model}\u{0}{generation}"
    ));
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x80; // version 8
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10x
    let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ── Input and checkpoint ─────────────────────────────────────────────────────

/// Wire shape (`taskId`/`agentModel`/…) — the row's spelling, kept stable so
/// rows written by earlier deploys stay legible.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkSessionInput {
    pub task_id: String,
    pub agent_model: String,
    /// The board, carried on the INPUT rather than looked up, because
    /// `audience()` gets nothing but the row — and the row's subject is the
    /// TICKET. A run that could not name its board could not name who may
    /// decide anything it ever asked.
    pub board_id: String,
    /// Only ever set by a caller that already had it; it goes into the
    /// dispatch prompt's header and nowhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_name: Option<String>,
    /// Which session this is for this ticket+agent. See `session_run_id`.
    #[serde(default)]
    pub generation: i64,
}

/// What one turn's reply is kept as.
///
/// BOUNDED SLICES, not the whole reply, and each one is a slice the session
/// actually reads: `head` is what goes on the activity line (the widest of
/// the three cuts the lines take), `tail` is the window the DONE/BLOCKED
/// test runs against, `checks` is the guard finding ids. The checkpoint is
/// read at every step boundary and published nowhere; keeping a whole model
/// reply in it would be paying for the full transcript on every beat of the
/// session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnReply {
    head: String,
    tail: String,
    checks: Vec<String>,
}

/// Which prompt produced a reply — and therefore how its activity line reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TurnKind {
    Dispatch,
    Turn,
    Reconcile,
}

/// The three-stage machine, in the wire spelling (`stage` tag, camelCase
/// fields, `notBefore` present only on the retired-turn path — skipped when
/// None).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "stage",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
enum WorkSessionCheckpoint {
    /// A turn is owed. Nothing has been sent for `turn` yet — or, after a
    /// reclaim, something may have been and we will never know.
    Send {
        turn: i64,
        stage_attempt: i64,
        last_tail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        not_before: Option<i64>,
    },
    /// A reply is persisted and has not reached the ticket yet. `reply: None`
    /// is the retired turn — the model call may or may not have happened.
    Record {
        turn: i64,
        stage_attempt: i64,
        said: TurnKind,
        reply: Option<TurnReply>,
    },
    /// The session is over and the run must be filed as an error, but the
    /// ticket has not been told yet.
    Failed {
        turn: i64,
        stage_attempt: i64,
        error: String,
    },
}

/// A row whose input this deploy cannot read is a bug report, not something
/// to guess at: guessing would dispatch a turn at an agent nobody named.
fn as_input(raw: &Value) -> Result<WorkSessionInput, String> {
    serde_json::from_value(raw.clone()).map_err(|_| {
        "work-session run has no readable input (taskId / agentModel / boardId)".into()
    })
}

/// Null is the brief. An unknown stage is a bug report, same as the input —
/// and the sentence names the stage it choked on, because that sentence is
/// what a person reading the run's error column can act on.
fn as_checkpoint(raw: &Value) -> Result<Option<WorkSessionCheckpoint>, String> {
    if raw.is_null() {
        return Ok(None);
    }
    let stage = raw.get("stage").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(stage, "send" | "record" | "failed") {
        return Err(format!(
            "work-session checkpoint has an unknown stage \"{stage}\""
        ));
    }
    serde_json::from_value(raw.clone())
        .map_err(|e| format!("work-session checkpoint could not be read: {e}"))
}

// ── What the session is allowed to see ───────────────────────────────────────

/// Where the ticket is now — the session's continue/stop signal.
///
/// THIS ASKS THE ONE PREDICATE. Never a hand-rolled working/terminal test —
/// a hand-rolled spelling once omitted BOTH archival clauses and the board's
/// agent policy, so a person archiving the ticket, archiving its board, or
/// revoking the agent's grant did not stop the live session already running.
/// `agent_ticket_refusal` answers authority, `working_keys` answers "still in
/// play", assignment is the one thing left that is genuinely local to a
/// session — and `stop` carries the reason so the activity line says WHY the
/// session ended instead of just naming a column.
///
/// IT IS ASKED ON EVERY TURN, and durability makes that MORE load-bearing
/// rather than less: a resumed session has been away for as long as a deploy
/// plus a reclaim, so the world it left is not the world it wakes up in. It
/// also hands back the TICKET it asked about, so the prompt for that turn is
/// built from the same read the authority came from and cannot describe a
/// ticket that has since moved.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub task: tasks::Task,
    pub stop: Option<String>,
}

// ── The deps ─────────────────────────────────────────────────────────────────

/// One turn's outcome: the reply and its guard findings.
#[derive(Debug, Clone)]
pub struct TurnOutput {
    pub text: String,
    pub findings: Vec<crate::gateway::guard::Finding>,
}

pub type SessionStateFn = Arc<
    dyn Fn(PgPool, String, String) -> BoxFuture<'static, Result<Option<SessionState>, String>>
        + Send
        + Sync,
>;
pub type BoardHintFn =
    Arc<dyn Fn(PgPool, String) -> BoxFuture<'static, Result<BoardHint, String>> + Send + Sync>;
pub type WorkflowsFn = Arc<
    dyn Fn(
            PgPool,
            tasks::Task,
        ) -> BoxFuture<'static, Result<Vec<workflows::WorkflowDelivery>, String>>
        + Send
        + Sync,
>;
pub type SkillNamesFn = Arc<
    dyn Fn(PgPool, String) -> BoxFuture<'static, Result<HashSet<String>, String>> + Send + Sync,
>;
/// ONE TURN = ONE HARNESS RUN. Err when the turn produced nothing usable.
pub type TurnFn = Arc<
    dyn Fn(AppState, String, String, String) -> BoxFuture<'static, Result<TurnOutput, String>>
        + Send
        + Sync,
>;
pub type LogActivityFn = Arc<
    dyn Fn(PgPool, String, String, String, String) -> BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;
pub type RecentActivityFn = Arc<
    dyn Fn(PgPool, String) -> BoxFuture<'static, Result<Vec<tasks::TaskActivity>, String>>
        + Send
        + Sync,
>;
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The two destinations the dispatch prompt may name. From `status_meta`, so
/// they come from `placeable` like every other destination in the product.
#[derive(Debug, Clone, Default)]
pub struct BoardHint {
    pub active_key: Option<String>,
    pub assigned_key: Option<String>,
}

/// The session's edges, each overridable, so tests drive whole sessions —
/// including a reclaim — with no database, no Redis and no model.
#[derive(Clone)]
pub struct WorkSessionDeps {
    pub state: AppState,
    pub session_state: SessionStateFn,
    pub board_hint: BoardHintFn,
    pub workflows_for_task: WorkflowsFn,
    /// Skill names this agent can actually load: the shared root + its own.
    pub skill_names: SkillNamesFn,
    pub turn: TurnFn,
    pub log_activity: LogActivityFn,
    /// The ticket's recent activity, read ONLY to keep a line from landing
    /// twice after a driver died between writing it and checkpointing that
    /// it had.
    pub recent_activity: RecentActivityFn,
    pub now: NowFn,
}

fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The real edges. `state` carries the pool every dep reads and the harness
/// assembly the turn runs through.
pub fn real_work_session_deps(state: AppState) -> WorkSessionDeps {
    WorkSessionDeps {
        state: state.clone(),
        session_state: Arc::new(|pg, task_id, agent_model| {
            Box::pin(real_session_state(pg, task_id, agent_model))
        }),
        board_hint: Arc::new(|pg, board_id| Box::pin(real_board_hint(pg, board_id))),
        workflows_for_task: Arc::new(|pg, task| {
            Box::pin(async move {
                let target = workflows::MatchTarget {
                    title: &task.title,
                    description: task.description.as_deref(),
                    tags: &task.tags,
                    board_id: &task.board_id,
                };
                workflows::workflows_for_task(&pg, &target)
                    .await
                    .map_err(|e| e.to_string())
            })
        }),
        skill_names: Arc::new(|pg, agent_model| Box::pin(real_skill_names(pg, agent_model))),
        turn: Arc::new(|state, agent_model, task_id, prompt| {
            Box::pin(real_turn(state, agent_model, task_id, prompt))
        }),
        log_activity: Arc::new(|pg, task_id, actor, kind, description| {
            Box::pin(async move {
                tasks::log_activity(&pg, &task_id, &actor, &kind, &description)
                    .await
                    .map_err(|e| e.to_string())
            })
        }),
        recent_activity: Arc::new(|pg, task_id| {
            Box::pin(async move {
                tasks::list_activity(&pg, &task_id)
                    .await
                    .map_err(|e| e.to_string())
            })
        }),
        now: Arc::new(wall_ms),
    }
}

/// Authority + "still in play" + the live ticket, in one read.
async fn real_session_state(
    pg: PgPool,
    task_id: String,
    agent_model: String,
) -> Result<Option<SessionState>, String> {
    let Some(task) = tasks::get_task(&pg, &task_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let target = tasks::AgentWriteTarget {
        board_id: task.board_id.clone(),
        status: task.status.clone(),
        archived_at: task.archived_at.clone(),
    };
    let subject = crate::agent_auth::AgentSubject::Model(agent_model.clone());
    if let Some(stop) =
        tasks::agent_ticket_refusal(&pg, &target, &subject, tasks::AgentIntent::Write)
            .await
            .map_err(|e| e.to_string())?
    {
        return Ok(Some(SessionState {
            task,
            stop: Some(stop),
        }));
    }
    if !task.assignees.contains(&agent_model) {
        return Ok(Some(SessionState {
            task,
            stop: Some("no longer assigned to this agent".into()),
        }));
    }
    let meta = statuses::status_meta(&pg, &task.board_id)
        .await
        .map_err(|e| e.to_string())?;
    if !meta.working_keys.contains(&task.status) {
        return Ok(Some(SessionState {
            stop: Some(format!("ticket moved to \"{}\"", task.status)),
            task,
        }));
    }
    Ok(Some(SessionState { task, stop: None }))
}

async fn real_board_hint(pg: PgPool, board_id: String) -> Result<BoardHint, String> {
    let meta = statuses::status_meta(&pg, &board_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(BoardHint {
        active_key: meta.active_key,
        assigned_key: meta.assigned_key,
    })
}

/// Skill NAMES the agent can load: directories under the shared fleet root
/// and the agent's own, matching the skill-name grammar. Names only — the
/// other half of the skills plane (SKILL.md summaries, queued regeneration)
/// is the skills ROUTE family's and deliberately not dragged in here: the
/// session asks one question, "which skills exist", and a name-only read
/// answers it without firing the summarizer.
static SKILL_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^[a-z0-9][a-z0-9._-]*$").unwrap());

async fn real_skill_names(pg: PgPool, agent_model: String) -> Result<HashSet<String>, String> {
    let slug: Option<String> =
        sqlx::query_scalar("select slug from agent_defs where model = $1 limit 1")
            .bind(&agent_model)
            .fetch_optional(&pg)
            .await
            .map_err(|e| e.to_string())?;
    let fleet = crate::gateway::provider::fleet_dir();
    let mut names = HashSet::new();
    add_skill_dir_names(&fleet.join("skills"), &mut names).await;
    if let Some(slug) = slug {
        add_skill_dir_names(&fleet.join("agents").join(slug).join("skills"), &mut names).await;
    }
    Ok(names)
}

async fn add_skill_dir_names(root: &std::path::Path, names: &mut HashSet<String>) {
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return; // no root yet is no skills, not an error
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(file_type) = entry.file_type().await
            && file_type.is_dir()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if SKILL_NAME_RE.is_match(&name) {
                names.insert(name);
            }
        }
    }
}

/// ONE TURN = ONE HARNESS RUN, through the work-session harness with the
/// agent's own model.
///
/// THE LEDGER LINE IS THE ONE THING ONLY THIS CALL CAN SAY. The ledger row
/// writes `task_id`, and a ticket's cost is summed by that column alone — so
/// without `task_id` here a session's spend lands in the ledger and never in
/// the number the ticket's owner reads. `source` stays 'chat' deliberately:
/// 'ticket' rows are agent-SELF-REPORTED through MCP `log_usage`, and this
/// turn is metered by Talaria on its own request path.
///
/// ABORT, FROM OUTSIDE. The harness runner takes no signal, so the driver
/// enforces the outcome from outside — it races the step against the step
/// budget and the lease, and DROPPING the step future cancels the request
/// underneath it. Either way the turn never completes without checkpointing,
/// and the next entry retires it by `stage_attempt`.
async fn real_turn(
    state: AppState,
    agent_model: String,
    task_id: String,
    prompt: String,
) -> Result<TurnOutput, String> {
    let run = run_harness(
        &state,
        &work_session_harness(),
        &json!({ "prompt": prompt }),
        RunContext {
            caller: format!("ticket:{task_id}"),
            user_id: None,
            model: Some(agent_model.clone()),
            step: None,
            tier: None,
            effort: None,
            ledger: Some(RunLedger {
                source: Some(LedgerSource::Chat),
                ref_id: Some(task_id.clone()),
                task_id: Some(task_id.clone()),
            }),
            deps: None,
        },
    )
    .await
    .map_err(|e| e.0)?;
    let Some(text) = run.value.and_then(|v| v.as_str().map(str::to_string)) else {
        return Err(run
            .error
            .unwrap_or_else(|| format!("no reply from {agent_model}")));
    };
    Ok(TurnOutput {
        text,
        findings: run.findings,
    })
}

// ── The activity trail ───────────────────────────────────────────────────────

/// Guard findings, on the line a HUMAN reads.
///
/// The harness already writes them to `guard_findings`, which is where the
/// fitness page reads a per-model confabulation rate — but nobody reviewing a
/// flagged ticket opens that table. A turn flagged `zero_tool_claim` is the
/// exact thing the reviewer signing that ticket off needs to see, so the
/// check ids ride on the activity line, IN FRONT of the reply so a bounded
/// slice can never truncate them away.
///
/// IDS ONLY — never `message` and above all never `snippet`, which is a
/// verbatim excerpt of the flagged text. And this never travels back to the
/// agent: the next turn's prompt is built from the ticket's state, not from
/// this line. Feeding a finding back would break guardrails' cardinal
/// invariant and teach the agent to argue with the guard instead of to call
/// a tool.
fn noted(line: &str, checks: &[String]) -> String {
    if checks.is_empty() {
        return line.to_string();
    }
    // Deduped preserving first-seen order.
    let mut seen: HashSet<&str> = HashSet::new();
    let mut ids: Vec<&str> = Vec::new();
    for c in checks {
        if seen.insert(c.as_str()) {
            ids.push(c.as_str());
        }
    }
    format!("[guard: {}] {line}", ids.join(", "))
}

/// Write one line of the session's trail.
///
/// A LOST LINE MUST NOT DESTROY THE SESSION. The trail is the RECORD of the
/// work, not the work: a database blip while writing "session turn 4" is not
/// a reason to abandon a ticket an agent is halfway through, and under the
/// runtime a throw here would do exactly that — the step fails, the run is
/// filed as an error, and the eleven remaining turns never happen. So a
/// failed write is loud in the log and invisible to the step.
///
/// `dedupe` is the at-least-once guard, and it is asked for ONLY on the path
/// where it can be true: a driver died between this write landing and the
/// checkpoint that records it, so the step is re-entered and would say the
/// same thing twice. Every line this file writes names its turn number or
/// its reason, so an exact (actor, type='dispatch', description) match on
/// the ticket's recent history is that line and not a coincidence.
async fn say(
    d: &WorkSessionDeps,
    pg: &PgPool,
    task_id: &str,
    actor: &str,
    description: &str,
    dedupe: bool,
) {
    let res = async {
        if dedupe {
            let recent = (d.recent_activity)(pg.clone(), task_id.to_string()).await?;
            if recent.iter().any(|a| {
                a.actor == actor && a.kind_tag == "dispatch" && a.description == description
            }) {
                tracing::warn!(
                    "{LOG} {task_id}: \"{}\" is already on the ticket — a driver died after \
                     writing it, not before",
                    utf16_prefix(description, 80)
                );
                return Ok(());
            }
        }
        (d.log_activity)(
            pg.clone(),
            task_id.to_string(),
            actor.to_string(),
            "dispatch".to_string(),
            description.to_string(),
        )
        .await
    }
    .await;
    if let Err(e) = res {
        tracing::error!(
            "{LOG} {task_id}: could not record \"{}\" on the ticket: {e}",
            utf16_prefix(description, 80)
        );
    }
}

// ── The prompts ──────────────────────────────────────────────────────────────

/// The workflow block, and the gap signal that rides with it.
///
/// Workflows name SKILLS — the agent loads the flow content from the skill
/// mounts it already reads; flow prose is never pasted into the prompt.
/// Skills the target agent can't see are flagged, not silently named (the
/// future gap loop starts from exactly this signal).
async fn workflow_context(
    d: &WorkSessionDeps,
    pg: &PgPool,
    task: &tasks::Task,
    agent_model: &str,
) -> Result<(String, String), String> {
    let flows = (d.workflows_for_task)(pg.clone(), task.clone()).await?;
    // jsonb passthrough: an absent/odd `skills` value is no skills, not a
    // crashed session. (The column is written by the workflow CRUD as an
    // array, so the tolerant read is theoretical — and the kinder reading is
    // the one that keeps an agent working.)
    let skills_of = |w: &workflows::WorkflowDelivery| -> Vec<String> {
        w.skills
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let available = if flows.iter().any(|w| !skills_of(w).is_empty()) {
        (d.skill_names)(pg.clone(), agent_model.to_string()).await?
    } else {
        HashSet::new()
    };
    let missing: Vec<String> = flows
        .iter()
        .flat_map(|w| {
            skills_of(w)
                .into_iter()
                .filter(|sk| !available.contains(sk))
        })
        .collect();

    let block = if flows.is_empty() {
        String::new()
    } else {
        let plural = if flows.len() > 1 { "s" } else { "" };
        let mut block = format!("\n\nHOW THIS KIND OF WORK IS DONE HERE (workflow{plural}):\n");
        let parts: Vec<String> = flows
            .iter()
            .map(|w| {
                let skills = skills_of(w);
                let usable: Vec<&String> =
                    skills.iter().filter(|sk| available.contains(*sk)).collect();
                let flow = if !usable.is_empty() {
                    let plural = if usable.len() > 1 { "s" } else { "" };
                    let list = usable
                        .iter()
                        .map(|sk| format!("\"{}\"", sk))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("Load and follow your skill{plural}: {list}.")
                } else {
                    "Use your judgment — no specific skill is bound to this workflow.".to_string()
                };
                let kits = kits_line(&w.toolkits);
                format!("── {} ──\n{flow}{kits}", w.name)
            })
            .collect();
        block.push_str(&parts.join("\n\n"));
        block
    };

    let mut line = format!("work pushed to {agent_model}");
    if !flows.is_empty() {
        let names = flows
            .iter()
            .map(|w| {
                let skills = skills_of(w);
                if skills.is_empty() {
                    w.name.clone()
                } else {
                    format!("{} [{}]", w.name, skills.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        line.push_str(&format!(" with workflow {names}"));
    }
    if !missing.is_empty() {
        let list = missing
            .iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(", ");
        line.push_str(&format!(" — skill {list} not available to this agent"));
    }
    Ok((block, line))
}

/// The `Expected toolkits:` clause — `{server}` plus its named tools, one
/// entry per row, empty when the workflow named none.
fn kits_line(toolkits: &Value) -> String {
    let Some(rows) = toolkits.as_array() else {
        return String::new();
    };
    let parts: Vec<String> = rows
        .iter()
        .filter_map(|t| {
            let server = t.get("server")?.as_str()?;
            let tools: Vec<&str> = t
                .get("tools")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            Some(if tools.is_empty() {
                server.to_string()
            } else {
                format!("{server} ({})", tools.join(", "))
            })
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("\nExpected toolkits: {}", parts.join("; "))
    }
}

/// The dispatch brief — turn one, and the only turn whose prompt describes
/// the ticket rather than pointing at it. The TEMPLATE is the harness's
/// `dispatch_prompt`; this side supplies what only the session knows: the
/// board hint, the workflow block, and step 2's status instruction.
async fn dispatch_prompt(
    d: &WorkSessionDeps,
    pg: &PgPool,
    task: &tasks::Task,
    agent_model: &str,
    board_name: Option<&str>,
) -> Result<String, String> {
    let hint = (d.board_hint)(pg.clone(), task.board_id.clone()).await?;
    let (block, _) = workflow_context(d, pg, task, agent_model).await?;
    // THE PROMPT HANDS THIS TO THE AGENT VERBATIM, so it is a destination
    // and must come from `placeable` like every other one. It used to be the
    // first active column by category, which does not exclude terminal
    // columns: on a board whose first active column is labelled "Cancelled"
    // the hint was "cancelled", so an agent obeying step 2 of its own
    // work-session prompt sent a TERMINAL move. `active_key` is picked from
    // `placeable`; with no active column at all we fall back to the pickup
    // queue; with neither we say so rather than invent a key the board may
    // not have.
    let active_hint = hint.active_key.or(hint.assigned_key);
    let step2 = match active_hint {
        Some(key) => format!(
            "comment a one-line acknowledgment in the ticket's discussion thread (the board's \
             humans read it), and triage_ticket to status \"{key}\" while you work."
        ),
        None => "comment a one-line acknowledgment in the ticket's discussion thread (the board's \
                 humans read it). Leave the status where it is — this board has no working column \
                 for you to move it to."
            .to_string(),
    };
    Ok(dispatch_brief(
        &crate::harness::defs::work_session::DispatchPromptInput {
            task_id: &task.id,
            ticket_ref: task.ticket_ref.as_deref().unwrap_or(&task.id),
            title: &task.title,
            description: task.description.as_deref(),
            board_name,
            workflow_block: &block,
            step2: &step2,
        },
    ))
}

/// The agent says it's finished but the ticket disagrees — one nudge to
/// reconcile (report_outcome / set blocked), then stop pushing.
fn reconcile_prompt(status: &str) -> String {
    format!(
        "[Work session — reconcile] You said DONE/BLOCKED but the ticket is still \"{status}\". \
         If finished: report_outcome now. If blocked: set status \"blocked\" with a comment. If \
         neither, keep working."
    )
}

/// Every continuation carries the LIVE ticket state, so the agent never works
/// a stale picture — which is also what makes a resumed session safe to
/// continue: the prompt is built from the ticket as it is now, not as it was
/// before the deploy.
fn continue_prompt(turn: i64, status: &str) -> String {
    format!(
        "[Work session — turn {turn}/{MAX_SESSION_TURNS}] You're mid-work on this ticket (status: \
         \"{status}\"). Continue like a developer: next step, run it, read the result, adjust. \
         Verify before you finish — tests, your own diff, and for UI work drive it in a real \
         browser (Playwright) and attach evidence. When genuinely done: report_outcome. If stuck: \
         status \"blocked\" + comment. End with your status line."
    )
}

// ── UTF-16 slices, the JS way ────────────────────────────────────────────────

/// `s.slice(0, n)` — UTF-16 units. JS can split a surrogate pair at the cut;
/// a Rust string cannot hold half a pair, so the cut backs off to the last
/// whole character that fits (the house precedent is `to_blurb`'s clamp).
fn utf16_prefix(s: &str, n: usize) -> &str {
    let mut units = 0usize;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}

/// `s.slice(-n)` — the LAST n UTF-16 units, same whole-character rule.
fn utf16_suffix(s: &str, n: usize) -> &str {
    let mut units = 0usize;
    for (i, c) in s.char_indices().rev() {
        units += c.len_utf16();
        if units > n {
            return &s[i + c.len_utf8()..];
        }
    }
    s
}

/// `turn - 1` turns are behind us. The plural is spelled out rather than a
/// `> 2` test because this line is also reachable at turn 1 — a run parked,
/// reclaimed or simply slow enough that the world changed between the
/// enqueue and the first prompt.
fn turns_so_far(turn: i64) -> String {
    let behind = turn - 1;
    let plural = if behind == 1 { "" } else { "s" };
    format!("{behind} turn{plural}")
}

// ── The step ─────────────────────────────────────────────────────────────────

/// ONE UNIT OF PROGRESS, on its own deps so a test can drive a whole session
/// — including a reclaim — with no database, no Redis and no model.
pub async fn work_session_step(
    ctx: RunStepContext,
    d: &WorkSessionDeps,
) -> Result<StepResult, String> {
    // OUTSIDE the machine, deliberately. Every other failure below is told
    // to the ticket before the run is filed as an error; a row whose input
    // or checkpoint this deploy cannot read is the one failure with nowhere
    // to say it — we do not know which ticket it is about. It goes straight
    // out to the driver, which writes the message onto the run.
    let input = as_input(&ctx.input)?;
    let cp = as_checkpoint(&ctx.checkpoint)?;
    let task_id = input.task_id.clone();
    let agent_model = input.agent_model.clone();
    let pg = d.state.pg.clone();
    let attempt = ctx.attempt as i64;

    let machine = async {
        // ── The failure tail ─────────────────────────────────────────────────
        // Last, so the ticket learns the session failed even though the run
        // is about to be filed as an error. Its own line is deduped for the
        // same reason every other one is: the throw below is what makes the
        // run terminal, and a driver that died between the line and the
        // throw re-enters here.
        if let Some(WorkSessionCheckpoint::Failed {
            stage_attempt,
            error,
            ..
        }) = &cp
        {
            say(
                d,
                &pg,
                &task_id,
                "talaria",
                &format!(
                    "work session with {agent_model} failed: {}",
                    utf16_prefix(error, 200)
                ),
                attempt > *stage_attempt,
            )
            .await;
            return Err(error.clone());
        }

        // ── Record: the reply is already durable, put it on the ticket ──────
        if let Some(WorkSessionCheckpoint::Record {
            turn,
            stage_attempt,
            said,
            reply,
        }) = &cp
        {
            let dedupe = attempt > *stage_attempt;
            match reply {
                None => {
                    // THE RETIRED TURN. The `send` step found that a driver
                    // had died while this turn was in flight, so we do not
                    // know whether the agent was asked — and did not ask
                    // again. Saying so on the ticket is the whole point: the
                    // pre-durability failure mode was a session that simply
                    // stopped, with the ticket still reading "work pushed to
                    // <agent>" and nothing else.
                    say(
                        d,
                        &pg,
                        &task_id,
                        "talaria",
                        &format!(
                            "turn {turn} was interrupted by a restart — its outcome is unknown; \
                             continuing from the ticket's current state"
                        ),
                        dedupe,
                    )
                    .await;
                }
                Some(reply) => {
                    let line = match said {
                        TurnKind::Dispatch => {
                            format!("picked up: {}", prefix_or(reply, 300, "(no reply)"))
                        }
                        TurnKind::Reconcile => {
                            format!("session reconcile: {}", prefix_or(reply, 200, "(no reply)"))
                        }
                        TurnKind::Turn => {
                            format!(
                                "session turn {turn}: {}",
                                prefix_or(reply, 250, "(no reply)")
                            )
                        }
                    };
                    say(
                        d,
                        &pg,
                        &task_id,
                        &agent_model,
                        &noted(&line, &reply.checks),
                        dedupe,
                    )
                    .await;
                }
            }
            return Ok(StepResult::Next {
                checkpoint: json!(WorkSessionCheckpoint::Send {
                    turn: turn + 1,
                    stage_attempt: attempt,
                    last_tail: reply.as_ref().map(|r| r.tail.clone()).unwrap_or_default(),
                    not_before: reply
                        .is_none()
                        .then(|| (d.now)() + INTERRUPTED_TURN_SETTLE_MS),
                }),
                phase: Some(format!("turn {turn} recorded")),
            });
        }

        // ── The brief: nothing has been said on this ticket yet ─────────────
        let Some(WorkSessionCheckpoint::Send {
            turn,
            stage_attempt,
            last_tail,
            not_before,
        }) = &cp
        else {
            // cp === null: the brief.
            let state = (d.session_state)(pg.clone(), task_id.clone(), agent_model.clone()).await?;
            // REFUSED BEFORE A WORD IS WRITTEN, and silently, exactly as
            // `maybe_dispatch_ticket` refuses: nothing was pushed, so there
            // is nothing to explain on the ticket. The gap this closes is
            // new to the durable run — a row waits in the queue, and the
            // ticket can be archived, closed or taken away from the agent
            // between the enqueue and the first step.
            let Some(state) = state else {
                return Ok(StepResult::Done {
                    result: json!({ "turns": 0, "ended": "ticket gone", "dispatched": false }),
                });
            };
            if let Some(stop) = &state.stop {
                return Ok(StepResult::Done {
                    result: json!({ "turns": 0, "ended": stop, "dispatched": false }),
                });
            }
            let (_, line) = workflow_context(d, &pg, &state.task, &agent_model).await?;
            // The lifecycle is auditable from the ticket: every step lands
            // in task_activity (dispatch start w/ matched workflows +
            // skills, the agent's reply or the failure, then the agent's own
            // actions).
            say(d, &pg, &task_id, "talaria", &line, attempt > 0).await;
            return Ok(StepResult::Next {
                checkpoint: json!(WorkSessionCheckpoint::Send {
                    turn: 1,
                    stage_attempt: attempt,
                    last_tail: String::new(),
                    not_before: None,
                }),
                phase: Some(format!("pushed to {agent_model}")),
            });
        };

        // ── Send: the one step that spends money ────────────────────────────
        // The cap is checked BEFORE authority: turn 12 was the last one sent,
        // and the cap line is written without asking the ticket anything.
        if *turn > MAX_SESSION_TURNS {
            say(
                d,
                &pg,
                &task_id,
                "talaria",
                &format!(
                    "work session hit the {MAX_SESSION_TURNS}-turn cap — leaving the ticket to the \
                     agent/heartbeat"
                ),
                attempt > *stage_attempt,
            )
            .await;
            return Ok(StepResult::Done {
                result: json!({ "turns": MAX_SESSION_TURNS, "ended": "turn cap" }),
            });
        }

        // A SOFT pause, not a wait inside the step: nobody is notified, no
        // attempt is consumed, and the run's own lease holds the gap. See
        // INTERRUPTED_TURN_SETTLE_MS. The condition clears itself — once the
        // clock passes `not_before` this branch is simply false — so the
        // deferral cannot become a loop.
        if let Some(not_before) = not_before
            && (d.now)() < *not_before
        {
            return Ok(StepResult::Retry {
                after: std::time::Duration::from_millis((not_before - (d.now)()) as u64),
                reason: format!("letting the interrupted turn settle before turn {turn}"),
            });
        }

        // AUTHORITY, ON EVERY TURN, BEFORE ANYTHING ELSE. A person archiving
        // the ticket, archiving its board, or revoking the agent's grant
        // stops the session mid-flight — and for a resumed run this is the
        // FIRST thing that happens, because the world may have changed
        // entirely while it was parked.
        let state = (d.session_state)(pg.clone(), task_id.clone(), agent_model.clone()).await?;
        if let Some(stop) = state.as_ref().and_then(|s| s.stop.clone()) {
            say(
                d,
                &pg,
                &task_id,
                "talaria",
                &format!("work session ended after {} — {stop}", turns_so_far(*turn)),
                attempt > *stage_attempt,
            )
            .await;
            return Ok(StepResult::Done {
                result: json!({ "turns": turn - 1, "ended": stop }),
            });
        }
        let Some(state) = state else {
            say(
                d,
                &pg,
                &task_id,
                "talaria",
                &format!(
                    "work session ended after {} — ticket gone",
                    turns_so_far(*turn)
                ),
                attempt > *stage_attempt,
            )
            .await;
            return Ok(StepResult::Done {
                result: json!({ "turns": turn - 1, "ended": "ticket gone" }),
            });
        };

        // ── The interrupted turn ─────────────────────────────────────────────
        // `stage_attempt` was the attempt when this stage was written. A
        // higher one now means a driver was reclaimed while this exact turn
        // was owed — so the prompt may already be with the agent, and
        // re-sending it would put two turns of one session against one
        // ticket. Retire it instead: the ticket gets a line saying so, the
        // next turn waits for anything in flight to settle, and the turn cap
        // counts it, because it cost a turn either way.
        if attempt > *stage_attempt {
            tracing::warn!(
                "{LOG} {task_id} ({agent_model}): turn {turn} was owed when a driver died at \
                 attempt {stage_attempt} — not re-sending it. The model may have been called and \
                 the agent may have used its tools; a second prompt would be a second agent on \
                 this ticket."
            );
            return Ok(StepResult::Next {
                checkpoint: json!(WorkSessionCheckpoint::Record {
                    turn: *turn,
                    stage_attempt: attempt,
                    said: if *turn == 1 {
                        TurnKind::Dispatch
                    } else {
                        TurnKind::Turn
                    },
                    reply: None,
                }),
                phase: Some(format!("turn {turn} interrupted")),
            });
        }

        let kind = if *turn == 1 {
            TurnKind::Dispatch
        } else if DONE_OR_BLOCKED.is_match(last_tail) {
            TurnKind::Reconcile
        } else {
            TurnKind::Turn
        };
        let prompt = match kind {
            TurnKind::Dispatch => {
                dispatch_prompt(
                    d,
                    &pg,
                    &state.task,
                    &agent_model,
                    input.board_name.as_deref(),
                )
                .await?
            }
            TurnKind::Reconcile => reconcile_prompt(&state.task.status),
            TurnKind::Turn => continue_prompt(*turn, &state.task.status),
        };

        // The last gate before the money: the driver aborts when the step
        // blows its budget or the lease moves, and a step that called anyway
        // would be the doubled side effect the runtime exists to prevent.
        if ctx.signal.is_aborted() {
            return Err(format!("the run was interrupted before turn {turn}"));
        }
        (ctx.log)(format!(
            "turn {turn} of {MAX_SESSION_TURNS} — waiting on {agent_model}"
        ));

        let reply = match (d.turn)(
            d.state.clone(),
            agent_model.clone(),
            task_id.clone(),
            prompt,
        )
        .await
        {
            Ok(reply) => reply,
            Err(e) => {
                // An abort is the DRIVER's outcome, not the session's.
                if ctx.signal.is_aborted() {
                    return Err(e);
                }
                // A turn that produced nothing usable ends the session with a
                // logged failure instead of driving eleven more turns off a
                // blank. It goes through a checkpoint rather than throwing
                // here so the ticket's failure line is written by a step with
                // nothing else in it: a throw from HERE would re-enter this
                // step on the next driver and re-send the prompt.
                return Ok(StepResult::Next {
                    checkpoint: json!(WorkSessionCheckpoint::Failed {
                        turn: *turn,
                        stage_attempt: attempt,
                        error: e,
                    }),
                    phase: Some(format!("turn {turn} failed")),
                });
            }
        };

        // NOTHING BETWEEN THE MODEL AND THE CHECKPOINT. Every use of this
        // reply — the activity line, the DONE/BLOCKED test — happens in the
        // next step, off the persisted copy. The window in which a crash
        // costs a second billed turn is this return and one database write,
        // and it cannot be made smaller from here.
        Ok(StepResult::Next {
            checkpoint: json!(WorkSessionCheckpoint::Record {
                turn: *turn,
                stage_attempt: attempt,
                said: kind,
                reply: Some(TurnReply {
                    head: utf16_prefix(&reply.text, 300).to_string(),
                    tail: utf16_suffix(&reply.text, 200).to_string(),
                    checks: {
                        let mut seen = HashSet::new();
                        reply
                            .findings
                            .iter()
                            .filter(|f| seen.insert(f.check.clone()))
                            .map(|f| f.check.to_string())
                            .collect()
                    },
                }),
            }),
            phase: Some(format!("turn {turn} answered")),
        })
    };

    match machine.await {
        Ok(result) => Ok(result),
        Err(e) => {
            // An abort is the DRIVER's outcome, not the session's: it has
            // already rejected its own race and decided whether this was a
            // lost lease or a blown budget. Re-filing it as a failed session
            // would put a "work session failed" line on a ticket whose
            // session is alive on another instance.
            if ctx.signal.is_aborted() {
                return Err(e);
            }
            // Anything else — a board read that threw, a workflow lookup that
            // died — is the session failing, and the ticket is owed the
            // sentence. One more step, so the line is written by something
            // that cannot repeat a model call.
            if matches!(cp, Some(WorkSessionCheckpoint::Failed { .. })) {
                return Err(e);
            }
            tracing::error!("{LOG} {task_id} ({agent_model}) step threw: {e}");
            Ok(StepResult::Next {
                checkpoint: json!(WorkSessionCheckpoint::Failed {
                    turn: cp
                        .as_ref()
                        .map(|c| match c {
                            WorkSessionCheckpoint::Send { turn, .. }
                            | WorkSessionCheckpoint::Record { turn, .. }
                            | WorkSessionCheckpoint::Failed { turn, .. } => *turn,
                        })
                        .unwrap_or(1),
                    stage_attempt: attempt,
                    error: e,
                }),
                phase: Some("session failed".into()),
            })
        }
    }
}

static DONE_OR_BLOCKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(DONE|BLOCKED)\b").unwrap());

/// The record stage's reply slices: `head.slice(n) || '(no reply)'` — an
/// empty slice is the parenthetical, not an empty line.
fn prefix_or(reply: &TurnReply, n: usize, fallback: &str) -> String {
    let cut = utf16_prefix(&reply.head, n);
    if cut.is_empty() {
        fallback.to_string()
    } else {
        cut.to_string()
    }
}

// ── The definition ───────────────────────────────────────────────────────────

/// The board's editors, which is who the ticket routes every other decision
/// to. Never the run's owner (there isn't one) and never the agent: a session
/// that could name its own audience would be an agent deciding who supervises
/// it. `nobody` when the input cannot name a board, which is a real stall to
/// report rather than a reason to widen.
fn audience(run: &RunRow) -> Authority {
    match run
        .input
        .get("boardId")
        .and_then(|v| v.as_str())
        .filter(|b| !b.is_empty())
    {
        Some(board_id) => Authority::Board {
            board_id: board_id.to_string(),
        },
        None => Authority::Nobody,
    }
}

/// The real step deps. They need the AppState, which the boot wiring owns, so
/// they are installed separately from registration; an unarmed step is the
/// loud refusal below — reached only by a driver armed before its deps,
/// which is a wiring bug.
static ARMED_DEPS: OnceLock<WorkSessionDeps> = OnceLock::new();

pub fn arm_work_session_step(deps: WorkSessionDeps) {
    let _ = ARMED_DEPS.set(deps);
}

/// The registered definition, exactly once per process — registered on the
/// first call, which `dispatch` makes before any enqueue, so the row's kind
/// is always registered before it is written. The returned `&'static Arc` is
/// the same one the registry holds.
pub fn work_session_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: WORK_SESSION_KIND.into(),
            label: "Agent work session".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(deps) = ARMED_DEPS.get().cloned() else {
                        return Err(
                            "work-session steps are armed by the scheduler's boot wiring; this \
                             step was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    work_session_step(ctx, &deps).await
                })
            }),
            audience: Arc::new(audience),
            // ELEVEN MINUTES, because one step is one turn and one turn is
            // one call to the work-session harness, whose hold is ten: an
            // agent restarting under a config propagation refuses
            // connections for tens of seconds and a fleet re-render
            // mid-session must not kill the session. This is also the lease
            // TTL, so a session whose instance died is reclaimable about
            // eleven minutes later — the right answer, not an unfortunate
            // one: coming back sooner would reclaim turns that are
            // genuinely still running.
            max_step_ms: 660_000,
            // FIVE, against the default three, and the reason is duration.
            // `attempt` counts drivers that DIED holding this run, and a
            // work session is the longest-lived run in the product — up to
            // twelve model turns of up to ten minutes each — so it is the
            // one kind that routinely spans more than one deploy. Three
            // would file a healthy session as an error for the crime of a
            // busy release day. The count is still bounded, and it is
            // self-limiting in a way no other kind's is: every reclaim
            // retires the turn it interrupted, so a session that keeps
            // killing drivers spends its turn budget doing it.
            max_attempts: 5,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Fixed vectors from the original derivation — not re-derived here,
    // which would test the derivation against itself.
    #[test]
    fn session_run_id_is_the_pinned_derivation() {
        assert_eq!(
            session_run_id("11111111-1111-1111-1111-111111111111", "claude-opus-4-5", 0),
            "b1715287-c692-8f1e-9255-ea7ba786880b"
        );
        assert_eq!(
            session_run_id("11111111-1111-1111-1111-111111111111", "claude-opus-4-5", 1),
            "a3cd749a-6cbe-87dd-a6f7-1495296efdce"
        );
        // The generation is in the name: same pair, different session.
        assert_ne!(
            session_run_id("11111111-1111-1111-1111-111111111111", "claude-opus-4-5", 0),
            session_run_id("11111111-1111-1111-1111-111111111111", "claude-opus-4-5", 1)
        );
        assert_eq!(
            session_run_id("22222222-2222-2222-2222-222222222222", "gpt-5.3", 24),
            "3c589dd9-691b-8cc5-9e8b-03a4d26b9675"
        );
        assert_eq!(
            session_run_id("11111111-1111-1111-1111-111111111111", "gpt-5.3", 0),
            "1a25157f-29df-8201-a2ef-c381add3169e"
        );
    }

    #[test]
    fn the_id_is_a_version_8_uuid_shape() {
        let id = session_run_id("t", "a", 0);
        // 8-4-4-4-12 lowercase hex, version nibble 8, variant 10x.
        assert_eq!(id.len(), 36);
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert!(id.chars().all(|c| !c.is_ascii_uppercase()));
        assert!(parts[2].starts_with('8'));
        assert!(
            parts[3].starts_with('8')
                || parts[3].starts_with('9')
                || parts[3].starts_with('a')
                || parts[3].starts_with('b')
        );
    }

    #[test]
    fn registration_carries_the_real_metadata_once() {
        let def = work_session_run();
        assert_eq!(def.kind, WORK_SESSION_KIND);
        assert_eq!(def.label, "Agent work session");
        assert_eq!(def.max_step_ms, 660_000);
        assert_eq!(def.max_attempts, 5);
        // The same Arc every time — register_run is once per process, and a
        // second registration would be the bug define.rs refuses.
        assert!(Arc::ptr_eq(def, work_session_run()));
        // The audience reads the input's boardId, never the owner.
        let mut row = minimal_row();
        assert!(matches!(audience(&row), Authority::Nobody));
        row.input = serde_json::json!({"boardId": "b-1"});
        assert_eq!(
            audience(&row),
            Authority::Board {
                board_id: "b-1".into()
            }
        );
        // Truthiness: an empty boardId string is nobody's, not a board
        // named "".
        row.input = serde_json::json!({"boardId": ""});
        assert!(matches!(audience(&row), Authority::Nobody));
    }

    #[tokio::test]
    async fn the_unarmed_step_refuses_naming_the_wiring_bug() {
        let def = work_session_run();
        let err = (def.step)(row_ctx(json!(null), json!(null), 0))
            .await
            .unwrap_err();
        assert!(err.contains("scheduler"), "{err}");
        assert!(err.contains("armed"), "{err}");
    }

    // ── Checkpoint wire shape ───────────────────────────────────────────────

    #[test]
    fn checkpoints_round_trip_the_db_spellings() {
        // A send checkpoint, with and without notBefore.
        let send: WorkSessionCheckpoint = serde_json::from_value(
            json!({"stage":"send","turn":3,"stageAttempt":1,"lastTail":"will DONE soon"}),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&send).unwrap(),
            json!({"stage":"send","turn":3,"stageAttempt":1,"lastTail":"will DONE soon"})
        );
        let settled: WorkSessionCheckpoint =
            serde_json::from_value(json!({"stage":"send","turn":4,"stageAttempt":2,"lastTail":"","notBefore":1788045420000i64}))
                .unwrap();
        assert_eq!(
            serde_json::to_value(&settled).unwrap(),
            json!({"stage":"send","turn":4,"stageAttempt":2,"lastTail":"","notBefore":1788045420000i64})
        );
        // A record with a reply, and the retired turn (reply null).
        let record: WorkSessionCheckpoint = serde_json::from_value(json!({
            "stage":"record","turn":1,"stageAttempt":0,"said":"dispatch",
            "reply":{"head":"on it","tail":"on it","checks":[]}
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            json!({"stage":"record","turn":1,"stageAttempt":0,"said":"dispatch",
                   "reply":{"head":"on it","tail":"on it","checks":[]}})
        );
        let retired: WorkSessionCheckpoint = serde_json::from_value(json!({
            "stage":"record","turn":2,"stageAttempt":1,"said":"turn","reply":null
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&retired).unwrap(),
            json!({"stage":"record","turn":2,"stageAttempt":1,"said":"turn","reply":null})
        );
        // A failure tail.
        let failed: WorkSessionCheckpoint = serde_json::from_value(json!({
            "stage":"failed","turn":5,"stageAttempt":2,"error":"gateway completion 429"
        }))
        .unwrap();
        assert!(matches!(failed, WorkSessionCheckpoint::Failed { .. }));
        // The notBefore-less send serializes WITHOUT the key — a checkpoint
        // must not carry a null notBefore a reader would treat as a number.
        assert!(
            !serde_json::to_value(&send)
                .unwrap()
                .get("notBefore")
                .is_some()
        );
    }

    #[test]
    fn unreadable_input_or_checkpoint_is_the_drivers_error() {
        assert!(as_input(&json!(null)).is_err());
        assert!(as_input(&json!({"taskId":"t"})).is_err()); // no agentModel/boardId
        assert_eq!(
            as_input(&json!({"taskId":"t","agentModel":"a","boardId":"b"}))
                .unwrap()
                .generation,
            0
        ); // generation defaults to 0 when absent
        assert!(as_checkpoint(&json!(null)).unwrap().is_none());
        assert!(as_checkpoint(&json!({"stage":"nonsense"})).is_err());
    }

    // ── A fake world to drive whole sessions ────────────────────────────────
    //
    // No database, no Redis, no model: the step is driven on recording deps.
    // `state` is real but lazy — the pool dials nothing until a query runs,
    // and no fake dep ever runs one.

    fn test_state() -> AppState {
        use crate::config::Config;
        let url = "postgres://work-session-test@localhost:5432/work-session-test";
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy(url)
            .expect("a lazy pool connects to nothing");
        let cfg = Config::from_parts(
            url.into(),
            "redis://work-session-test@localhost:6379".into(),
            "test-root".into(),
            String::new(),
            String::new(),
            "0".into(),
        )
        .expect("the test config is valid on its face");
        AppState::new(pg, Arc::new(cfg))
    }

    fn task() -> tasks::Task {
        tasks::Task {
            id: "t-1".into(),
            board_id: "b-1".into(),
            ticket_ref: Some("TASK-1".into()),
            title: "Fix the thing".into(),
            description: Some("It is broken".into()),
            status: "in_progress".into(),
            priority: "medium".into(),
            effort: None,
            assignees: vec!["hermes".into()],
            created_by: "u-1".into(),
            due_date: None,
            start_date: None,
            color: None,
            tags: vec![],
            attachments: serde_json::Value::Null,
            time_spent_seconds: 0,
            estimated_hours: None,
            parent_id: None,
            comment_count: 0,
            outcome: None,
            resolution: None,
            error_message: None,
            created_at: String::new(),
            updated_at: String::new(),
            completed_at: None,
            archived_at: None,
        }
    }

    /// What the fake world saw: lines written, and the ticket's recent
    /// history as the dedupe path reads it (pre-populatable, so a re-entry
    /// can find its own prior line).
    #[derive(Clone)]
    struct Recorder {
        lines: Arc<Mutex<Vec<(String, String)>>>,
        recent: Arc<Mutex<Vec<tasks::TaskActivity>>>,
    }

    /// Deps with a live, assignable ticket and a scripted turn.
    fn recording_deps(turn_text: &'static str) -> (WorkSessionDeps, Recorder) {
        let lines: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let recent: Arc<Mutex<Vec<tasks::TaskActivity>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_for_log = lines.clone();
        let recent_for_read = recent.clone();
        let deps = WorkSessionDeps {
            state: test_state(),
            session_state: Arc::new(|_pg, _task, _agent| {
                Box::pin(async {
                    Ok(Some(SessionState {
                        task: task(),
                        stop: None,
                    }))
                })
            }),
            board_hint: Arc::new(|_pg, _board| {
                Box::pin(async {
                    Ok(BoardHint {
                        active_key: Some("in_progress".into()),
                        assigned_key: Some("todo".into()),
                    })
                })
            }),
            workflows_for_task: Arc::new(|_pg, _t| Box::pin(async { Ok(vec![]) })),
            skill_names: Arc::new(|_pg, _a| Box::pin(async { Ok(HashSet::new()) })),
            turn: Arc::new(move |_state, _agent, _task, _prompt| {
                Box::pin(async {
                    Ok(TurnOutput {
                        text: turn_text.to_string(),
                        findings: vec![],
                    })
                })
            }),
            log_activity: Arc::new(move |_pg, task_id, actor, _kind, description| {
                // An `Fn` dep cannot hand its capture to the future — clone
                // the Arc into it, the same way the real deps move their
                // owned arguments.
                let lines_for_log = lines_for_log.clone();
                Box::pin(async move {
                    lines_for_log
                        .lock()
                        .unwrap()
                        .push((actor, format!("{task_id}: {description}")));
                    Ok(())
                })
            }),
            recent_activity: Arc::new(move |_pg, _task_id| {
                let recent_for_read = recent_for_read.clone();
                Box::pin(async move { Ok(recent_for_read.lock().unwrap().clone()) })
            }),
            now: Arc::new(|| 1_788_045_420_000),
        };
        (deps, Recorder { lines, recent })
    }

    fn input() -> Value {
        json!({"taskId":"t-1","agentModel":"hermes","boardId":"b-1","generation":0})
    }

    fn row_ctx(input: Value, checkpoint: Value, attempt: i32) -> RunStepContext {
        let (tx, signal) = crate::runs::define::StepSignal::channel();
        // A dropped watch sender leaves the channel at its last value —
        // `false`, never aborted — which is the shape an uncontended run has.
        drop(tx);
        RunStepContext {
            run: minimal_row(),
            input,
            checkpoint,
            decision: None,
            signal,
            log: Arc::new(|_| {}),
            attempt,
        }
    }

    fn minimal_row() -> RunRow {
        use crate::runs::define::RunState;
        RunRow {
            id: "r-1".into(),
            kind: WORK_SESSION_KIND.into(),
            owner_user_id: None,
            subject_type: Some("task".into()),
            subject_id: Some("t-1".into()),
            state: RunState::Queued,
            phase: "queued".into(),
            checkpoint: serde_json::Value::Null,
            input: serde_json::Value::Null,
            result: serde_json::Value::Null,
            error: None,
            attempt: 0,
            lease_owner: None,
            lease_expires_at: None,
            approval_key: None,
            decision: None,
            created_at: String::new(),
            updated_at: String::new(),
            started_at: None,
            finished_at: None,
        }
    }

    // ── The machine, stage by stage ─────────────────────────────────────────

    #[tokio::test]
    async fn the_brief_pushes_and_checkpoints_turn_one() {
        let (deps, rec) = recording_deps("On it — starting now.");
        let r = work_session_step(row_ctx(input(), json!(null), 0), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = r else {
            panic!("expected next");
        };
        assert_eq!(
            checkpoint,
            json!({"stage":"send","turn":1,"stageAttempt":0,"lastTail":""})
        );
        assert_eq!(phase.unwrap(), "pushed to hermes");
        assert_eq!(
            rec.lines.lock().unwrap().as_slice(),
            &[(
                "talaria".to_string(),
                "t-1: work pushed to hermes".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn a_refused_brief_ends_quietly_with_nothing_written() {
        // `session_state` refuses: the ticket was archived between the
        // enqueue and the first step. NOTHING was pushed, so there is
        // nothing to explain on the ticket.
        let (mut deps, rec) = recording_deps("unused");
        deps.session_state = Arc::new(|_pg, _t, _a| {
            Box::pin(async {
                Ok(Some(SessionState {
                    task: task(),
                    stop: Some("the ticket is archived".into()),
                }))
            })
        });
        let r = work_session_step(row_ctx(input(), json!(null), 0), &deps)
            .await
            .unwrap();
        match r {
            StepResult::Done { result } => {
                assert_eq!(
                    result,
                    json!({"turns": 0, "ended": "the ticket is archived", "dispatched": false})
                );
            }
            _ => panic!("expected done"),
        }
        assert!(rec.lines.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn record_writes_the_reply_and_advances() {
        let (deps, rec) = recording_deps("unused");
        let cp = json!({
            "stage":"record","turn":2,"stageAttempt":0,"said":"turn",
            "reply":{"head":"fixed the parser; tests green","tail":"status: working","checks":["zero_tool_claim","zero_tool_claim","secret_exfil"]}
        });
        let r = work_session_step(row_ctx(input(), cp, 0), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = r else {
            panic!("expected next");
        };
        assert_eq!(phase.unwrap(), "turn 2 recorded");
        assert_eq!(
            checkpoint,
            json!({"stage":"send","turn":3,"stageAttempt":0,"lastTail":"status: working"})
        );
        // Guard ids ride IN FRONT of the whole line, deduped.
        assert_eq!(
            rec.lines.lock().unwrap()[0].1,
            "t-1: [guard: zero_tool_claim, secret_exfil] session turn 2: fixed the parser; tests green"
        );
    }

    #[tokio::test]
    async fn record_lines_match_the_said_kind() {
        let (deps, rec) = recording_deps("unused");
        let cp = |said: &str, head: &str| {
            json!({
                "stage":"record","turn":1,"stageAttempt":0,"said":said,
                "reply":{"head":head,"tail":"","checks":[]}
            })
        };
        // dispatch → picked up, reconcile → its own line, and an empty head
        // is the parenthetical, not an empty slice.
        work_session_step(row_ctx(input(), cp("dispatch", "on it"), 0), &deps)
            .await
            .unwrap();
        work_session_step(row_ctx(input(), cp("reconcile", "reported"), 0), &deps)
            .await
            .unwrap();
        work_session_step(row_ctx(input(), cp("dispatch", ""), 0), &deps)
            .await
            .unwrap();
        let lines = rec.lines.lock().unwrap();
        assert_eq!(lines[0].1, "t-1: picked up: on it");
        assert_eq!(lines[1].1, "t-1: session reconcile: reported");
        assert_eq!(lines[2].1, "t-1: picked up: (no reply)");
    }

    #[tokio::test]
    async fn the_retired_turn_says_unknown_and_waits() {
        let (deps, rec) = recording_deps("unused");
        let now = 1_788_045_420_000i64;
        let cp = json!({"stage":"record","turn":4,"stageAttempt":1,"said":"turn","reply":null});
        let r = work_session_step(row_ctx(input(), cp, 1), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = r else {
            panic!("expected next");
        };
        assert_eq!(phase.unwrap(), "turn 4 recorded");
        // notBefore = now + settle, lastTail cleared, and the ticket is told
        // the outcome is unknown rather than left reading "work pushed to…".
        assert_eq!(
            checkpoint,
            json!({"stage":"send","turn":5,"stageAttempt":1,"lastTail":"","notBefore":now + INTERRUPTED_TURN_SETTLE_MS})
        );
        assert_eq!(
            rec.lines.lock().unwrap()[0].1,
            "t-1: turn 4 was interrupted by a restart — its outcome is unknown; continuing from the ticket's current state"
        );
    }

    #[tokio::test]
    async fn the_turn_cap_ends_the_session_without_asking_the_ticket() {
        let (deps, rec) = recording_deps("unused");
        let cp = json!({"stage":"send","turn":13,"stageAttempt":0,"lastTail":""});
        let r = work_session_step(row_ctx(input(), cp, 0), &deps)
            .await
            .unwrap();
        match r {
            StepResult::Done { result } => {
                assert_eq!(
                    result,
                    json!({"turns": MAX_SESSION_TURNS, "ended": "turn cap"})
                );
            }
            _ => panic!("expected done"),
        }
        // The cap line names the cap and hands the ticket back.
        assert!(
            rec.lines.lock().unwrap()[0]
                .1
                .contains("12-turn cap — leaving the ticket")
        );
    }

    #[tokio::test]
    async fn authority_stops_the_session_mid_flight() {
        let (mut deps, rec) = recording_deps("unused");
        deps.session_state = Arc::new(|_pg, _t, _a| {
            Box::pin(async {
                Ok(Some(SessionState {
                    task: task(),
                    stop: Some("the agent's grant was revoked".into()),
                }))
            })
        });
        let cp = json!({"stage":"send","turn":5,"stageAttempt":2,"lastTail":""});
        let r = work_session_step(row_ctx(input(), cp, 2), &deps)
            .await
            .unwrap();
        match r {
            StepResult::Done { result } => {
                assert_eq!(
                    result,
                    json!({"turns": 4, "ended": "the agent's grant was revoked"})
                );
            }
            _ => panic!("expected done"),
        }
        assert_eq!(
            rec.lines.lock().unwrap()[0].1,
            "t-1: work session ended after 4 turns — the agent's grant was revoked"
        );
    }

    #[tokio::test]
    async fn a_not_before_in_the_future_is_a_soft_retry() {
        let (deps, _rec) = recording_deps("unused");
        let now = 1_788_045_420_000i64;
        let cp = json!({"stage":"send","turn":5,"stageAttempt":1,"lastTail":"","notBefore":now + 60_000});
        let r = work_session_step(row_ctx(input(), cp, 1), &deps)
            .await
            .unwrap();
        match r {
            StepResult::Retry { after, reason } => {
                assert_eq!(after.as_millis() as i64, 60_000);
                assert!(reason.contains("settle before turn 5"));
            }
            _ => panic!("expected retry"),
        }
    }

    #[tokio::test]
    async fn an_owed_turn_is_retired_not_resent() {
        let (deps, rec) = recording_deps("never called");
        // attempt 2 > stageAttempt 0: a driver died holding this turn.
        let cp = json!({"stage":"send","turn":3,"stageAttempt":0,"lastTail":""});
        let r = work_session_step(row_ctx(input(), cp, 2), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = r else {
            panic!("expected next");
        };
        assert_eq!(phase.unwrap(), "turn 3 interrupted");
        assert_eq!(
            checkpoint,
            json!({"stage":"record","turn":3,"stageAttempt":2,"said":"turn","reply":null})
        );
        // And the turn dep was never invoked — the fake's `turn_text` is
        // "never called", so any call would have produced a record reply.
        assert!(rec.lines.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_turn_kind_comes_from_the_turn_number_and_the_tail() {
        // turn 1 → dispatch (the brief), whatever the tail says.
        let (deps, _rec) = recording_deps("Done with it.");
        let cp = json!({"stage":"send","turn":1,"stageAttempt":0,"lastTail":"DONE"});
        let StepResult::Next { checkpoint, .. } = work_session_step(row_ctx(input(), cp, 0), &deps)
            .await
            .unwrap()
        else {
            panic!("expected next");
        };
        assert_eq!(checkpoint["said"], "dispatch");
        // A tail that says DONE → reconcile.
        let cp = json!({"stage":"send","turn":4,"stageAttempt":0,"lastTail":"... I am DONE with the fix"});
        let StepResult::Next { checkpoint, .. } = work_session_step(row_ctx(input(), cp, 0), &deps)
            .await
            .unwrap()
        else {
            panic!("expected next");
        };
        assert_eq!(checkpoint["said"], "reconcile");
        // Otherwise a plain turn.
        let cp = json!({"stage":"send","turn":4,"stageAttempt":0,"lastTail":"still working"});
        let StepResult::Next { checkpoint, .. } = work_session_step(row_ctx(input(), cp, 0), &deps)
            .await
            .unwrap()
        else {
            panic!("expected next");
        };
        assert_eq!(checkpoint["said"], "turn");
        // The reply is sliced: head 300, tail 200.
        assert_eq!(checkpoint["reply"]["tail"], "Done with it.");
    }

    #[tokio::test]
    async fn a_turn_that_answers_nothing_fails_through_a_checkpoint() {
        let (mut deps, _rec) = recording_deps("unused");
        deps.turn = Arc::new(|_s, _a, _t, _p| {
            Box::pin(async { Err("gateway completion 429: rate limited".to_string()) })
        });
        let cp = json!({"stage":"send","turn":2,"stageAttempt":0,"lastTail":""});
        let r = work_session_step(row_ctx(input(), cp, 0), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = r else {
            panic!("expected next, not a throw — a throw would re-send the prompt");
        };
        assert_eq!(phase.unwrap(), "turn 2 failed");
        assert_eq!(checkpoint["stage"], "failed");
        assert_eq!(checkpoint["error"], "gateway completion 429: rate limited");
    }

    #[tokio::test]
    async fn the_failed_stage_tells_the_ticket_then_fails_the_run() {
        let (deps, rec) = recording_deps("unused");
        let cp =
            json!({"stage":"failed","turn":6,"stageAttempt":0,"error":"the model never answered"});
        let err = work_session_step(row_ctx(input(), cp, 1), &deps)
            .await
            .unwrap_err();
        assert_eq!(err, "the model never answered");
        assert_eq!(
            rec.lines.lock().unwrap()[0].1,
            "t-1: work session with hermes failed: the model never answered"
        );
    }

    #[tokio::test]
    async fn a_step_failure_becomes_a_failed_checkpoint_for_the_next_step() {
        let (mut deps, _rec) = recording_deps("unused");
        deps.session_state =
            Arc::new(|_pg, _t, _a| Box::pin(async { Err("the boards read died".to_string()) }));
        let cp = json!({"stage":"send","turn":3,"stageAttempt":0,"lastTail":""});
        let r = work_session_step(row_ctx(input(), cp, 0), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = r else {
            panic!("expected next");
        };
        assert_eq!(phase.unwrap(), "session failed");
        assert_eq!(checkpoint["stage"], "failed");
        assert_eq!(checkpoint["error"], "the boards read died");
    }

    #[tokio::test]
    async fn an_abort_is_the_drivers_outcome_not_a_failed_session() {
        // The same step failure as above, but the driver aborted: the error
        // goes OUT rather than onto the ticket, because the session may be
        // alive on another instance — a "work session failed" line here
        // would be about a session this instance no longer owns.
        let (mut deps, rec) = recording_deps("unused");
        deps.session_state =
            Arc::new(|_pg, _t, _a| Box::pin(async { Err("the boards read died".to_string()) }));
        let (tx, signal) = crate::runs::define::StepSignal::channel();
        let _ = tx.send(true);
        drop(tx);
        let err = work_session_step(
            RunStepContext {
                run: minimal_row(),
                input: input(),
                checkpoint: json!({"stage":"send","turn":3,"stageAttempt":0,"lastTail":""}),
                decision: None,
                signal,
                log: Arc::new(|_| {}),
                attempt: 0,
            },
            &deps,
        )
        .await
        .unwrap_err();
        assert_eq!(err, "the boards read died");
        assert!(rec.lines.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_brief_dedupes_on_reentry() {
        // A driver died after writing the dispatch line but before the
        // checkpoint: re-entry (attempt 1) reads the ticket's history, finds
        // its own line, and says nothing — the session still advances.
        let (deps, rec) = recording_deps("unused");
        rec.recent.lock().unwrap().push(tasks::TaskActivity {
            id: "a-1".into(),
            actor: "talaria".into(),
            kind_tag: "dispatch".into(),
            description: "work pushed to hermes".into(),
            created_at: String::new(),
        });
        let r = work_session_step(row_ctx(input(), json!(null), 1), &deps)
            .await
            .unwrap();
        assert!(matches!(r, StepResult::Next { .. }));
        assert!(rec.lines.lock().unwrap().is_empty());
    }

    // ── slices ──────────────────────────────────────────────────────────────

    #[test]
    fn utf16_slices_cut_at_whole_characters() {
        assert_eq!(utf16_prefix("hello", 3), "hel");
        assert_eq!(utf16_prefix("hello", 50), "hello");
        assert_eq!(utf16_suffix("hello", 3), "llo");
        assert_eq!(utf16_suffix("hello", 50), "hello");
        // 😀 is two units: 199 ascii + 😀 = 201 units; the prefix cut at 200
        // stops before the pair rather than splitting it.
        let s = format!("{}😀", "x".repeat(199));
        assert_eq!(utf16_len(utf16_prefix(&s, 200)), 199);
        assert_eq!(utf16_suffix(&s, 2), "😀");
        assert_eq!(utf16_suffix(&s, 1), ""); // one unit cannot hold the pair
    }

    fn utf16_len(s: &str) -> usize {
        s.encode_utf16().count()
    }

    #[test]
    fn noted_dedupes_and_keeps_order() {
        let checks = vec!["a".to_string(), "b".to_string(), "a".to_string()];
        assert_eq!(noted("line", &checks), "[guard: a, b] line");
        assert_eq!(noted("line", &[]), "line");
    }
}
