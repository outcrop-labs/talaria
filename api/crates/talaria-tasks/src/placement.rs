// THE ONE DOOR FOR A TICKET'S COLUMN.
//
// The owner's rule, stated more than once and broken every time by a new side
// door: a ticket changes column only when it is TOLD to.
//
//   (a) a person moves it — a drag, the status menu, a review's rejection, a
//       column they delete or recategorise;
//   (b) an agent a person asked moves it — which means THE APPROVAL PATH: an
//       agent's own status write is refused, in a chat that refusal becomes
//       an approval card (talaria-chips `ticket_move`), and the person's
//       click makes the move as that person. An agent's own call outside its
//       session is not evidence that anybody asked;
//   (c) an approved, running workchain — a person built the chain, it is not
//       paused, and this step's turn has come. A step definition names no
//       column, so the chain moves nothing itself: its approval RELEASES its
//       ready steps for pickup ([`running_chain_step`]), and the step's own
//       session is refused like any other.
//
// Nothing else moves a ticket: not assigning it, not a work session starting,
// progressing or finishing, not a PR opening or merging or being reviewed, not
// triage, a sweep, a scheduler job or an app trigger. Progress is recorded as
// activity, a label or a field (`outcome`, `error_message`) — never a column.
//
// [`set_status`] is the only statement in the api that writes
// `tasks.status`. It is crate-private and takes a crate-private
// [`StatusMover`]: outside this crate a column moves only through
// `update_task` (whose person must be a `TaskActor::person` a route built
// from its session), the review decision and the board move. `bun run check`
// fails any other write (scripts/check-invariants.mjs,
// `ticket-status-one-writer`).
//
// THE PLACEMENT IS THE RELEASE. Every status write records who made it
// (`tasks.placed_by` — always a user id for a person, `placed_by_kind`,
// `placed_at`), and an agent picks a ticket up only from a pickup column a
// PERSON placed it in, or as a running chain's ready step ([`pickup_refusal`]).
// Assignment alone never starts work. A placement is SPENT once a session
// that started on it has ended with the agent's report (`report_outcome`:
// an outcome or an error): the same work is not picked up again until a
// person places the ticket again. A session that stopped without a report —
// the turn cap, an error, a restart — spent nothing, so the sweep continues
// it, within the 25-session cap per ticket and agent.

use std::collections::HashMap;

use sqlx::{PgConnection, PgPool};
use talaria_statuses::{StatusMeta, status_meta};

/// THE HELD DECISION (2026-11-04, the owner's to make): may a released
/// ticket's own work session move it forward — In progress at pickup, review
/// or Blocked at the end? `false` is today's answer: it stays where the person
/// put it, every move is a 403, and the result rides `report_outcome`. To
/// switch it on: flip this (agent_mover then records the session's move as
/// kind `agent`, which never releases), and change the three prompt sites that
/// say "never move" — `SESSION_STEP2` and the continue/reconcile prompts in
/// talaria-runs-work-session, and the brief's step 6 in talaria-harness-defs
/// `work_session.rs` — plus `report_outcome` in mcp/src/index.ts.
pub const SESSION_MOVES_ITS_TICKET: bool = false;

/// Who is changing a ticket's column. Crate-private: no other crate can mint
/// one. A person comes only from a `TaskActor::person` (or a board move's
/// `Mover::Human`), whose user id a route read from the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusMover {
    /// (a) A signed-in person; the user id.
    Person(String),
    /// An agent's own move that is allowed but releases nothing: a board move
    /// (talaria-tasks moving.rs), or — only with [`SESSION_MOVES_ITS_TICKET`]
    /// — its session's forward move. The agent's model.
    Agent(String),
}

impl StatusMover {
    /// The `tasks.placed_by_kind` spelling (the migration's check constraint
    /// lists these, and `workchain` for a chain engine that may one day move).
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            StatusMover::Person(_) => "person",
            StatusMover::Agent(_) => "agent",
        }
    }

    /// The `tasks.placed_by` value: a user id, or an agent's model.
    pub(crate) fn placed_by(&self) -> &str {
        match self {
            StatusMover::Person(id) | StatusMover::Agent(id) => id,
        }
    }
}

/// Every `placed_by_kind` that releases a ticket for pickup: a person's. An
/// agent's own move never does — the person's approval of its card does, as
/// that person.
pub const RELEASING_KINDS: &[&str] = &["person"];

/// The refusal an agent gets for moving the ticket its own work session is
/// on. Said to the agent as a tool error, so it names the thing to do instead.
/// "cannot" is load-bearing: the chat turns a refused triage_ticket whose
/// answer says it into an approval card (talaria-chips).
pub const SESSION_NO_MOVE: &str = "a work session cannot move its ticket — record the result with report_outcome (errorMessage if you are blocked) and a person moves it";

/// The refusal for any other agent status write: an agent moves a ticket
/// only through a person's approval.
pub const AGENT_NEEDS_APPROVAL: &str = "agents cannot move a ticket's column themselves — a person moves it, or approves the move card your request shows them in your chat";

/// The refusal for a caller with no mover at all: the platform, a sweep, a
/// trigger.
pub const NO_MOVER: &str = "only a person, an agent's move a person approved, or a running workchain moves a ticket — this change is not one of them";

/// The refusal for a person named on the record but not signed in here
/// (`TaskActor::human`): they can edit fields, not move the column.
pub const PERSON_NOT_SIGNED_IN: &str = "a ticket's column moves only for a signed-in person — this write names a person but carries no session";

/// THE status write. Writes the column and who placed it there, nothing
/// else: the activity line, the notices and the push side stay the caller's
/// (update_task, the review decision, a move between boards), so this is the
/// one statement and not a second write path. `expect_from` makes the write
/// conditional on the column the caller decided from; `false` means the
/// ticket had moved (or is gone) and nothing was written.
pub(crate) async fn set_status(
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
    .bind(mover.placed_by())
    .bind(mover.kind())
    .bind(expect_from)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.is_some())
}

/// Is a work session live on this ticket for this agent — queued, running,
/// or parked on a person.
pub(crate) async fn live_session(
    pg: &PgPool,
    task_id: &str,
    agent: &str,
) -> Result<bool, sqlx::Error> {
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
/// routes create one), so a running one is an approved one, and its ready
/// step is released for pickup.
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

/// The mover an agent's status write counts as, or the refusal. Its own live
/// session on the ticket: refused (unless the held decision is switched on).
/// Anything else: refused — the approval card is how a person asks.
pub(crate) async fn agent_mover(
    pg: &PgPool,
    task_id: &str,
    agent: &str,
) -> Result<Result<StatusMover, &'static str>, sqlx::Error> {
    if live_session(pg, task_id, agent).await? {
        return Ok(if SESSION_MOVES_ITS_TICKET {
            Ok(StatusMover::Agent(agent.to_string()))
        } else {
            Err(SESSION_NO_MOVE)
        });
    }
    Ok(Err(AGENT_NEEDS_APPROVAL))
}

/// Why the pickup gate refuses an agent, in two cases a sweep counts apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickupRefusal {
    /// The ticket is gone.
    Gone,
    /// Nobody allowed placed it where it is, and it is no running chain's
    /// ready step.
    Unreleased,
    /// A session that started on this placement ended with the agent's
    /// report.
    Spent,
}

impl PickupRefusal {
    pub fn message(self) -> &'static str {
        match self {
            PickupRefusal::Gone => "the ticket is gone",
            PickupRefusal::Unreleased => {
                "nobody placed this ticket in its pickup column — a person moves it there (assignment alone never starts work)"
            }
            PickupRefusal::Spent => {
                "the agent already reported on this placement — a person moves the ticket again to start another session"
            }
        }
    }
}

/// THE PICKUP GATE's placement half: why `agent` may NOT start work on this
/// ticket, or None when it may. The column half (is it a pickup column at
/// all) and the authority half (board policy, archived, the chain order)
/// are dispatch's own questions, asked before this.
///
///   released  a person placed it where it is — or it is a running
///             workchain's ready step
///   unspent   no session of this agent's that STARTED on this placement
///             (`created_at`, so a session still ending when the person
///             re-placed the ticket does not swallow the new placement) has
///             ended with the agent's report. A session that stopped without
///             one — the turn cap, an error, a restart — spent nothing.
pub async fn pickup_refusal(
    pg: &PgPool,
    task_id: &str,
    agent: &str,
) -> Result<Option<PickupRefusal>, sqlx::Error> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "select board_id::text, placed_by_kind, placed_at::text from tasks where id = $1::uuid",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    let Some((board_id, kind, placed_at)) = row else {
        return Ok(Some(PickupRefusal::Gone));
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
            return Ok(Some(PickupRefusal::Unreleased));
        }
    }
    let spent: Option<(i32,)> = sqlx::query_as(
        "select 1 from runs r \
          where r.kind = 'work-session' and r.subject_id = $1 \
            and r.input->>'agentModel' = $2 and r.state = 'done' \
            and ($3::timestamptz is null or r.created_at >= $3::timestamptz) \
            and exists (select 1 from task_activity x \
                         where x.task_id = $4::uuid and x.actor = $2 \
                           and x.type in ('outcome', 'error') \
                           and x.created_at >= r.created_at \
                           and x.created_at <= coalesce(r.finished_at, r.updated_at)) \
          limit 1",
    )
    .bind(task_id)
    .bind(agent)
    .bind(placed_at)
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    if spent.is_some() {
        return Ok(Some(PickupRefusal::Spent));
    }
    Ok(None)
}

/// Has `agent` reported (an outcome or an error) on this ticket since its
/// live work session began? The session's own end: a session never moves its
/// ticket, so it ends on the report rather than on a column. Every report
/// counts, the same text again included — update_task writes the report's
/// activity line for an agent whether or not the text changed.
pub async fn session_reported(
    pg: &PgPool,
    task_id: &str,
    agent: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "select exists (select 1 from task_activity a \
           where a.task_id = $1::uuid and a.actor = $2 and a.type in ('outcome', 'error') \
             and a.created_at >= (select max(r.created_at) from runs r \
               where r.kind = 'work-session' and r.subject_id = $3 \
                 and r.input->>'agentModel' = $2 \
                 and r.state in ('queued', 'running', 'awaiting')))",
    )
    .bind(task_id)
    .bind(agent)
    .bind(task_id)
    .fetch_one(pg)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_persons_placement_releases() {
        let person = StatusMover::Person("6b3e…".into());
        let agent = StatusMover::Agent("doug-engineering".into());
        assert!(RELEASING_KINDS.contains(&person.kind()));
        assert!(!RELEASING_KINDS.contains(&agent.kind()));
        assert_eq!(agent.placed_by(), "doug-engineering");
    }

    #[test]
    fn agent_refusals_say_cannot_so_the_chat_offers_the_approval_card() {
        // talaria-chips turns a refused triage_ticket into a ticket_move
        // card only when the answer says "cannot" (or carries an error).
        for refusal in [SESSION_NO_MOVE, AGENT_NEEDS_APPROVAL] {
            assert!(refusal.contains("cannot"), "{refusal}");
        }
    }
}
