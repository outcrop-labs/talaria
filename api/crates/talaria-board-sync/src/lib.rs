// A BOARD SYNCS FROM A CLICKUP LIST — one way, ClickUp → Talaria
// (docs/BOARD-SYNC.md). A board owner connects one list with a ClickUp personal
// token; a durable `clickup-sync` run pulls its tasks, comments and
// attachments into ordinary tickets through the task engine, on a schedule
// (manual, 15 minutes, hourly, daily) and on "Sync now".
//
// THE PIECES:
//   mapping.rs  pure: the status table, the intake fallback, the move guards,
//               the create route's clamps, people, the comment author label.
//   merge.rs    pure: the field-level three-way conflict rule.
//   store.rs    the sync's own rows (board_syncs, board_sync_items).
//   world.rs    the step's edges — ClickUp (`Remote`) and Talaria (`Local`).
//   run.rs      the run kind: passes over a checkpoint, budgeted by time and
//               by ClickUp's rate limit.
//   job.rs      the scheduler job that starts due syncs, and run pruning.
//   status.rs   the projection the board's chip and settings read.
//   ops.rs      what the routes do: connect, edit, replace the credential,
//               disconnect, sync now, re-sync a ticket, browse, preview.
//
// OWNED BY THE BOARD, not the person: the run has no owner (subject 'board'),
// the credential sits on the board's row, and connected_by is only a record.
// A ClickUp token dies with its ClickUp member, which Talaria cannot prevent —
// the GET /user probe every run starts with turns that into a red chip for
// every member and one grouped notice to the board's owners and the admins,
// and "Replace credential" keeps the mapping, the links and the history.
//
// Provider-neutral names (board_syncs, provider = 'clickup') at no cost; there
// is no provider trait in v1, and a general migration tool is out of scope.

pub mod job;
pub mod mapping;
pub mod merge;
pub mod ops;
pub mod run;
pub mod status;
pub mod store;
pub mod world;

use std::sync::{Arc, OnceLock};

use serde_json::json;
use talaria_clickup::Client;
use talaria_runs_define::{Authority, DEFAULT_MAX_ATTEMPTS, RunDefinition, register_run};
use talaria_runs_run::{EnqueueOptions, enqueue};
use talaria_secretbox::SecretBox;
use talaria_state::AppState;
use talaria_tasks_types::TaskDeps;

pub use run::{Input, Mode};
pub use store::SyncRow;

/// The run kind. Stable forever: it is written into every row.
pub const KIND: &str = "clickup-sync";
pub const PROVIDER: &str = "clickup";
/// Who the sync is on a ticket: `created_by` and the activity actor. Matches
/// no agent and no person (the comment-room owner falls back past it).
pub const SYNC_ACTOR: &str = "clickup-sync";

/// One step's wall-clock deadline (and the lease TTL). Steps stop themselves
/// at a quarter of it (run::STEP_BUDGET_MS).
const MAX_STEP_MS: u64 = 240_000;

/// `tasks.external_key` for a ClickUp task.
pub fn external_key(task_id: &str) -> String {
    format!("clickup:{task_id}")
}

/// `channel_messages.import_key` for a ClickUp comment or reply.
pub fn comment_key(comment_id: &str) -> String {
    format!("clickup:comment:{comment_id}")
}

/// The ClickUp page of a list, when the workspace is known.
pub fn list_url(workspace_id: Option<&str>, list_id: &str) -> Option<String> {
    workspace_id.map(|w| format!("https://app.clickup.com/{w}/v/li/{list_id}"))
}

/// Open the connection's sealed token into a client.
pub fn open_client(sb: &SecretBox, sync: &SyncRow) -> Result<Client, String> {
    let sealed = sync
        .credential_sealed
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "the sync has no credential".to_string())?;
    let token = sb
        .open(sealed)
        .map_err(|_| "the sync's credential does not decrypt".to_string())?;
    Ok(Client::new(&token))
}

/// The AppState the step builds its edges from, per step (so a key rotation's
/// new secretbox is the one used). Armed by the boot wiring, and by the routes
/// that start a run — a process with the schedule off still drives the run a
/// person started from it.
static ARMED: OnceLock<AppState> = OnceLock::new();

pub fn arm_sync_step(state: AppState) {
    let _ = ARMED.set(state);
}

async fn real_world(state: &AppState) -> Result<world::Real, String> {
    let sb = state.secretbox().await?;
    let redis = state.redis().await.ok();
    Ok(world::Real {
        pg: state.pg.clone(),
        tasks: TaskDeps::from_route(state.pg.clone(), redis),
        sb,
    })
}

/// The registered definition, once per process.
pub fn sync_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: KIND.into(),
            label: "Sync a board from ClickUp".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(state) = ARMED.get().cloned() else {
                        return Err(
                            "clickup-sync steps are armed by the boot wiring; this step was reached \
                             by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    let real = real_world(&state).await?;
                    let sb = real.sb.clone();
                    run::run_step(
                        &ctx.run.id,
                        &ctx.input,
                        &ctx.checkpoint,
                        &ctx.signal,
                        &real,
                        move |sync: &SyncRow| open_client(&sb, sync),
                    )
                    .await
                })
            }),
            // THE BOARD'S RUN: every member may watch it (may_watch_run's
            // subject 'board'), nobody owns it.
            audience: Arc::new(|run| Authority::Board {
                board_id: run.subject_id.clone().unwrap_or_default(),
            }),
            max_step_ms: MAX_STEP_MS,
            idle_step_ms: None,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        })
    })
}

/// What starting a run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    New(String),
    /// A sync was already live on the board; this is that run.
    Live(String),
}

impl Started {
    pub fn id(&self) -> &str {
        match self {
            Started::New(id) | Started::Live(id) => id,
        }
    }
}

/// The board's live sync run, if any.
pub async fn live_run(
    pg: &sqlx::PgPool,
    board_id: &str,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "select id::text, phase from runs where kind = $1 and subject_id = $2 \
           and state in ('queued', 'running', 'awaiting') order by created_at desc limit 1",
    )
    .bind(KIND)
    .bind(board_id)
    .fetch_optional(pg)
    .await
}

/// The runs engine's real edges, assembled here rather than read from the
/// boot-injected dispatch seam: a process with the schedule off never runs the
/// boot wiring, and a person's Sync now must still start (and drive) its run.
async fn run_deps(state: &AppState) -> Result<talaria_runs_run::RunDeps, String> {
    let redis = state
        .redis()
        .await
        .map_err(|e| format!("the sync could not be started: redis is unavailable ({e})"))?;
    let realtime = talaria_realtime::RealtimeDeps::publish_only(Some(redis.clone()));
    Ok(talaria_runs_decide::assembly::real_run_deps(
        state.pg.clone(),
        redis,
        realtime,
    ))
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23505")
}

/// Start a sync run for this connection — or answer the one already live on
/// the board (the partial unique index `runs_one_live_board_sync` is the
/// guarantee; the read first is a courtesy).
pub async fn start_run(
    state: &AppState,
    sync: &SyncRow,
    mode: Mode,
    external_id: Option<String>,
) -> Result<Started, String> {
    arm_sync_step(state.clone());
    let _ = sync_run();
    if let Some((id, _)) = live_run(&state.pg, &sync.board_id)
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(Started::Live(id));
    }
    let deps = run_deps(state).await?;
    let input = Input {
        sync_id: sync.id.clone(),
        mode,
        external_id,
    };
    let phase = match mode {
        Mode::Remap => "Queued — applying the new status mapping",
        Mode::Ticket => "Queued — re-syncing a ticket",
        _ => "Queued",
    };
    match enqueue(
        sync_run(),
        serde_json::to_value(&input).unwrap_or_else(|_| json!({})),
        EnqueueOptions {
            owner_user_id: None,
            subject_type: Some("board".into()),
            subject_id: Some(sync.board_id.clone()),
            phase: Some(phase.into()),
            id: None,
            start: Some(true),
        },
        &deps,
    )
    .await
    {
        Ok(run) => {
            if let Err(e) = store::set_last_run(&state.pg, &sync.id, &run.id).await {
                tracing::warn!("[board-sync] last run not recorded for {}: {e}", sync.id);
            }
            Ok(Started::New(run.id))
        }
        Err(e) if is_unique_violation(&e) => {
            match live_run(&state.pg, &sync.board_id)
                .await
                .map_err(|e| e.to_string())?
            {
                Some((id, _)) => Ok(Started::Live(id)),
                None => Err("a sync run was starting on this board — try again".into()),
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Stop the board's live run (a disconnect). The runs store's compare-and-set
/// refuses the in-flight step's next write.
pub async fn cancel_live(state: &AppState, board_id: &str, reason: &str) -> Result<(), String> {
    let Some((id, _)) = live_run(&state.pg, board_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let deps = run_deps(state).await?;
    talaria_runs_run::cancel_run(&id, Some(reason.to_string()), &deps)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
