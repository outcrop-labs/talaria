// THE `runs` table, and nothing else.
//
// Separated from the driver for one reason that is not tidiness: every write in
// here is a COMPARE-AND-SET on `(id, lease_owner, state)`, and those predicates
// are the entire correctness argument of the runs system. A driver that could
// write `set state = 'done'` without the `where lease_owner = $token and state
// = 'running'` clause would be a driver that finishes a run another instance is
// already advancing, and a cancellation that only the running instance honors.
// Holding them in one file, in one shape, is how they stay reviewable.
//
// THE PREDICATE IS ALSO THE CANCELLATION CHECK. Because every write requires
// `state = 'running'`, a run that any instance set to `cancelled` rejects the
// next write from its driver automatically — no polling, no flag, no
// "whichever process happens to be running it". The driver still re-reads the
// row at each step boundary so it can stop BEFORE burning another step, but
// even a driver that did not could not finish a cancelled run.
//
// The `RunStore` trait keeps the store overridable per call: the driver is
// written against `dyn RunStore`, and its tests fake the whole thing with no
// database.

use super::define::{DecisionAnswer, RunDecision, RunRow, RunState, is_terminal};
use crate::agent_auth::epoch_ms_to_iso;
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;
use sqlx::Row as _;
use sqlx::postgres::{PgPool, PgRow};

/// Why a compare-and-set write did not land. Never "it failed" — every one of
/// these means something different to the driver, and collapsing them is how a
/// clean handover gets logged as an error.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteFailure {
    /// Another instance owns this run now. A CLEAN STOP, not a fault.
    LeaseLost { state: RunState },
    /// Somebody cancelled it while the step was running. Honor it.
    Cancelled,
    /// The row is gone (the owner's account was deleted mid-run, say).
    Missing,
    /// The row is no longer `running` and not cancelled — another driver
    /// parked or finished it. Also a clean stop.
    State { state: RunState },
}

pub type WriteOutcome = Result<(), WriteFailure>;

/// `claim`'s four answers. `Taken` is not an error at either call site: held
/// by a live lease, or deferred by a `retry` whose wait has not elapsed — the
/// sweep will find it again.
#[derive(Debug, Clone)]
pub enum ClaimOutcome {
    /// This run is ours. `reclaimed` = it was RECLAIMED from a driver that
    /// stopped renewing — a crash, a deploy, or a container paused past its
    /// lease — and `attempt` has already been incremented on the row. Boxed:
    /// the row is 20-odd fields and would dwarf the empty arms of this enum.
    Claimed {
        run: Box<RunRow>,
        reclaimed: bool,
    },
    Missing,
    Taken {
        state: RunState,
        until: Option<String>,
    },
    NotRunnable {
        state: RunState,
    },
}

#[derive(Debug, Clone)]
pub enum AnswerOutcome {
    Answered(Box<RunRow>),
    Missing,
    /// Not `awaiting` — either it never was, or a second answer of the same
    /// question won the race (somebody answered it, which is what they
    /// wanted).
    NotAwaiting {
        state: Option<RunState>,
    },
    /// The answer names a question this run is not parked on — a stale tab,
    /// the run having since been re-parked on a DIFFERENT question. Two
    /// people, two devices, one run: this is not hypothetical.
    StaleKey {
        state: RunState,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CancelOutcome {
    Cancelled { state: RunState },
    Missing,
    Terminal { state: RunState },
}

/// A run being created. `enqueue` (in run.rs) fills this in.
#[derive(Debug, Clone)]
pub struct NewRun {
    pub id: String,
    pub kind: String,
    pub owner_user_id: Option<String>,
    pub subject_type: Option<String>,
    pub subject_id: Option<String>,
    pub input: Value,
    pub phase: String,
}

pub trait RunStore: Send + Sync {
    fn insert<'a>(&'a self, row: NewRun) -> BoxFuture<'a, Result<RunRow, sqlx::Error>>;
    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<RunRow>, sqlx::Error>>;
    /// Take the run, if it is takeable. Atomic: the state flip, the lease
    /// stamp and the attempt increment are one statement, because a claim
    /// split into a read and a write is a claim two instances can both win.
    fn claim<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        lease_ms: i64,
    ) -> BoxFuture<'a, Result<ClaimOutcome, sqlx::Error>>;
    /// Push the lease out while a step is still going.
    fn heartbeat<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        lease_ms: i64,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>>;
    /// Progress. The ONE write that must land before its side effect is
    /// visible anywhere — see the ordering rule in run.rs.
    fn checkpoint<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        checkpoint: Value,
        phase: String,
        clear_decision: bool,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>>;
    /// Words only. Split from `checkpoint` so `ctx.log` cannot accidentally
    /// persist a checkpoint the step has not returned yet.
    fn phase<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        phase: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>>;
    fn complete<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        result: Value,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>>;
    fn fail<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        error: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>>;
    /// Park on a human decision.
    #[allow(clippy::too_many_arguments)]
    fn park<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        decision: RunDecision,
        approval_key: String,
        phase: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>>;
    /// Soft pause: back to `queued`, but not takeable until `until_ms` (epoch
    /// ms). The lease stamp IS the wait — there is no `next_attempt_at`
    /// column because that is precisely what a lease expiry already means,
    /// and two columns describing one instant is two columns that will
    /// disagree.
    fn defer<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        until_ms: i64,
        reason: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>>;
    /// Give the lease back without changing state, so the next driver does
    /// not have to wait out the TTL.
    fn release<'a>(&'a self, id: &'a str, token: &'a str)
    -> BoxFuture<'a, Result<(), sqlx::Error>>;
    /// Answer the question a run is parked on and put it back in the queue.
    /// Callable by any instance — the person answering is on whichever one
    /// their request landed on, which is never reliably the one that asked.
    fn answer<'a>(
        &'a self,
        id: &'a str,
        answer: DecisionAnswer,
    ) -> BoxFuture<'a, Result<AnswerOutcome, sqlx::Error>>;
    /// Cancel from ANYWHERE. The honorable stop: no lease predicate, because
    /// cancellation must be honorable by an instance that has never touched
    /// the run. The driver that owns it finds out at its next step boundary,
    /// or when its next write is refused.
    fn cancel<'a>(
        &'a self,
        id: &'a str,
        reason: Option<String>,
    ) -> BoxFuture<'a, Result<CancelOutcome, sqlx::Error>>;
    /// Runs whose driver stopped renewing, oldest first. THE reclaim query.
    fn due<'a>(&'a self, limit: i64) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>>;
    /// "This user's active runs" — the other real query.
    fn active_for<'a>(
        &'a self,
        user_id: &'a str,
        limit: Option<i64>,
    ) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>>;
}

// ── The Postgres implementation ──────────────────────────────────────────────

/// The store over the shared pool. `PgPool` is already an interior-mutex
/// caravan; every method is `&self`.
#[derive(Clone)]
pub struct PgRunStore {
    pub pg: PgPool,
}

/// Selected explicitly and epoch-ms-cast, rather than `select *`, so adding a
/// column to the table cannot silently change the shape every consumer reads
/// (and so no timestamp crate is needed: the row is published over SSE and
/// rendered on a device that never talked to Postgres, so it leaves this file
/// as ISO strings — the note on `RunRow`).
///
/// `pub(crate)` because the approvals census reads the same whole-row shape —
/// its run rows are handed to the run definition's own `audience`, which is
/// the row's whole business — and a second column list would be a second
/// opinion about what a run row is.
pub(crate) const COLS: &str = "id::text, kind, \
     owner_user_id::text, subject_type, subject_id, state, phase, checkpoint, input, result, \
     error, attempt, lease_owner, \
     (trunc(extract(epoch from lease_expires_at) * 1000))::bigint as lease_expires_ms, \
     approval_key, decision, \
     (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
     (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms, \
     (trunc(extract(epoch from started_at) * 1000))::bigint as started_ms, \
     (trunc(extract(epoch from finished_at) * 1000))::bigint as finished_ms";

/// A state string this build does not know — a row from a newer deploy. Loud
/// decode failure rather than a silent carry: a state the driver cannot match
/// is a state it cannot make a decision about.
fn decode_state(s: &str) -> Result<RunState, sqlx::Error> {
    let state = match s {
        "queued" => RunState::Queued,
        "running" => RunState::Running,
        "awaiting" => RunState::Awaiting,
        "done" => RunState::Done,
        "error" => RunState::Error,
        "cancelled" => RunState::Cancelled,
        other => {
            return Err(sqlx::Error::ColumnDecode {
                index: "state".into(),
                source: format!("unknown run state {other:?} (row from a newer deploy?)").into(),
            });
        }
    };
    Ok(state)
}

fn decode_decision(v: Option<Value>) -> Result<Option<RunDecision>, sqlx::Error> {
    v.map(|raw| {
        serde_json::from_value(raw).map_err(|e| sqlx::Error::ColumnDecode {
            index: "decision".into(),
            source: format!("decision column is not a RunDecision: {e}").into(),
        })
    })
    .transpose()
}

/// Row → `RunRow`, shared by every reader that selects `COLS` (the store's own
/// reads and the approvals census's `awaiting` sweep alike).
pub(crate) fn hydrate(row: &PgRow) -> Result<RunRow, sqlx::Error> {
    Ok(RunRow {
        id: row.try_get("id")?,
        kind: row.try_get("kind")?,
        owner_user_id: row.try_get::<Option<String>, _>("owner_user_id")?,
        subject_type: row.try_get("subject_type")?,
        subject_id: row.try_get("subject_id")?,
        state: decode_state(&row.try_get::<String, _>("state")?)?,
        phase: row.try_get("phase")?,
        // Null jsonb decodes as Value::Null, not an error: a run with no
        // checkpoint yet is the FIRST step, not a broken row.
        checkpoint: row
            .try_get::<Option<Value>, _>("checkpoint")?
            .unwrap_or(Value::Null),
        input: row
            .try_get::<Option<Value>, _>("input")?
            .unwrap_or(Value::Null),
        result: row
            .try_get::<Option<Value>, _>("result")?
            .unwrap_or(Value::Null),
        error: row.try_get("error")?,
        attempt: row.try_get("attempt")?,
        lease_owner: row.try_get("lease_owner")?,
        lease_expires_at: row
            .try_get::<Option<i64>, _>("lease_expires_ms")?
            .map(epoch_ms_to_iso),
        approval_key: row.try_get("approval_key")?,
        decision: decode_decision(row.try_get("decision")?)?,
        created_at: epoch_ms_to_iso(row.try_get("created_ms")?),
        updated_at: epoch_ms_to_iso(row.try_get("updated_ms")?),
        started_at: row
            .try_get::<Option<i64>, _>("started_ms")?
            .map(epoch_ms_to_iso),
        finished_at: row
            .try_get::<Option<i64>, _>("finished_ms")?
            .map(epoch_ms_to_iso),
    })
}

/// Explain a compare-and-set that matched nothing. Costs one extra read, and
/// only on the path where something already went sideways — the alternative is
/// a driver that logs 'write failed' for a clean handover, a cancellation and
/// a deleted row alike, which is the exact silence this project is eliminating.
async fn why(pg: &PgPool, id: &str, token: &str) -> WriteFailure {
    match get_pg(pg, id).await {
        Ok(None) => WriteFailure::Missing,
        Ok(Some(run)) => match run.state {
            RunState::Cancelled => WriteFailure::Cancelled,
            _ if run.lease_owner.as_deref() != Some(token) => {
                WriteFailure::LeaseLost { state: run.state }
            }
            _ => WriteFailure::State { state: run.state },
        },
        // The explanation read failed; the honest report is the failure we
        // know least about. The driver treats every WriteFailure as a stop.
        Err(_) => WriteFailure::Missing,
    }
}

async fn get_pg(pg: &PgPool, id: &str) -> Result<Option<RunRow>, sqlx::Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "select {COLS} from runs where id = $1::uuid"
    )))
    .bind(id)
    .fetch_optional(pg)
    .await?;
    row.map(|r| hydrate(&r)).transpose()
}

/// Clamp run error text at 4000 BYTES on a char boundary — the cut lands
/// before the boundary, never inside a character.
fn clamp_error(error: &str) -> String {
    if error.len() <= 4000 {
        return error.to_string();
    }
    let mut end = 4000;
    while end > 0 && !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].to_string()
}

fn decision_value(decision: &RunDecision) -> Result<Value, sqlx::Error> {
    serde_json::to_value(decision).map_err(|e| sqlx::Error::Encode(Box::new(e)))
}

impl PgRunStore {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }
}

/// One compare-and-set write: run the statement, and if it matched nothing,
/// explain WHY it did not land. Every write shares this shape; the macro keeps
/// the predicate (in the SQL) and the explanation (in `why`) reading as one
/// decision instead of two that can drift.
macro_rules! cas_write {
    ($pg:expr, $id:expr, $token:expr, $sql:expr, $($bind:expr),* $(,)?) => {{
        let rows = sqlx::query($sql)$(.bind($bind))*.fetch_all($pg).await?;
        if rows.is_empty() {
            Ok(Err(why($pg, $id, $token).await))
        } else {
            Ok(Ok(()))
        }
    }};
}

impl RunStore for PgRunStore {
    fn insert<'a>(&'a self, row: NewRun) -> BoxFuture<'a, Result<RunRow, sqlx::Error>> {
        async move {
            let inserted = sqlx::query(sqlx::AssertSqlSafe(format!(
                "insert into runs (id, kind, owner_user_id, subject_type, subject_id, state, \
                 phase, input) \
                 values ($1::uuid, $2, $3::uuid, $4, $5, 'queued', $6, $7) \
                 returning {COLS}"
            )))
            .bind(&row.id)
            .bind(&row.kind)
            .bind(&row.owner_user_id)
            .bind(&row.subject_type)
            .bind(&row.subject_id)
            .bind(&row.phase)
            .bind(&row.input)
            .fetch_one(&self.pg)
            .await
            .map_err(|e| {
                // Name the kind on the error: the insert failing carries it in
                // its Postgres text, but a caller
                // staring at a bare constraint violation cannot tell which
                // enqueue died. One sentence, same failure.
                sqlx::Error::Protocol(format!("[runs] insert of {}: {e}", row.kind))
            })?;
            hydrate(&inserted)
        }
        .boxed()
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<RunRow>, sqlx::Error>> {
        async move { get_pg(&self.pg, id).await }.boxed()
    }

    fn claim<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        lease_ms: i64,
    ) -> BoxFuture<'a, Result<ClaimOutcome, sqlx::Error>> {
        async move {
            // ONE statement. The predicate says exactly what "takeable" means:
            //   · queued, and no live lease on it (a fresh run, or one deferred
            //     by a `retry` whose wait has elapsed)
            //   · running, and the lease has EXPIRED — the driver that had it
            //     is gone. That, and only that, is a reclaim, and it is the
            //     only branch that touches `attempt`. A healthy run taking
            //     four hundred steps never passes through here again, so it
            //     cannot exhaust `max_attempts` by succeeding.
            // `started_at` is stamped once and never moved: it is when the
            // WORK began, not when this driver picked it up, and a resumed
            // run whose start time jumps forward is a run whose age no
            // queue-depth graph can read.
            //
            // The CTE is not decoration. `returning` reads the NEW row, so the
            // state this run was in BEFORE the flip — the one thing that
            // distinguishes a reclaim from a first pickup — is unreadable from
            // a plain update. The `for update` row lock also serializes two
            // instances claiming the same run in the same millisecond, so
            // exactly one of them sees `queued` and the other sees a live
            // lease.
            let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "with prev as ( \
                   select id as pid, state as prev_state from runs where id = $1::uuid for update \
                 ) \
                 update runs set \
                   state = 'running', \
                   attempt = runs.attempt + case when prev.prev_state = 'running' then 1 else 0 end, \
                   lease_owner = $2, \
                   lease_expires_at = now() + make_interval(secs => $3::float8), \
                   started_at = coalesce(runs.started_at, now()), \
                   updated_at = now() \
                 from prev \
                 where runs.id = prev.pid \
                   and prev.prev_state in ('queued', 'running') \
                   and (runs.lease_expires_at is null or runs.lease_expires_at <= now()) \
                 returning {COLS}, (prev.prev_state = 'running') as was_running"
            )))
            .bind(id)
            .bind(token)
            .bind(lease_ms.max(1) as f64 / 1000.0)
            .fetch_all(&self.pg)
            .await?;
            if let Some(row) = rows.first() {
                return Ok(ClaimOutcome::Claimed {
                    run: Box::new(hydrate(row)?),
                    reclaimed: row.try_get::<bool, _>("was_running")?,
                });
            }
            let current = match get_pg(&self.pg, id).await? {
                Some(run) => run,
                None => return Ok(ClaimOutcome::Missing),
            };
            if matches!(current.state, RunState::Queued | RunState::Running) {
                return Ok(ClaimOutcome::Taken {
                    state: current.state,
                    until: current.lease_expires_at,
                });
            }
            Ok(ClaimOutcome::NotRunnable {
                state: current.state,
            })
        }
        .boxed()
    }

    fn heartbeat<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        lease_ms: i64,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        async move {
            // Deliberately does NOT touch `updated_at`: a heartbeat is not
            // progress, and "my active runs, most recently updated first" must
            // not be reordered by a run that has done nothing for an hour but
            // is still breathing.
            cas_write!(
                &self.pg,
                id,
                token,
                "update runs set lease_expires_at = now() + make_interval(secs => $3::float8) \
                 where id = $1::uuid and lease_owner = $2 and state = 'running' returning id",
                id,
                token,
                lease_ms.max(1) as f64 / 1000.0,
            )
        }
        .boxed()
    }

    fn checkpoint<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        checkpoint: Value,
        phase: String,
        clear_decision: bool,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        async move {
            cas_write!(
                &self.pg,
                id,
                token,
                "update runs set \
                   checkpoint = $3, phase = $4, \
                   decision = case when $5 then null else decision end, \
                   approval_key = case when $5 then null else approval_key end, \
                   updated_at = now() \
                 where id = $1::uuid and lease_owner = $2 and state = 'running' returning id",
                id,
                token,
                checkpoint,
                phase,
                clear_decision,
            )
        }
        .boxed()
    }

    fn phase<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        phase: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        async move {
            cas_write!(
                &self.pg,
                id,
                token,
                "update runs set phase = $3, updated_at = now() \
                 where id = $1::uuid and lease_owner = $2 and state = 'running' returning id",
                id,
                token,
                phase,
            )
        }
        .boxed()
    }

    fn complete<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        result: Value,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        async move {
            // The lease is dropped in the same statement that finishes the
            // run. Two statements would leave a window where a done run still
            // looks leased, and the reclaim query would have to special-case
            // terminal states forever.
            cas_write!(
                &self.pg,
                id,
                token,
                "update runs set \
                   state = 'done', result = $3, error = null, \
                   decision = null, approval_key = null, \
                   lease_owner = null, lease_expires_at = null, \
                   finished_at = now(), updated_at = now() \
                 where id = $1::uuid and lease_owner = $2 and state = 'running' returning id",
                id,
                token,
                result,
            )
        }
        .boxed()
    }

    fn fail<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        error: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        async move {
            let error = clamp_error(&error);
            cas_write!(
                &self.pg,
                id,
                token,
                "update runs set \
                   state = 'error', error = $3, \
                   lease_owner = null, lease_expires_at = null, \
                   finished_at = now(), updated_at = now() \
                 where id = $1::uuid and lease_owner = $2 and state = 'running' returning id",
                id,
                token,
                error,
            )
        }
        .boxed()
    }

    fn park<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        decision: RunDecision,
        approval_key: String,
        phase: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        async move {
            // The lease is released here too. An `awaiting` run is not being
            // driven by anybody, and a lease held across a wait for a human
            // would either expire (making the row look reclaimable when it is
            // not) or have to be renewed by a process with nothing to do.
            let decision = decision_value(&decision)?;
            cas_write!(
                &self.pg,
                id,
                token,
                "update runs set \
                   state = 'awaiting', decision = $3, approval_key = $4, phase = $5, \
                   lease_owner = null, lease_expires_at = null, updated_at = now() \
                 where id = $1::uuid and lease_owner = $2 and state = 'running' returning id",
                id,
                token,
                decision,
                approval_key,
                phase,
            )
        }
        .boxed()
    }

    fn defer<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        until_ms: i64,
        reason: String,
    ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
        async move {
            // Back to `queued`, but the lease stamp stays in the FUTURE, which
            // is what makes the wait real: `claim` refuses anything with a
            // live lease, so no instance takes this run before `until` —
            // including this one. The lease OWNER stays set as well, so the
            // row says who deferred it.
            cas_write!(
                &self.pg,
                id,
                token,
                "update runs set \
                   state = 'queued', phase = $4, \
                   lease_expires_at = to_timestamp($3::float8), \
                   updated_at = now() \
                 where id = $1::uuid and lease_owner = $2 and state = 'running' returning id",
                id,
                token,
                (until_ms / 1000) as f64,
                reason,
            )
        }
        .boxed()
    }

    fn release<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        async move {
            // Only from `running`: a run this driver parked, deferred or
            // finished has already had its lease dealt with, and clearing it
            // again would strip the deferral wait off a `retry`.
            sqlx::query(
                "update runs set lease_owner = null, lease_expires_at = null \
                 where id = $1::uuid and lease_owner = $2 and state = 'running'",
            )
            .bind(id)
            .bind(token)
            .execute(&self.pg)
            .await?;
            Ok(())
        }
        .boxed()
    }

    fn answer<'a>(
        &'a self,
        id: &'a str,
        answer: DecisionAnswer,
    ) -> BoxFuture<'a, Result<AnswerOutcome, sqlx::Error>> {
        async move {
            let current = match get_pg(&self.pg, id).await? {
                Some(run) => run,
                None => return Ok(AnswerOutcome::Missing),
            };
            if current.state != RunState::Awaiting {
                return Ok(AnswerOutcome::NotAwaiting {
                    state: Some(current.state),
                });
            }
            // An answer names the question it answers. Without this check, an
            // answer submitted from a stale tab would resume the run with the
            // wrong decision, and the step would read it as an answer to the
            // question it is actually waiting on.
            let request = match &current.decision {
                Some(d) if d.request.key == answer.key => d.request.clone(),
                _ => {
                    return Ok(AnswerOutcome::StaleKey {
                        state: current.state,
                    });
                }
            };
            let decision = decision_value(&RunDecision {
                request,
                answer: Some(answer),
            })?;
            let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "update runs set \
                   state = 'queued', decision = $2, \
                   lease_owner = null, lease_expires_at = null, updated_at = now() \
                 where id = $1::uuid and state = 'awaiting' \
                 returning {COLS}"
            )))
            .bind(id)
            .bind(decision)
            .fetch_all(&self.pg)
            .await?;
            // Lost a race with another answer of the same question. Not an
            // error worth raising to the person: somebody answered it, which
            // is what they wanted.
            match rows.first() {
                Some(row) => Ok(AnswerOutcome::Answered(Box::new(hydrate(row)?))),
                None => Ok(AnswerOutcome::NotAwaiting { state: None }),
            }
        }
        .boxed()
    }

    fn cancel<'a>(
        &'a self,
        id: &'a str,
        reason: Option<String>,
    ) -> BoxFuture<'a, Result<CancelOutcome, sqlx::Error>> {
        async move {
            // NO LEASE PREDICATE, and that is the entire point — see the
            // trait method. The driver that owns the run finds out at its
            // next step boundary, or when its next write is refused.
            let rows = sqlx::query(
                "update runs set \
                   state = 'cancelled', error = $2, \
                   lease_owner = null, lease_expires_at = null, \
                   finished_at = now(), updated_at = now() \
                 where id = $1::uuid and state in ('queued', 'running', 'awaiting') \
                 returning state",
            )
            .bind(id)
            .bind(reason)
            .fetch_all(&self.pg)
            .await?;
            if let Some(row) = rows.first() {
                return Ok(CancelOutcome::Cancelled {
                    state: decode_state(&row.try_get::<String, _>("state")?)?,
                });
            }
            match get_pg(&self.pg, id).await? {
                None => Ok(CancelOutcome::Missing),
                Some(run) => Ok(CancelOutcome::Terminal { state: run.state }),
            }
        }
        .boxed()
    }

    fn due<'a>(&'a self, limit: i64) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
        async move {
            // THE reclaim query, and one of the two the indexes exist for.
            // Oldest expiry first so a queue that has fallen behind drains in
            // the order it fell behind, rather than starving whatever has
            // been waiting longest.
            let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "select {COLS} from runs \
                 where state in ('queued', 'running') \
                   and (lease_expires_at is null or lease_expires_at <= now()) \
                 order by lease_expires_at asc nulls first, created_at asc \
                 limit $1"
            )))
            .bind(limit.max(1))
            .fetch_all(&self.pg)
            .await?;
            rows.iter().map(hydrate).collect()
        }
        .boxed()
    }

    fn active_for<'a>(
        &'a self,
        user_id: &'a str,
        limit: Option<i64>,
    ) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
        async move {
            // The other real query: what this person has in flight, newest
            // activity first. `awaiting` is in the list because a run parked
            // on a question the user has to answer is the MOST active thing
            // they have.
            let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "select {COLS} from runs \
                 where owner_user_id = $1::uuid and state in ('queued', 'running', 'awaiting') \
                 order by updated_at desc \
                 limit $2"
            )))
            .bind(user_id)
            .bind(limit.unwrap_or(50).max(1))
            .fetch_all(&self.pg)
            .await?;
            rows.iter().map(hydrate).collect()
        }
        .boxed()
    }
}

// ── The latest run of a KIND ─────────────────────────────────────────────────
//
// The third real query, and it lives here rather than beside any one
// definition because it is what EVERY ownerless run needs: `due` answers "what
// is nobody driving", `active_for` answers "what has this person got in
// flight", and this answers "what is the admin panel showing". A fitness
// sweep, a retrieval backfill and a reindex are all nobody's run about
// nothing, so the kind is the only handle they have.
//
// PLAIN FUNCTIONS RATHER THAN `RunStore` METHODS, deliberately — `RunStore` is
// the compare-and-set surface the DRIVER writes through, and a read that no
// driver performs does not belong in that contract.

/// The projection an org-wide status panel reads. Narrow on purpose: the
/// panels need these nine columns, and handing back the row shape the driver
/// uses would invite a caller to write through it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindRunView {
    pub id: String,
    pub state: RunState,
    pub phase: String,
    pub input: Value,
    pub checkpoint: Value,
    pub result: Value,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

pub async fn latest_run_of_kind(
    pg: &PgPool,
    kind: &str,
) -> Result<Option<KindRunView>, sqlx::Error> {
    let row = sqlx::query(
        "select id::text, state, phase, input, checkpoint, result, error, \
                (trunc(extract(epoch from started_at) * 1000))::bigint as started_ms, \
                (trunc(extract(epoch from finished_at) * 1000))::bigint as finished_ms \
         from runs where kind = $1 \
         order by created_at desc limit 1",
    )
    .bind(kind)
    .fetch_optional(pg)
    .await?;
    row.map(|r| {
        Ok(KindRunView {
            id: r.try_get("id")?,
            state: decode_state(&r.try_get::<String, _>("state")?)?,
            phase: r.try_get("phase")?,
            input: r
                .try_get::<Option<Value>, _>("input")?
                .unwrap_or(Value::Null),
            checkpoint: r
                .try_get::<Option<Value>, _>("checkpoint")?
                .unwrap_or(Value::Null),
            result: r
                .try_get::<Option<Value>, _>("result")?
                .unwrap_or(Value::Null),
            error: r.try_get("error")?,
            started_at: r
                .try_get::<Option<i64>, _>("started_ms")?
                .map(epoch_ms_to_iso),
            finished_at: r
                .try_get::<Option<i64>, _>("finished_ms")?
                .map(epoch_ms_to_iso),
        })
    })
    .transpose()
}

/// The run this kind is currently doing, or None. `awaiting` counts as active —
/// a run parked on a question is not finished, however long it sits.
pub async fn active_run_of_kind(
    pg: &PgPool,
    kind: &str,
) -> Result<Option<KindRunView>, sqlx::Error> {
    let latest = latest_run_of_kind(pg, kind).await?;
    Ok(latest.filter(|r| !is_terminal(r.state)))
}

// ── Pure unit tests ──────────────────────────────────────────────────────────
//
// The Postgres surface is proven by the `#[ignore]`d live-DB tests in
// tests/runs_store.rs (house rule: no service-dependent tests in CI). What is
// pure and belongs here: the string helpers whose correctness the SQL depends
// on, and the state decode the outcomes turn on.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_clamp_lands_on_a_char_boundary() {
        // 4000 ASCII bytes pass through untouched; a multibyte char straddling
        // the cut must not panic — the clamp lands on a char boundary.
        assert_eq!(clamp_error(&"x".repeat(4000)).len(), 4000);
        assert_eq!(clamp_error(&"x".repeat(4001)).len(), 4000);

        let emoji = "é".repeat(2100); // 4200 bytes of 2-byte chars
        let clamped = clamp_error(&emoji);
        assert!(clamped.len() <= 4000 && clamped.len().is_multiple_of(2));
        assert!(clamped.chars().all(|c| c == 'é'));

        // A 4-byte char sitting exactly at the boundary is dropped whole.
        let straddling = format!("{}{}x", "y".repeat(3998), "😀");
        let clamped = clamp_error(&straddling);
        assert_eq!(clamped, "y".repeat(3998));
    }

    #[test]
    fn unknown_states_decode_loudly() {
        assert_eq!(decode_state("awaiting").unwrap(), RunState::Awaiting);
        let err = decode_state("paused").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("paused"),
            "the error must name the state: {msg}"
        );
    }
}
