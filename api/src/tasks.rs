// Tasks — the port of ui/src/server/tasks.ts, grown slice by slice. This file
// starts with the two pieces earlier slices needed (realtime.rs hops a run
// with a `task` subject to that task's BOARD, because a ticket's read ACL is
// its board's; the approvals census names a ticket's HUMAN assignees as the
// people first nagged); the CRUD, assignment, and quality planes land with the
// boards family's own batch.

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
