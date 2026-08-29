// Tasks — the port of ui/src/server/tasks.ts, grown slice by slice. This file
// starts with the one piece the runs watch gate needs (realtime.rs hops a run
// with a `task` subject to that task's BOARD, because a ticket's read ACL is
// its board's); the CRUD, assignment, and quality planes land with the boards
// family's own batch.

use sqlx::PgPool;

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
