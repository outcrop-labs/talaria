// The pair lock and the rewind hold: what keeps a rewind and the dispatcher
// from opening two sessions on one (ticket, agent).
//
// The dispatch walk reads the pair's runs through the pooled store, so a
// transaction-scoped lock would cover none of it. The lock is a SESSION
// advisory lock on one checked-out connection, held from the hold read
// through the walk until `enqueue` returns. The rewind takes the same lock to
// place its hold, and again to insert its attempt and lift the hold, so a
// dispatcher past its hold read cannot claim the attempt's generation.
//
// A rewind that fails can leave its fork in the agent's store under the
// attempt's id when the agent cannot be reached to delete it. The dispatcher
// never claims an id a rewind reserved (`rewind_reserved`), so no fresh
// session continues that conversation. Only the next rewind, which asks the
// agent first, takes the fork back and may use the generation again.
//
// A session lock rides its connection: a pooled connection handed back with
// the lock still held would hold it for the life of the pool. `release`
// unlocks before the connection returns, and a guard dropped unreleased
// spawns the unlock.

use sqlx::pool::PoolConnection;
use sqlx::{PgConnection, PgPool, Postgres};
use talaria_runs_work_session::session_run_id;

use crate::MAX_SESSIONS_PER_TICKET_AGENT;

pub struct SessionLock {
    conn: Option<PoolConnection<Postgres>>,
}

/// Take the pair's lock, waiting for whoever holds it.
pub async fn lock_pair(
    pg: &PgPool,
    task_id: &str,
    agent_model: &str,
) -> Result<SessionLock, sqlx::Error> {
    let mut conn = pg.acquire().await?;
    sqlx::query("select pg_advisory_lock(hashtext($1))")
        .bind(format!("work-session:{task_id}:{agent_model}"))
        .execute(&mut *conn)
        .await?;
    Ok(SessionLock { conn: Some(conn) })
}

impl SessionLock {
    pub fn conn(&mut self) -> &mut PgConnection {
        self.conn
            .as_mut()
            .expect("a session lock holds its connection until release")
    }

    pub async fn release(mut self) {
        if let Some(mut conn) = self.conn.take() {
            let _ = sqlx::query("select pg_advisory_unlock_all()")
                .execute(&mut *conn)
                .await;
        }
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            tokio::spawn(async move {
                let _ = sqlx::query("select pg_advisory_unlock_all()")
                    .execute(&mut *conn)
                    .await;
            });
        }
    }
}

/// The rewind holding the pair, or None. A hold whose rewind run has ended
/// is deleted here: a rewind that exhausted its driver attempts never ran a
/// step that could lift it.
pub async fn hold_on(
    conn: &mut PgConnection,
    task_id: &str,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    let hold: Option<(String, String)> = sqlx::query_as(
        "select h.rewind_run_id::text, r.state from work_dispatch_holds h \
         join runs r on r.id = h.rewind_run_id \
         where h.task_id = $1::uuid and h.agent_model = $2",
    )
    .bind(task_id)
    .bind(agent_model)
    .fetch_optional(&mut *conn)
    .await?;
    match hold {
        None => Ok(None),
        Some((rewind, state)) if matches!(state.as_str(), "done" | "error" | "cancelled") => {
            lift_hold(conn, task_id, agent_model, &rewind).await?;
            Ok(None)
        }
        Some((rewind, _)) => Ok(Some(rewind)),
    }
}

/// Place `rewind_run_id`'s hold on the pair. False when another live rewind
/// already holds it; a hold this rewind placed earlier is kept.
pub async fn place_hold(
    conn: &mut PgConnection,
    task_id: &str,
    agent_model: &str,
    rewind_run_id: &str,
) -> Result<bool, sqlx::Error> {
    match hold_on(conn, task_id, agent_model).await? {
        Some(other) if other != rewind_run_id => return Ok(false),
        Some(_) => return Ok(true),
        None => {}
    }
    sqlx::query(
        "insert into work_dispatch_holds (task_id, agent_model, rewind_run_id) \
         values ($1::uuid, $2, $3::uuid) \
         on conflict (task_id, agent_model) do nothing",
    )
    .bind(task_id)
    .bind(agent_model)
    .bind(rewind_run_id)
    .execute(conn)
    .await?;
    Ok(true)
}

pub async fn lift_hold(
    conn: &mut PgConnection,
    task_id: &str,
    agent_model: &str,
    rewind_run_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "delete from work_dispatch_holds \
         where task_id = $1::uuid and agent_model = $2 and rewind_run_id = $3::uuid",
    )
    .bind(task_id)
    .bind(agent_model)
    .bind(rewind_run_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// The pair's session ids that a `run-rewind` reserved for its attempt,
/// whatever came of the rewind. A rewind checkpoints its plan, with the
/// attempt's id at `plan.attemptRunId`, before it forks anything, so this
/// names every id a leftover fork can sit under. A rewind that made its
/// attempt has a run row at that id as well, which the walk reads first.
pub async fn rewind_reserved(
    conn: &mut PgConnection,
    task_id: &str,
    agent_model: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let pair: Vec<String> = (0..MAX_SESSIONS_PER_TICKET_AGENT)
        .map(|generation| session_run_id(task_id, agent_model, generation))
        .collect();
    sqlx::query_scalar(
        "select distinct checkpoint->'plan'->>'attemptRunId' from runs \
         where kind = 'run-rewind' and checkpoint->'plan'->>'attemptRunId' = any($1)",
    )
    .bind(&pair)
    .fetch_all(conn)
    .await
}
