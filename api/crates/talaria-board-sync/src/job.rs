// The `board-sync` scheduler job: once a minute, start a `clickup-sync` run for
// every active connection that is due, and a remap run for every connection
// holding a mapping edit no run has applied. It only ENQUEUES — the runs
// engine owns the work, its lease, its reclaim and its retry.
//
// THE SCHEDULE ADVANCES WHATEVER THE OUTCOME: next_sync_at moves to now +
// interval in the same statement that claims the row, so a run that fails
// does not make the next tick start it again at once, and two instances
// cannot both claim it (`for update skip locked`, under the job's lease
// besides). A board that is archived is skipped: an archived board takes no
// imports.
//
// It also prunes: finished sync runs older than RUN_KEEP_DAYS go, except each
// connection's last run (the status reads it). One run per board per tick is
// otherwise a row a minute forever.

use std::sync::Arc;

use talaria_scheduler::{JobName, JobSpec, register_job};
use talaria_state::AppState;

use crate::store::{SyncRow, sync_by_id};
use crate::{KIND, Mode, Started, start_run};

const EVERY_MS: u64 = 60_000;
const FIRST_DELAY_MS: u64 = 90_000;
const MAX_RUN_MS: u64 = 30_000;
const CLAIM_LIMIT: i64 = 50;
pub const RUN_KEEP_DAYS: i32 = 14;

pub fn register_board_sync_job(state: AppState) {
    register_job(JobSpec {
        name: JobName::BoardSync,
        every_ms: EVERY_MS,
        first_run_delay_ms: Some(FIRST_DELAY_MS),
        max_run_ms: Some(MAX_RUN_MS),
        per_instance: false,
        run: Arc::new(move || {
            let state = state.clone();
            Box::pin(async move { tick(&state).await })
        }),
    });
}

/// Claim the due connections and push their next time out, in one statement.
async fn claim_due(pg: &sqlx::PgPool) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "with due as ( \
           select s.id from board_syncs s join boards b on b.id = s.board_id \
           where s.state = 'active' and s.interval_minutes is not null \
             and s.next_sync_at is not null and s.next_sync_at <= now() \
             and b.archived_at is null \
           order by s.next_sync_at asc limit $1 \
           for update of s skip locked \
         ) \
         update board_syncs s set next_sync_at = now() + (s.interval_minutes * interval '1 minute'), \
           updated_at = now() \
         from due where s.id = due.id returning s.id::text",
    )
    .bind(CLAIM_LIMIT)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Connections holding a mapping edit and no live run on their board.
async fn pending_remaps(pg: &sqlx::PgPool) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select s.id::text from board_syncs s join boards b on b.id = s.board_id \
         where s.remap_pending and s.state in ('active', 'paused', 'needs_credential') \
           and b.archived_at is null \
           and not exists (select 1 from runs r where r.kind = $1 and r.subject_id = s.board_id::text \
                           and r.state in ('queued', 'running', 'awaiting')) \
         limit $2",
    )
    .bind(KIND)
    .bind(CLAIM_LIMIT)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn prune_runs(pg: &sqlx::PgPool) -> Result<u64, sqlx::Error> {
    let r = sqlx::query(
        "delete from runs r where r.kind = $1 and r.state in ('done', 'error', 'cancelled') \
           and r.finished_at < now() - ($2::int * interval '1 day') \
           and not exists (select 1 from board_syncs s where s.last_run_id = r.id)",
    )
    .bind(KIND)
    .bind(RUN_KEEP_DAYS)
    .execute(pg)
    .await?;
    Ok(r.rows_affected())
}

async fn start(state: &AppState, id: &str, mode: Mode) -> Option<Started> {
    let sync: SyncRow = match sync_by_id(&state.pg, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("[board-sync] connection {id} could not be read: {e}");
            return None;
        }
    };
    match start_run(state, &sync, mode, None).await {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(
                "[board-sync] sync of board {} not started: {e}",
                sync.board_id
            );
            None
        }
    }
}

pub async fn tick(state: &AppState) -> Result<Option<String>, String> {
    let due = claim_due(&state.pg).await.map_err(|e| e.to_string())?;
    let (mut started, mut live) = (0, 0);
    for id in &due {
        match start(state, id, Mode::Scheduled).await {
            Some(Started::New(_)) => started += 1,
            Some(Started::Live(_)) => live += 1,
            None => {}
        }
    }
    let remaps = pending_remaps(&state.pg).await.map_err(|e| e.to_string())?;
    let mut remapped = 0;
    for id in &remaps {
        if let Some(Started::New(_)) = start(state, id, Mode::Remap).await {
            remapped += 1;
        }
    }
    let pruned = prune_runs(&state.pg).await.unwrap_or_else(|e| {
        tracing::warn!("[board-sync] run prune failed: {e}");
        0
    });
    if started + live + remapped == 0 && pruned == 0 {
        return Ok(None);
    }
    Ok(Some(format!(
        "{started} sync run(s) started, {live} already running, {remapped} remap(s), {pruned} old run(s) pruned"
    )))
}
