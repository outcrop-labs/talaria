// Tasks — the port of ui/src/server/tasks.ts, grown slice by slice. This file
// starts with the pieces earlier slices needed (realtime.rs hops a run with a
// `task` subject to that task's BOARD, because a ticket's read ACL is its
// board's; the approvals census names a ticket's HUMAN assignees as the people
// first nagged) and the agent-write POLICY plane (the one predicate every
// gate, session and heartbeat asks of a ticket); the CRUD, assignment, and
// quality planes land with the boards family's own batch.

use crate::agent_auth::{AgentSubject, subject_model};
use crate::boards::{board_allows_agent, board_info};
use crate::statuses::status_meta;
use sqlx::PgPool;

/// tasks.ts isHumanAssignee: the assignees array mixes AGENT model ids (bare
/// strings, unchanged — the heartbeat/outreach `@>` predicates keep matching)
/// and HUMANS as `user:<uuid>`.
pub fn is_human_assignee(a: &str) -> bool {
    a.starts_with("user:")
}

/// tasks.ts humanAssigneeIds: the human half of a mixed assignees array, with
/// the `user:` prefix stripped.
pub fn human_assignee_ids(assignees: &[String]) -> Vec<String> {
    assignees
        .iter()
        .filter(|a| is_human_assignee(a))
        .map(|a| a[5..].to_string())
        .collect()
}

/// tasks.ts agentAssignees: the agent half of a mixed assignees array, bare
/// model ids unchanged — the strings the heartbeat's `@>` predicates and the
/// dispatch walk match.
pub fn agent_assignees(assignees: &[String]) -> Vec<String> {
    assignees
        .iter()
        .filter(|a| !is_human_assignee(a))
        .cloned()
        .collect()
}

// ── The agent-write policy plane ─────────────────────────────────────────────
//
// ONE PREDICATE, MANY GATES. The patch gate, the work-session loop, the
// heartbeat and the MCP tool surface all ask the same question — may THIS
// agent act on THIS ticket — and before this consolidation they asked four
// private approximations of it that each omitted a clause. It returns the
// REASON rather than a boolean so the refusal is the same sentence wherever
// the write arrived, and a new caller cannot invent a vaguer one.

/// The ticket shape the predicate needs (TS AgentWriteTarget). `get_task`
/// returns it; so does the row the heartbeat query selects.
pub struct AgentWriteTarget {
    pub board_id: String,
    pub status: String,
    pub archived_at: Option<String>,
}

/// What the agent is trying to do (TS AgentIntent). The two verbs differ in
/// EXACTLY one clause, which is why they are one function and not two:
/// · `Write`   — change the ticket, or hang a record off it (status, fields,
///               dependency edges, spend rows, workbench jobs).
/// · `Comment` — say something on it. A CLOSED ticket still takes comments:
///               that is the agent's channel on work it can no longer edit,
///               and it is deliberate. Archival is not that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentIntent {
    Write,
    Comment,
}

/// Why an agent may not act on this ticket, or None when it may (TS
/// agentTicketRefusal).
///
/// FOUR STOP CONDITIONS, in the order a person would ask them:
/// · the BOARD does not allow this agent — revoked, never granted, or the
///   board is archived/gone;
/// · the ticket is ARCHIVED — a person took this work off the table;
/// · the intent is COMMENT — not a stop at all, the exemption above;
/// · the ticket is CLOSED — a person signed off on this work.
///
/// WHY ARCHIVAL STOPS COMMENTS TOO, at both levels. The comment exemption
/// exists for work "the agent can no longer edit" but a person is still
/// looking at — a signed-off ticket sitting in a done column on a live board.
/// Archival is not that: it withdraws the work from view. Board archival
/// already refused agent comments (it runs through `board_allows_agent`)
/// while ticket archival did not, which is one act of a person's meaning two
/// different things one level apart. Made consistent in the direction that
/// keeps the exemption's stated reason true: closed keeps its channel,
/// archived does not have one, at either level. READS are not this question —
/// an agent that can see the board can read the ticket, because reading
/// changes nothing.
///
/// TS's `facts` argument is a per-pass CACHE, never an answer; the Rust port
/// reads through, and the one board it resolves twice per call is a query the
/// cache existed to save.
pub async fn agent_ticket_refusal(
    pg: &PgPool,
    task: &AgentWriteTarget,
    agent: &AgentSubject,
    intent: AgentIntent,
) -> Result<Option<String>, sqlx::Error> {
    let board = board_info(pg, &task.board_id).await?;
    if !board_allows_agent(pg, &task.board_id, agent).await? {
        if board.exists && board.archived_at.is_some() {
            return Ok(Some(format!(
                "agents cannot work a ticket on an archived board — a person restores {} first",
                board.label
            )));
        }
        return Ok(Some(format!(
            "agent \"{}\" is not allowed on this board",
            subject_model(agent)
        )));
    }
    if task.archived_at.is_some() {
        return Ok(Some(
            "agents cannot work an archived ticket — a person restores it first".into(),
        ));
    }
    if intent == AgentIntent::Comment {
        return Ok(None);
    }
    let meta = status_meta(pg, &task.board_id).await?;
    if meta.terminal(&task.status) {
        return Ok(Some("agents cannot change a closed ticket".into()));
    }
    Ok(None)
}

/// tasks.ts getTask().boardId, and nothing else about the row. The full task
/// read is a wide `TASK_SELECT` join this edge has no use for — it selects
/// exactly the one column the watch gate asks about.
pub async fn task_board_id(pg: &PgPool, task_id: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select board_id::text from tasks where id = $1::uuid")
            .bind(task_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.and_then(|(board_id,)| board_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_assignees_strip_their_prefix_and_leave_agent_ids_alone() {
        let assignees: Vec<String> = ["user:u-1", "claude-opus-4-5", "user:u-2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(human_assignee_ids(&assignees), ["u-1", "u-2"]);
        // The `@>` predicates keep matching bare strings, so an agent id that
        // merely CONTAINS 'user' is not a human.
        assert!(!is_human_assignee("impersonating-user:x"));
        assert!(human_assignee_ids(&[]).is_empty());
    }
}
