// Conversations — the port of ui/src/server/conversations.ts, grown slice by
// slice. This file starts with the one piece the runs watch gate needs
// (realtime.rs resolves a run with a `conversation` subject through
// `accessibleConversation`); the message, plan, and title planes land with the
// chat family's own batch.

use sqlx::PgPool;

/// conversations.ts accessibleConversation, reduced to the question its
/// callers that only gate ask: does this row exist for this person? TS
/// selects five columns and answers `rows.length`; the WHERE clause is the
/// whole rule and is ported verbatim — the OWNER, or (a PLAN, and a member):
/// a chat does not admit collaborators, and that distinction is decided here
/// and nowhere else. Re-deciding it at a call site would be a second answer.
pub async fn conversation_accessible(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from conversations c \
         where c.id = $1::uuid and c.kind in ('chat', 'plan') \
           and (c.user_id = $2::uuid or (c.kind = 'plan' and exists( \
             select 1 from conversation_members cm \
             where cm.conversation_id = c.id and cm.user_id = $2::uuid))) \
         limit 1",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}
