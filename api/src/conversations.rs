// Conversations — the port of ui/src/server/conversations.ts, grown slice by
// slice. The gate predicate landed with the runs watch gate (realtime.rs
// resolves a run with a `conversation` subject through it); the agent read
// and priorMessages land with the plan-draft plane, whose transcript is a
// conversation's turns with refs and file bytes re-read. The message, plan,
// and title planes land with the chat family's own batch.

use sqlx::PgPool;

use crate::refs::ref_blocks;
use crate::secretbox::SecretBox;
use crate::uploads::attachment_text_blocks;

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

/// The same predicate with the one column the plan-draft POST needs after it
/// passes: the conversation's own agent, which drafts. None covers both "not
/// accessible" and "accessible but agentless" — TS's `?.agentModel ?? null`
/// reads the same either way, and the route answers 'plan not found' for
/// both, never leaking which.
pub async fn accessible_conversation_agent(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "select agent_model from conversations c \
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
    Ok(row.filter(|(m,)| !m.is_empty()).map(|(m,)| m))
}

/// One prior turn as a transcript line (conversations.ts priorMessages'
/// mapped row): the role the gateway saw, and the content after voice
/// prefixes, ref chips and file bytes are folded in.
#[derive(Debug, Clone)]
pub struct PriorMessage {
    pub role: String, // 'user' | 'assistant'
    pub content: String,
}

/// Prior turns (role + content) for the gateway, oldest first
/// (conversations.ts priorMessages). Multiplayer plans prefix user turns
/// with the author's name so the agent can tell voices apart; 1:1 threads
/// stay plain. Attached knowledge/artifact refs ride along on every rebuild
/// — a queued turn or resume never loses them — while textual file uploads
/// re-read their bytes for the RECENT TAIL only (FILE_TAIL = 12).
///
/// A message row a non-array attachments value cannot crash this walk: the
/// ref/file legs treat it as nothing, which is what the TS shape checks do.
pub async fn prior_messages(
    pg: &PgPool,
    sb: &SecretBox,
    conversation_id: &str,
) -> Result<Vec<PriorMessage>, sqlx::Error> {
    struct Row {
        role: String,
        content: String,
        attachments: serde_json::Value,
        author_label: Option<String>,
        members: i32,
    }
    let rows: Vec<Row> =
        sqlx::query_as::<_, (String, String, serde_json::Value, Option<String>, i32)>(
            "select m.role, m.content, m.attachments, coalesce(u.name, u.email), \
                (select count(*)::int from conversation_members cm \
                 where cm.conversation_id = m.conversation_id) \
         from messages m left join users u on u.id = m.author_user_id \
         where m.conversation_id = $1::uuid and m.role in ('user','assistant') \
           and (m.content <> '' or m.attachments <> '[]'::jsonb) \
         order by m.seq asc",
        )
        .bind(conversation_id)
        .fetch_all(pg)
        .await?
        .into_iter()
        .map(|(role, content, attachments, author_label, members)| Row {
            role,
            content,
            attachments,
            author_label,
            members,
        })
        .collect();

    let multi_voice = rows.iter().any(|r| r.members > 0);
    // Textual file uploads re-read their bytes on every rebuild — scope that
    // to the recent tail; ref chips carry their content inline and always
    // ride. The bound is the row count AFTER the SQL filter, exactly TS's
    // `i >= rows.length - FILE_TAIL`.
    const FILE_TAIL: usize = 12;
    let total = rows.len();
    let mut out: Vec<PriorMessage> = Vec::with_capacity(total);
    for (i, r) in rows.iter().enumerate() {
        let user = r.role == "user";
        let voiced = if multi_voice && user && r.author_label.is_some() {
            format!("{}: {}", r.author_label.as_deref().unwrap(), r.content)
        } else {
            r.content.clone()
        };
        let files = if user && i >= total.saturating_sub(FILE_TAIL) {
            attachment_text_blocks(pg, sb, &r.attachments, usize::MAX).await
        } else {
            String::new()
        };
        let refs = if user {
            ref_blocks(&r.attachments)
        } else {
            String::new()
        };
        out.push(PriorMessage {
            role: r.role.clone(),
            content: format!("{voiced}{refs}{files}"),
        });
    }
    Ok(out
        .into_iter()
        .filter(|m| !m.content.trim().is_empty())
        .collect())
}
