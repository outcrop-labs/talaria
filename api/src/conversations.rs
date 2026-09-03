// Conversations — the access gates (realtime.rs resolves a run with a
// `conversation` subject through the same predicate), prior-turn transcripts
// for the gateway, the listing/detail wire views, plan membership, the
// message-row plumbing a streamed reply writes through, and the title
// defaults.

use serde_json::Value;
use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;
use crate::gateway::fleet_chat::ToolCall;
use crate::refs::ref_blocks;
use crate::secretbox::SecretBox;
use crate::uploads::attachment_text_blocks;

/// The access gate, reduced to the question its callers that only gate ask:
/// does this row exist for this person? The WHERE clause is the whole rule —
/// the OWNER, or (a PLAN, and a member): a chat does not admit
/// collaborators, and that distinction is decided here and nowhere else.
/// Re-deciding it at a call site would be a second answer.
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
/// accessible" and "accessible but agentless", and the route answers
/// 'plan not found' for both, never leaking which.
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

/// One prior turn as a transcript line: the role the gateway saw, and the
/// content after voice
/// prefixes, ref chips and file bytes are folded in.
#[derive(Debug, Clone)]
pub struct PriorMessage {
    pub role: String, // 'user' | 'assistant'
    pub content: String,
}

/// Prior turns (role + content) for the gateway, oldest first. Multiplayer
/// plans prefix user turns
/// with the author's name so the agent can tell voices apart; 1:1 threads
/// stay plain. Attached knowledge/artifact refs ride along on every rebuild
/// — a queued turn or resume never loses them — while textual file uploads
/// re-read their bytes for the RECENT TAIL only (FILE_TAIL = 12).
///
/// A message row a non-array attachments value cannot crash this walk: the
/// ref/file legs treat it as nothing.
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
    // ride. The bound is the row count AFTER the SQL filter.
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
            attachment_text_blocks(pg, sb, &r.attachments, 3).await
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

/// The full row the chat route gates and reads on: whose conversation this
/// is, which kind, which agent, and the caller's standing. Chats never admit
/// collaborators; that rule lives in the WHERE clause.
#[derive(Debug)]
pub struct AccessibleConversation {
    pub agent_model: String,
    pub kind: String, // 'chat' | 'plan'
    pub title: Option<String>,
    pub owner_user_id: String,
    pub role: String, // 'owner' | 'collaborator'
    pub plan_template_id: Option<String>,
}

/// The accessible-conversation select, as one named tuple.
type AccessibleRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
);

pub async fn accessible_conversation(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<AccessibleConversation>, sqlx::Error> {
    let row: Option<AccessibleRow> = sqlx::query_as(
        "select agent_model, kind, title, user_id::text, \
                    case when user_id = $2::uuid then 'owner' else 'collaborator' end, \
                    plan_template_id::text \
             from conversations c \
             where id = $1::uuid and kind in ('chat', 'plan') \
               and (user_id = $2::uuid or (kind = 'plan' and exists( \
                 select 1 from conversation_members cm \
                 where cm.conversation_id = c.id and cm.user_id = $2::uuid)))",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(
        |(agent_model, kind, title, owner_user_id, role, plan_template_id)| {
            AccessibleConversation {
                agent_model,
                kind,
                title,
                owner_user_id,
                role,
                plan_template_id,
            }
        },
    ))
}

// ── The listing/detail wire views ─────────────────────────────────────────────
//
// Wire key order follows the SQL select order — the struct fields are
// declared to match it: the sidebar row, and the detail's conversation +
// message rows.

/// The sidebar's view of a thread — the listing row.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationListRow {
    pub id: String,
    pub agent_model: String,
    pub title: Option<String>,
    pub updated_at: String,
    /// An assistant reply is streaming right now (the agent is working).
    pub working: bool,
    /// The LATEST assistant turn errored — the thread needs a re-run.
    pub failed: bool,
    pub role: String,
    /// For plans shared WITH you: who owns it (display label).
    pub owner_label: Option<String>,
    /// Landed messages past the caller's read cursor — assistant replies, or
    /// someone else's turn in a shared plan. Your own turn never counts: you
    /// wrote it, and a sent message ringing its own author's badge is the
    /// oldest badge bug there is. The no-cursor floor is -1 (conversations
    /// number from seq 0 — unlike channels, whose counter starts at 1), so a
    /// reader who has never opened the thread sees its very first message.
    pub unread_count: i32,
}

/// The user's conversations of a given kind, newest activity first. Plans
/// are multiplayer: your own AND
/// ones shared with you; chats stay strictly your own.
pub async fn list_conversations(
    pg: &PgPool,
    user_id: &str,
    kind: &str,
) -> Result<Vec<ConversationListRow>, sqlx::Error> {
    #[allow(clippy::type_complexity)] // the select's own columns, one each
    type Row = (
        String,
        String,
        Option<String>,
        i64,
        bool,
        bool,
        String,
        Option<String>,
        i32,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "select c.id::text, c.agent_model, c.title, \
                (trunc(extract(epoch from c.updated_at) * 1000))::bigint, \
                exists( \
                  select 1 from messages m \
                  where m.conversation_id = c.id and m.role = 'assistant' \
                    and m.status = 'streaming' and m.created_at > now() - interval '10 minutes' \
                ), \
                coalesce(( \
                  select m.status = 'error' from messages m \
                  where m.conversation_id = c.id and m.role = 'assistant' \
                  order by m.seq desc limit 1 \
                ), false), \
                case when c.user_id = $1::uuid then 'owner' else 'collaborator' end, \
                case when c.user_id = $1::uuid then null else coalesce(o.name, o.email) end, \
                coalesce(( \
                  select count(*)::int from messages m \
                  where m.conversation_id = c.id \
                    and m.seq > coalesce(cr.last_read_seq, -1) \
                    and m.status = 'complete' \
                    and (m.role = 'assistant' or m.author_user_id is distinct from $1::uuid) \
                ), 0) \
         from conversations c \
         join users o on o.id = c.user_id \
         left join conversation_reads cr on cr.conversation_id = c.id and cr.user_id = $1::uuid \
         where c.archived = false and c.kind = $2 \
           and (c.user_id = $1::uuid or ($2 = 'plan' and exists( \
             select 1 from conversation_members cm \
             where cm.conversation_id = c.id and cm.user_id = $1::uuid))) \
         order by c.updated_at desc",
    )
    .bind(user_id)
    .bind(kind)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                agent_model,
                title,
                updated_ms,
                working,
                failed,
                role,
                owner_label,
                unread_count,
            )| {
                ConversationListRow {
                    id,
                    agent_model,
                    title,
                    updated_at: epoch_ms_to_iso(updated_ms),
                    working,
                    failed,
                    role,
                    owner_label,
                    unread_count,
                }
            },
        )
        .collect())
}

/// Advance a person's read cursor on a conversation (never backwards). The
/// upsert is the whole existence story: unlike channel_members, a
/// conversation's owner has no standing row anywhere, so the first read
/// creates the cursor and every later one advances it.
pub async fn mark_conversation_read(
    pg: &PgPool,
    conversation_id: &str,
    user_id: &str,
    seq: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into conversation_reads (conversation_id, user_id, last_read_seq) \
         values ($1::uuid, $2::uuid, $3) \
         on conflict (conversation_id, user_id) do update \
           set last_read_seq = greatest(conversation_reads.last_read_seq, excluded.last_read_seq), \
               updated_at = now()",
    )
    .bind(conversation_id)
    .bind(user_id)
    .bind(seq)
    .execute(pg)
    .await?;
    Ok(())
}

/// The thread's highest landed seq — the "mark to latest" arm of the read
/// route, for callers who know WHICH thread but haven't loaded its seqs.
/// An empty thread reads as 0, which the counting floor (-1) treats as
/// "seen through seq 0" — nothing pending, exactly what an empty thread
/// should read as.
pub async fn latest_message_seq(pg: &PgPool, conversation_id: &str) -> Result<i32, sqlx::Error> {
    let (latest,): (i32,) = sqlx::query_as(
        "select coalesce(max(seq), 0) from messages where conversation_id = $1::uuid",
    )
    .bind(conversation_id)
    .fetch_one(pg)
    .await?;
    Ok(latest)
}

/// The detail page's narrower conversation row.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDetail {
    pub id: String,
    pub agent_model: String,
    pub title: Option<String>,
    pub updated_at: String,
    pub role: String,
}

/// One message row — the nullable jsonb trio serializes as null (the keys
/// are always present on the wire). `guard` is the one nullable column of
/// the set (messages table): unguarded rows carry null, not [].
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub role: String,
    pub content: String,
    pub reasoning: String,
    pub tools: Value,
    pub status: String,
    pub seq: i32,
    pub attachments: Value,
    pub guard: Option<Value>,
    pub metadata: Value,
    pub author_user_id: Option<String>,
    pub author_label: Option<String>,
}

/// The human a conversation belongs to, by id alone — no access check. The
/// attribution ladder resolves a chat turn to its user here; scoping by
/// access would be circular (the point is to learn WHO to scope to).
pub async fn conversation_owner(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("select user_id::text from conversations where id = $1::uuid")
            .bind(conversation_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.map(|(v,)| v))
}

/// A conversation + its messages in order. Access: yours — or a plan you're
/// a member of; RESEARCH ACCESS FOLLOWS THE
/// RUN, not a second membership list (research_runs/research_members decide),
/// so a person cannot keep the conversation after losing the report.
pub async fn get_conversation(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<(ConversationDetail, Vec<MessageRow>)>, sqlx::Error> {
    let conv: Option<(String, String, Option<String>, i64, String)> = sqlx::query_as(
        "select c.id::text, c.agent_model, c.title, \
                (trunc(extract(epoch from c.updated_at) * 1000))::bigint, \
                case when c.user_id = $2::uuid then 'owner' else 'collaborator' end \
         from conversations c \
         where c.id = $1::uuid and c.kind in ('chat', 'plan', 'research') \
           and ( \
             c.user_id = $2::uuid \
             or (c.kind = 'plan' and exists( \
               select 1 from conversation_members cm \
               where cm.conversation_id = c.id and cm.user_id = $2::uuid)) \
             or (c.kind = 'research' and exists( \
               select 1 from research_runs r \
                 left join research_members rm on rm.run_id = r.id and rm.user_id = $2::uuid \
                where r.conversation_id = c.id \
                  and (r.owner_user_id = $2::uuid or rm.user_id is not null))) \
           )",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    let Some((id, agent_model, title, updated_ms, role)) = conv else {
        return Ok(None);
    };
    // The message select's row, pre-mapping into MessageRow.
    type MessageSelRow = (
        String,
        String,
        String,
        Value,
        String,
        i32,
        Value,
        Option<Value>,
        Value,
        Option<String>,
        Option<String>,
    );
    let messages: Vec<MessageSelRow> = sqlx::query_as(
        "select m.role, m.content, m.reasoning, m.tools, m.status, m.seq, \
                    m.attachments, m.guard, m.metadata, \
                    m.author_user_id::text, coalesce(u.name, u.email) \
             from messages m left join users u on u.id = m.author_user_id \
             where m.conversation_id = $1::uuid order by m.seq asc",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    Ok(Some((
        ConversationDetail {
            id,
            agent_model,
            title,
            updated_at: epoch_ms_to_iso(updated_ms),
            role,
        },
        messages
            .into_iter()
            .map(
                |(
                    role,
                    content,
                    reasoning,
                    tools,
                    status,
                    seq,
                    attachments,
                    guard,
                    metadata,
                    author_user_id,
                    author_label,
                )| {
                    MessageRow {
                        role,
                        content,
                        reasoning,
                        tools,
                        status,
                        seq,
                        attachments,
                        guard,
                        metadata,
                        author_user_id,
                        author_label,
                    }
                },
            )
            .collect(),
    )))
}

/// A plan's humans, owner first then collaborators.
#[derive(Debug)]
pub struct PlanMember {
    pub user_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: String, // 'owner' | 'collaborator'
}

pub async fn list_plan_members(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Vec<PlanMember>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select u.id::text, u.name, u.email, 'owner' as role \
         from conversations c join users u on u.id = c.user_id where c.id = $1::uuid \
         union all \
         select u.id::text, u.name, u.email, 'collaborator' as role \
         from conversation_members cm join users u on u.id = cm.user_id \
         where cm.conversation_id = $1::uuid",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, name, email, role)| PlanMember {
            user_id,
            name,
            email,
            role,
        })
        .collect())
}

/// The user's standing on a plan: owner, collaborator, or nothing.
/// Nothing is also "not a plan" — the `kind = 'plan'` clause keeps chat
/// conversations out, and callers treat null as absent.
pub async fn plan_role(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "select case when c.user_id = $2::uuid then 'owner' \
                    when cm.user_id is not null then 'collaborator' end as role \
         from conversations c \
         left join conversation_members cm on cm.conversation_id = c.id and cm.user_id = $2::uuid \
         where c.id = $1::uuid and c.kind = 'plan'",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.and_then(|(role,)| role))
}

/// Add a collaborator. Idempotent — re-sharing with someone
/// already on the plan is a no-op, not an error.
pub async fn add_plan_member(
    pg: &PgPool,
    conversation_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into conversation_members (conversation_id, user_id) \
         values ($1::uuid, $2::uuid) on conflict do nothing",
    )
    .bind(conversation_id)
    .bind(user_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Remove a collaborator. The owner is not a member row, so
/// this only ever removes collaborators.
pub async fn remove_plan_member(
    pg: &PgPool,
    conversation_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "delete from conversation_members where conversation_id = $1::uuid and user_id = $2::uuid",
    )
    .bind(conversation_id)
    .bind(user_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// The caller's total unread messages across conversations of one kind — a
/// rail badge's number. The SAME predicate `list_conversations` counts per
/// thread (keep the two in lockstep), including the plan-membership scoping
/// and the -1 no-cursor floor.
pub async fn conversation_unread_total(
    pg: &PgPool,
    user_id: &str,
    kind: &str,
) -> Result<i32, sqlx::Error> {
    let (n,): (i32,) = sqlx::query_as(
        "select count(*)::int from conversations c \
         join messages m on m.conversation_id = c.id \
         left join conversation_reads cr on cr.conversation_id = c.id and cr.user_id = $1::uuid \
         where c.archived = false and c.kind = $2 \
           and m.seq > coalesce(cr.last_read_seq, -1) \
           and m.status = 'complete' \
           and (m.role = 'assistant' or m.author_user_id is distinct from $1::uuid) \
           and (c.user_id = $1::uuid or ($2 = 'plan' and exists( \
             select 1 from conversation_members cm \
             where cm.conversation_id = c.id and cm.user_id = $1::uuid)))",
    )
    .bind(user_id)
    .bind(kind)
    .fetch_one(pg)
    .await?;
    Ok(n)
}

/// Next sequence number for a conversation.
pub async fn next_seq(pg: &PgPool, conversation_id: &str) -> Result<i32, sqlx::Error> {
    let (next,): (i32,) = sqlx::query_as(
        "select coalesce(max(seq), -1) + 1 as next from messages where conversation_id = $1::uuid",
    )
    .bind(conversation_id)
    .fetch_one(pg)
    .await?;
    Ok(next)
}

/// Insert the user's turn; returns the message id — activity indexing keys
/// on it. `metadata` is always an object (default `{}`), so the effort stamp
/// rides as a member, never as the whole value.
pub async fn insert_user_message(
    pg: &PgPool,
    conversation_id: &str,
    seq: i32,
    content: &str,
    attachments: &Value,
    author_user_id: Option<&str>,
    metadata: &Value,
) -> Result<String, sqlx::Error> {
    let (id,): (String,) = sqlx::query_as(
        "insert into messages (conversation_id, seq, role, content, status, attachments, author_user_id, metadata) \
         values ($1::uuid, $2, 'user', $3, 'complete', $4, $5::uuid, $6) returning id::text",
    )
    .bind(conversation_id)
    .bind(seq)
    .bind(content)
    .bind(attachments)
    .bind(author_user_id)
    .bind(metadata)
    .fetch_one(pg)
    .await?;
    Ok(id)
}

/// The conversation's in-flight assistant reply, if any. A streaming row
/// older than 10 minutes is a
/// crashed persist — mark it errored and report none, so a dead stream can
/// never wedge the conversation's turn-taking.
pub async fn active_streaming_assistant(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String, bool)> = sqlx::query_as(
        "select id::text, created_at < now() - interval '10 minutes' as stale \
         from messages \
         where conversation_id = $1::uuid and role = 'assistant' and status = 'streaming' \
         order by seq desc limit 1",
    )
    .bind(conversation_id)
    .fetch_optional(pg)
    .await?;
    match row {
        None => Ok(None),
        Some((id, stale)) => {
            if stale {
                sqlx::query("update messages set status = 'error' where id = $1::uuid")
                    .bind(&id)
                    .execute(pg)
                    .await?;
                Ok(None)
            } else {
                Ok(Some(id))
            }
        }
    }
}

/// The reasoning effort the NEWEST user message asked for, or null. The
/// queued-message contract: /api/chat stamps
/// `metadata.effort` on the row, and the continuation chain reads it back
/// here — a message that never picked one answers null, which is the default
/// again, which is correct.
pub async fn last_user_message_effort(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Value,)> = sqlx::query_as(
        "select metadata from messages \
         where conversation_id = $1::uuid and role = 'user' \
         order by seq desc limit 1",
    )
    .bind(conversation_id)
    .fetch_optional(pg)
    .await?;
    let meta = row.map(|(m,)| m).unwrap_or(Value::Null);
    let effort = meta.get("effort").and_then(Value::as_str).unwrap_or("");
    Ok((!effort.trim().is_empty()).then(|| effort.to_string()))
}

/// Create the assistant row, status='streaming'; returns its id.
pub async fn insert_streaming_assistant(
    pg: &PgPool,
    conversation_id: &str,
    seq: i32,
    metadata: &Value,
) -> Result<String, sqlx::Error> {
    let (id,): (String,) = sqlx::query_as(
        "insert into messages (conversation_id, seq, role, status, metadata) \
         values ($1::uuid, $2, 'assistant', 'streaming', $3) returning id::text",
    )
    .bind(conversation_id)
    .bind(seq)
    .bind(metadata)
    .fetch_one(pg)
    .await?;
    Ok(id)
}

/// Flush accumulated assistant state — throttled during streaming, final at
/// end.
pub async fn update_assistant(
    pg: &PgPool,
    message_id: &str,
    content: &str,
    reasoning: &str,
    tools: &[ToolCall],
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages set content = $2, reasoning = $3, tools = $4, status = $5 \
         where id = $1::uuid",
    )
    .bind(message_id)
    .bind(content)
    .bind(reasoning)
    .bind(serde_json::to_value(tools).unwrap_or(Value::Array(vec![])))
    .bind(status)
    .execute(pg)
    .await?;
    Ok(())
}

/// Pin confab-guard findings to a reply. Metadata only — the UI renders a
/// caveat; transcripts never see it.
pub async fn set_message_guard(
    pg: &PgPool,
    message_id: &str,
    findings: &[crate::gateway::guard::Finding],
) -> Result<(), sqlx::Error> {
    sqlx::query("update messages set guard = $2 where id = $1::uuid")
        .bind(message_id)
        .bind(serde_json::to_value(findings).unwrap_or(Value::Null))
        .execute(pg)
        .await?;
    Ok(())
}

/// Bump the conversation's updated_at, setting a title from the first turn
/// only while the row's own is empty.
pub async fn touch_conversation(
    pg: &PgPool,
    conversation_id: &str,
    title_if_empty: Option<&str>,
) -> Result<(), sqlx::Error> {
    match title_if_empty {
        Some(t) => {
            sqlx::query(
                "update conversations set updated_at = now(), \
                     title = coalesce(nullif(title, ''), $2) \
                 where id = $1::uuid",
            )
            .bind(conversation_id)
            .bind(t)
            .execute(pg)
            .await?;
        }
        None => {
            sqlx::query("update conversations set updated_at = now() where id = $1::uuid")
                .bind(conversation_id)
                .execute(pg)
                .await?;
        }
    }
    Ok(())
}

/// The mechanical default stamped at creation — a title still equal to it
/// means nobody has named the conversation on purpose. Shared with the
/// titler's gate and the sweep.
pub fn mechanical_from(s: &str) -> String {
    // JS: s.replace(/\s+/g, ' ').trim().slice(0, 80) — collapse runs of any
    // whitespace to one space, trim, first 80 chars.
    let collapsed: Vec<&str> = s.split_whitespace().collect();
    collapsed.join(" ").chars().take(80).collect()
}

/// A message's `content` JS length for the ledger's char estimate: UTF-16
/// units for a string, the PART COUNT for an image turn's array
/// (`m.content.length` on either shape — the estimate is a fallback, and the
/// odd unit for image turns ships deliberately).
pub fn content_js_length(v: &Value) -> usize {
    match v {
        Value::String(s) => s.encode_utf16().count(),
        Value::Array(a) => a.len(),
        _ => 0,
    }
}

/// Same shape as the titler's mechanical default — a conversation's first
/// title IS the mechanical one until the Titler says otherwise.
pub fn title_from(s: &str) -> String {
    mechanical_from(s)
}

/// The row /api/chat inserts for a brand-new conversation. An explicit plan
/// template (plan surface only) is the highest link in the template
/// resolution chain — null falls through to the agent's binding — and a
/// non-plan kind never carries one: `plan_template_id` binds only for
/// kind='plan', decided here so no caller re-decides it.
pub async fn create_conversation(
    pg: &PgPool,
    user_id: &str,
    agent_model: &str,
    title: &str,
    kind: &str,
    plan_template_id: Option<&str>,
) -> Result<String, sqlx::Error> {
    let template = if kind == "plan" {
        plan_template_id
    } else {
        None
    };
    let row: Option<(String,)> = sqlx::query_as(
        "insert into conversations (user_id, agent_model, title, kind, plan_template_id) \
         values ($1::uuid, $2, $3, $4, $5::uuid) returning id::text",
    )
    .bind(user_id)
    .bind(agent_model)
    .bind(title)
    .bind(kind)
    .bind(template)
    .fetch_optional(pg)
    .await?;
    Ok(row.expect("insert … returning always yields a row").0)
}
