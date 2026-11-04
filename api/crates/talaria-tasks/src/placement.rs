// THE ONE DOOR FOR A TICKET'S COLUMN.
//
// The owner's rule, stated more than once and broken every time by a new side
// door: a ticket changes column only when it is TOLD to.
//
//   (a) a person moves it — a drag, the status menu, a review's rejection, a
//       column they delete or recategorise, an approval card they approve;
//   (b) an agent moves it with its own move or update tool, because a person
//       asked it to — which the server reads as "not as part of a work
//       session on that ticket", since a session is Talaria talking, not a
//       person;
//   (c) a step of an approved, running workchain moves it — a person built
//       the chain, it is not paused, and this step's turn has come.
//
// Nothing else moves a ticket: not assigning it, not a work session starting,
// progressing or finishing, not a PR opening or merging or being reviewed, not
// triage, a sweep, a scheduler job or an app trigger. Progress is recorded as
// activity, a label or a field (`outcome`, `error_message`) — never a column.
//
// [`set_status`] is the only statement in the api that writes
// `tasks.status`, and it cannot be called without a [`StatusMover`], which
// has exactly those three kinds. `bun run check` fails any other
// `update tasks set … status =` (scripts/check-invariants.mjs,
// `ticket-status-one-writer`).
//
// THE MOVER IS ALSO THE RELEASE. Every placement records who made it
// (`tasks.placed_by`, `placed_by_kind`, `placed_at`), and an agent picks a
// ticket up only from a pickup column one of the three placed it in
// ([`pickup_refusal`]). Assignment alone never starts work, and a placement
// is spent once a session on it has finished: the same ticket is not picked
// up again until somebody allowed places it again.

use std::collections::HashMap;

use sqlx::{PgConnection, PgPool};
use talaria_statuses::{StatusMeta, status_meta};

/// Who is changing a ticket's column. There is no fourth kind — the platform,
/// a sweep, a trigger or a PR has no constructor here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusMover {
    /// (a) A person's explicit move. The id is the activity actor the route
    /// writes for them (an email, a name, or a user id).
    Person(String),
    /// (b) An agent's own tool call outside any work session on the ticket —
    /// a conversation where a person asked for the move. The id is the
    /// agent's model.
    Agent(String),
    /// (c) A step of an approved, running workchain — the agent working the
    /// step moves its ticket on the chain's behalf.
    Workchain { chain_id: String, actor: String },
}

impl StatusMover {
    /// The `tasks.placed_by_kind` spelling (the migration's check constraint
    /// lists the same three).
    pub fn kind(&self) -> &'static str {
        match self {
            StatusMover::Person(_) => "person",
            StatusMover::Agent(_) => "agent",
            StatusMover::Workchain { .. } => "workchain",
        }
    }

    /// Who the activity line names.
    pub fn actor(&self) -> &str {
        match self {
            StatusMover::Person(id) | StatusMover::Agent(id) => id,
            StatusMover::Workchain { actor, .. } => actor,
        }
    }
}

/// Every `placed_by_kind` a pickup accepts — all three, because every
/// allowed mover releases the ticket. Null (nobody placed it) does not.
pub const RELEASING_KINDS: &[&str] = &["person", "agent", "workchain"];

/// The refusal an agent gets for moving the ticket its own work session is
/// on. Said to the agent as a tool error, so it names the thing to do instead.
pub const SESSION_NO_MOVE: &str = "a work session never moves its ticket — record the result with report_outcome (or a comment if you are blocked) and a person moves it";

/// The refusal any caller without a mover gets: the platform, a sweep, a
/// trigger.
pub const NO_MOVER: &str = "only a person, an agent a person asked, or a running workchain moves a ticket — this change is not one of them";

/// THE status write. Writes the column and who placed it there, nothing
/// else: the activity line, the notices and the push side stay the caller's
/// (update_task, the review decision, a move between boards), so this is the
/// one statement and not a second write path. `expect_from` makes the write
/// conditional on the column the caller decided from; `false` means the
/// ticket had moved (or is gone) and nothing was written.
pub async fn set_status(
    conn: &mut PgConnection,
    task_id: &str,
    expect_from: Option<&str>,
    to: &str,
    mover: &StatusMover,
) -> Result<bool, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "update tasks set status = $2, placed_by = $3, placed_by_kind = $4, \
         placed_at = now(), updated_at = now() \
         where id = $1::uuid and ($5::text is null or status = $5) returning id::text",
    )
    .bind(task_id)
    .bind(to)
    .bind(mover.actor())
    .bind(mover.kind())
    .bind(expect_from)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.is_some())
}

/// Is a work session live on this ticket for this agent — queued, running,
/// or parked on a person. That is the one context in which an agent's write
/// is Talaria's doing rather than a person's ask.
pub async fn live_session(pg: &PgPool, task_id: &str, agent: &str) -> Result<bool, sqlx::Error> {
    let live: Option<(i32,)> = sqlx::query_as(
        "select 1 from runs where kind = 'work-session' and subject_id = $1 \
           and state in ('queued', 'running', 'awaiting') and input->>'agentModel' = $2 limit 1",
    )
    .bind(task_id)
    .bind(agent)
    .fetch_optional(pg)
    .await?;
    Ok(live.is_some())
}

/// The chain this ticket is a RUNNING step of, if any: a step of an unpaused
/// workchain whose turn has come (every predecessor done — `Ready` in the
/// one readiness derive). A chain is a person's construction (only the board
/// routes create one), so a running one is an approved one.
pub async fn running_chain_step(
    pg: &PgPool,
    task_id: &str,
    board_id: &str,
    meta: &StatusMeta,
) -> Result<Option<String>, sqlx::Error> {
    let chain: Option<(String,)> = sqlx::query_as(
        "select w.id::text from task_workchain_steps s \
           join task_workchains w on w.id = s.workchain_id \
          where s.task_id = $1::uuid and not w.paused limit 1",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    let Some((chain_id,)) = chain else {
        return Ok(None);
    };
    let metas = HashMap::from([(board_id.to_string(), meta.clone())]);
    let readiness = talaria_workchains::chain_readiness(pg, &metas, &[task_id.to_string()]).await?;
    Ok((readiness.get(task_id) == Some(&talaria_workchains::Readiness::Ready)).then_some(chain_id))
}

/// The mover an agent's status write counts as, or the refusal. A step of a
/// running workchain is (c), whatever else is true. Otherwise a write made
/// while the agent's own work session is live on the ticket is the session's
/// side effect and refused; any other is (b).
pub async fn agent_mover(
    pg: &PgPool,
    task_id: &str,
    board_id: &str,
    meta: &StatusMeta,
    agent: &str,
) -> Result<Result<StatusMover, &'static str>, sqlx::Error> {
    if let Some(chain_id) = running_chain_step(pg, task_id, board_id, meta).await? {
        return Ok(Ok(StatusMover::Workchain {
            chain_id,
            actor: agent.to_string(),
        }));
    }
    if live_session(pg, task_id, agent).await? {
        return Ok(Err(SESSION_NO_MOVE));
    }
    Ok(Ok(StatusMover::Agent(agent.to_string())))
}

/// THE PICKUP GATE's placement half: why `agent` may NOT start work on this
/// ticket, or None when it may. The column half (is it a pickup column at
/// all) and the authority half (board policy, archived, the chain order)
/// are dispatch's own questions, asked before this.
///
///   released  an allowed mover placed it where it is — or it is a running
///             workchain step, which a person approved by building the chain
///   unspent   no work session of this agent's on it has finished since that
///             placement — a finished session spent it, and only a new
///             placement releases it again (an errored one did not finish
///             the work, so the self-heal sweep may start it again)
pub async fn pickup_refusal(
    pg: &PgPool,
    task_id: &str,
    agent: &str,
) -> Result<Option<&'static str>, sqlx::Error> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "select board_id::text, placed_by_kind, placed_at::text from tasks where id = $1::uuid",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    let Some((board_id, kind, placed_at)) = row else {
        return Ok(Some("the ticket is gone"));
    };
    let released = kind
        .as_deref()
        .is_some_and(|k| RELEASING_KINDS.contains(&k));
    if !released {
        let meta = status_meta(pg, &board_id).await?;
        if running_chain_step(pg, task_id, &board_id, &meta)
            .await?
            .is_none()
        {
            return Ok(Some(
                "nobody placed this ticket in its pickup column — a person moves it there (assignment alone never starts work)",
            ));
        }
    }
    let spent: Option<(i32,)> = sqlx::query_as(
        "select 1 from runs where kind = 'work-session' and subject_id = $1 \
           and input->>'agentModel' = $2 and state = 'done' \
           and ($3::timestamptz is null or updated_at >= $3::timestamptz) limit 1",
    )
    .bind(task_id)
    .bind(agent)
    .bind(placed_at)
    .fetch_optional(pg)
    .await?;
    if spent.is_some() {
        return Ok(Some(
            "a work session on this placement already finished — a person moves the ticket again to start another",
        ));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_kinds_are_the_constraint_and_the_release() {
        let movers = [
            StatusMover::Person("ada@example.invalid".into()),
            StatusMover::Agent("doug-engineering".into()),
            StatusMover::Workchain {
                chain_id: "c".into(),
                actor: "doug-engineering".into(),
            },
        ];
        let kinds: Vec<&str> = movers.iter().map(StatusMover::kind).collect();
        assert_eq!(kinds, RELEASING_KINDS);
        assert_eq!(movers[2].actor(), "doug-engineering");
    }
}
