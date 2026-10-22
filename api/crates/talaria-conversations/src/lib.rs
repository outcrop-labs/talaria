// Conversations — the access gates (realtime.rs resolves a run with a
// `conversation` subject through the same predicate), prior-turn transcripts
// for the gateway, the listing/detail wire views, plan membership, the
// message-row plumbing a streamed reply writes through, and the title
// defaults.

use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_gateway::fleet_chat::ToolCall;
use talaria_secretbox::SecretBox;

use std::sync::OnceLock;

pub static REF_BLOCKS: OnceLock<fn(&Value) -> String> = OnceLock::new();
use talaria_uploads::attachment_text_blocks;

/// The access gate, reduced to the question its callers that only gate ask:
/// does this row exist for this person? The WHERE clause is the whole rule —
/// the OWNER, or (a PLAN, and a member), or (RESEARCH, and the run says so):
/// a chat does not admit collaborators, and research access follows the RUN
/// (research_runs/research_members decide — the same leg get_conversation
/// carries). These distinctions are decided here and nowhere else.
/// Re-deciding it at a call site would be a second answer.
///
/// Ticket threads left the conversations engine with the rooms cutover — a
/// task's room is a CHANNEL, its audience decided by board membership on the
/// channels side (channels.rs), never here.
pub async fn conversation_accessible(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from conversations c \
         where c.id = $1::uuid and c.kind in ('chat', 'plan', 'research', 'work') \
           and (c.user_id = $2::uuid \
             or (c.kind in ('plan', 'work') and ( \
               exists(select 1 from conversation_members cm \
                      where cm.conversation_id = c.id and cm.user_id = $2::uuid) \
               or exists(select 1 from conversation_teams ct \
                         join team_members tm on tm.team_id = ct.team_id \
                         where ct.conversation_id = c.id and tm.user_id = $2::uuid))) \
             or (c.kind = 'research' and exists( \
               select 1 from research_runs r \
                 left join research_members rm on rm.run_id = r.id and rm.user_id = $2::uuid \
                where r.conversation_id = c.id \
                  and (r.owner_user_id = $2::uuid or rm.user_id is not null \
                    or exists(select 1 from research_teams rt \
                              join team_members tm on tm.team_id = rt.team_id \
                              where rt.run_id = r.id and tm.user_id = $2::uuid))))) \
         limit 1",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

/// The same predicate with the one column the plan-draft POST needs after it
/// passes: the conversation's own agent, which drafts. Research rows pass the
/// gate (they must not be refused here) but carry no drafting meaning — the
/// plan-draft route requires kind='plan' on top. None covers both "not
/// accessible" and "accessible but agentless", and the route answers
/// 'plan not found' for both, never leaking which.
pub async fn accessible_conversation_agent(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "select agent_model from conversations c \
         where c.id = $1::uuid and c.kind in ('chat', 'plan', 'research', 'work') \
           and (c.user_id = $2::uuid \
             or (c.kind in ('plan', 'work') and ( \
               exists(select 1 from conversation_members cm \
                      where cm.conversation_id = c.id and cm.user_id = $2::uuid) \
               or exists(select 1 from conversation_teams ct \
                         join team_members tm on tm.team_id = ct.team_id \
                         where ct.conversation_id = c.id and tm.user_id = $2::uuid))) \
             or (c.kind = 'research' and exists( \
               select 1 from research_runs r \
                 left join research_members rm on rm.run_id = r.id and rm.user_id = $2::uuid \
                where r.conversation_id = c.id \
                  and (r.owner_user_id = $2::uuid or rm.user_id is not null \
                    or exists(select 1 from research_teams rt \
                              join team_members tm on tm.team_id = rt.team_id \
                              where rt.run_id = r.id and tm.user_id = $2::uuid))))) \
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
///
/// A STEER THE RUN ACCEPTED IS NOT HISTORY TO RE-SEND. The steer route
/// inserts its row with the next seq while the reply (a lower seq) streams,
/// so on the following turn it sits in the trailing run of user rows that
/// the pinned arm folds into the new message — and Hermes consumed those
/// words mid-run already. The row stays in the thread for readers; it is
/// left out of the transcript the agent is sent, on both arms, the same way
/// `unanswered_user_after` leaves it out of the chain.
pub async fn prior_messages(
    pg: &PgPool,
    sb: &SecretBox,
    conversation_id: &str,
    // A row to leave out — the auto-resume re-drive names the dead attempt's
    // own row here, so the agent re-hears the conversation as it stood when
    // the attempt began, not its own death notice. None everywhere else.
    exclude: Option<&str>,
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
         from messages m \
         join conversations c on c.id = m.conversation_id \
         left join users u on u.id = m.author_user_id \
         where m.conversation_id = $1::uuid and m.role in ('user','assistant') \
           and (m.content <> '' or m.attachments <> '[]'::jsonb) \
           and ($2::uuid is null or m.id <> $2::uuid) \
           and not (coalesce(m.metadata, '{}'::jsonb) @> '{\"steer_accepted\": true}'::jsonb) \
         order by m.seq asc",
        )
        .bind(conversation_id)
        .bind(exclude)
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
            REF_BLOCKS
                .get()
                .map(|f| f(&r.attachments))
                .unwrap_or_default()
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
/// collaborators; that rule lives in the WHERE clause. Work sessions share
/// the plan's membership clause — one idiom, two surfaces.
#[derive(Debug)]
pub struct AccessibleConversation {
    pub agent_model: String,
    pub kind: String, // 'chat' | 'plan' | 'research' | 'work'
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
             where id = $1::uuid and kind in ('chat', 'plan', 'research', 'work') \
               and (user_id = $2::uuid \
                 or (kind in ('plan', 'work') and ( \
                   exists(select 1 from conversation_members cm \
                          where cm.conversation_id = c.id and cm.user_id = $2::uuid) \
                   or exists(select 1 from conversation_teams ct \
                             join team_members tm on tm.team_id = ct.team_id \
                             where ct.conversation_id = c.id and tm.user_id = $2::uuid))) \
                 or (kind = 'research' and exists( \
                   select 1 from research_runs r \
                     left join research_members rm on rm.run_id = r.id and rm.user_id = $2::uuid \
                    where r.conversation_id = c.id \
                      and (r.owner_user_id = $2::uuid or rm.user_id is not null \
                        or exists(select 1 from research_teams rt \
                                  join team_members tm on tm.team_id = rt.team_id \
                                  where rt.run_id = r.id and tm.user_id = $2::uuid)))))",
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
    /// WHERE THIS VIEWER PUT THE THREAD — their own pin, group and
    /// hand-placed slot, never the owner's. A shared plan sits in several
    /// people's lists and each of them arranges it for themselves
    /// (`conversation_layout`, keyed by the pair), so these three are read
    /// from the row belonging to whoever is asking. A thread nobody arranged
    /// carries the defaults rather than nothing: a client that had to tell
    /// `false` from absent would grow two code paths for one state.
    pub pinned: bool,
    pub group: Option<String>,
    pub position: Option<i32>,
}

/// The user's conversations of a given kind, newest activity first.
pub async fn list_conversations(
    pg: &PgPool,
    user_id: &str,
    kind: &str,
    archived: bool,
) -> Result<Vec<ConversationListRow>, sqlx::Error> {
    list_conversations_in(pg, user_id, &[kind], archived).await
}

/// The user's conversations across a SET of kinds, newest activity first.
///
/// WHY A SET. One surface lists work sessions, plans and research threads
/// together, and three queries merged client-side would sort three pages
/// independently and then have to re-sort them — with the unread counts and
/// the "shared with you" rows already computed per query. One query answers
/// the question the surface is actually asking.
///
/// WHO CAN SEE WHAT, per kind. Plans and work sessions are multiplayer: your
/// own AND ones shared with you, by member or by team. Research threads follow
/// the RUN's audience — owner, member, or a team the run was shared with —
/// because a thread about a report must not be readable by anyone who cannot
/// read the report. Chats stay strictly your own.
///
/// THE MEMBERSHIP TEST READS THE ROW'S KIND, not the requested filter, and
/// that is the fix a set made necessary: `$2 in ('plan','work')` was asking
/// "did the caller ask for plans", which is the same question only while the
/// filter holds exactly one kind. Ask for plans AND chats together under the
/// old form and every chat in the org became visible to anyone with a
/// membership row. The two are indistinguishable at one kind, which is why it
/// read as correct for as long as it did.
///
/// `archived` selects the retired set (`true`) or the live one (`false`) — the
/// same flag the list has always filtered, now a parameter so a plan the owner
/// archived can be found again. Decay only ever sets it on kind = 'chat'.
///
/// THE ORDER IS THE VIEWER'S: their pinned threads first, then the slots they
/// placed by hand, then newest activity — which is the order this list has
/// always had and the one every unorganised row still falls back to.
pub async fn list_conversations_in(
    pg: &PgPool,
    user_id: &str,
    kinds: &[&str],
    archived: bool,
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
        bool,
        Option<String>,
        Option<i32>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "select c.id::text, c.agent_model, c.title, \
                (trunc(extract(epoch from c.updated_at) * 1000))::bigint, \
                exists( \
                  select 1 from messages m \
                  where m.conversation_id = c.id and m.role = 'assistant' \
                    and m.status = 'streaming' and m.streamed_at > now() - interval '5 minutes' \
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
                ), 0), \
                coalesce(l.pinned, false), \
                l.group_name, \
                l.position \
         from conversations c \
         join users o on o.id = c.user_id \
         left join conversation_reads cr on cr.conversation_id = c.id and cr.user_id = $1::uuid \
         left join conversation_layout l \
                on l.conversation_id = c.id and l.user_id = $1::uuid \
         where c.archived = $3 and c.kind = any($2) \
           and (c.user_id = $1::uuid \
             or (c.kind in ('plan', 'work') and ( \
             exists(select 1 from conversation_members cm \
                    where cm.conversation_id = c.id and cm.user_id = $1::uuid) \
             or exists(select 1 from conversation_teams ct \
                       join team_members tm on tm.team_id = ct.team_id \
                       where ct.conversation_id = c.id and tm.user_id = $1::uuid))) \
             or (c.kind = 'research' and exists( \
               select 1 from research_runs r \
                 left join research_members rm on rm.run_id = r.id and rm.user_id = $1::uuid \
                where r.conversation_id = c.id \
                  and (r.owner_user_id = $1::uuid or rm.user_id is not null \
                    or exists(select 1 from research_teams rt \
                              join team_members tm on tm.team_id = rt.team_id \
                              where rt.run_id = r.id and tm.user_id = $1::uuid))))) \
         order by coalesce(l.pinned, false) desc, l.position asc nulls last, \
                  c.updated_at desc",
    )
    .bind(user_id)
    .bind(
        kinds
            .iter()
            .map(|k| (*k).to_string())
            .collect::<Vec<String>>(),
    )
    .bind(archived)
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
                pinned,
                group,
                position,
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
                    pinned,
                    group,
                    position,
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
    /// The document pane's memory for this conversation, in pane order. Each
    /// entry is `{ kind: 'google' | 'artifact', id, title?, mime? }`. Empty on
    /// every kind that has no pane, which is most of them.
    pub pinned_files: serde_json::Value,
}

/// One message row — the nullable jsonb trio serializes as null (the keys
/// are always present on the wire). `guard` is the one nullable column of
/// the set (messages table): unguarded rows carry null, not [].
///
/// `id` and `createdAt` lead the row and `reactions` trails it — additions
/// for the Comms transcript (send times, reaction chips); a client that does
/// not read them is unaffected. `reactions` is always an array, empty when
/// nobody reacted.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub id: String,
    pub created_at: String,
    pub role: String,
    pub content: String,
    pub reasoning: String,
    pub tools: Value,
    pub status: String,
    pub seq: i32,
    pub attachments: Value,
    pub guard: Option<Value>,
    pub metadata: Value,
    pub chips: Value,
    pub author_user_id: Option<String>,
    pub author_label: Option<String>,
    pub reactions: Vec<MessageReaction>,
}

/// One emoji's reactions on an agent-DM message, actors in the order they
/// reacted — the same shape a channel message's `reactions` entries have, so
/// one chip component reads both.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageReaction {
    pub emoji: String,
    pub actors: Vec<String>,
    pub actor_types: Vec<String>,
}

/// One grouped reaction row: (message id, emoji, actors, actor types), in the
/// order the emoji was first used.
pub type ReactionGroupRow = (String, String, Vec<String>, Vec<String>);

/// Hang grouped reaction rows on the messages they belong to, keeping the
/// rows' order per message. Rows for a message not in the slice are dropped.
pub fn attach_reactions(messages: &mut [MessageRow], rows: Vec<ReactionGroupRow>) {
    // id -> index, built once; the first message with an id wins, as a scan would.
    let mut index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::with_capacity(messages.len());
    for (i, m) in messages.iter().enumerate() {
        index.entry(m.id.clone()).or_insert(i);
    }
    for (message_id, emoji, actors, actor_types) in rows {
        if let Some(&i) = index.get(message_id.as_str()) {
            messages[i].reactions.push(MessageReaction {
                emoji,
                actors,
                actor_types,
            });
        }
    }
}

/// The reaction actor for a person — the same identity channel reactions
/// record (email, else name, else "user"), so "is this chip mine" is one
/// comparison on every transcript.
pub fn reaction_actor(email: Option<&str>, name: Option<&str>) -> String {
    email
        .or(name)
        .map(str::to_string)
        .unwrap_or_else(|| "user".into())
}

/// Is this message part of this conversation? The reaction route's guard
/// against a message id borrowed from somewhere else.
pub async fn conversation_has_message(
    pg: &PgPool,
    conversation_id: &str,
    message_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> =
        sqlx::query_as("select 1 from messages where id = $1::uuid and conversation_id = $2::uuid")
            .bind(message_id)
            .bind(conversation_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.is_some())
}

/// Toggle one person's reaction on an agent-DM message: add it if absent,
/// remove it if present. Returns true when the reaction is now ON.
pub async fn toggle_message_reaction(
    pg: &PgPool,
    message_id: &str,
    emoji: &str,
    actor: &str,
    actor_type: &str,
) -> Result<bool, sqlx::Error> {
    let removed = sqlx::query(
        "delete from message_reactions \
         where message_id = $1::uuid and emoji = $2 and actor = $3",
    )
    .bind(message_id)
    .bind(emoji)
    .bind(actor)
    .execute(pg)
    .await?;
    if removed.rows_affected() > 0 {
        return Ok(false);
    }
    sqlx::query(
        "insert into message_reactions (message_id, emoji, actor, actor_type) \
         values ($1::uuid, $2, $3, $4) on conflict do nothing",
    )
    .bind(message_id)
    .bind(emoji)
    .bind(actor)
    .bind(actor_type)
    .execute(pg)
    .await?;
    Ok(true)
}

/// Grouped reactions for a set of messages, one row per (message, emoji),
/// emoji in first-use order and actors in reaction order.
pub async fn message_reaction_groups(
    pg: &PgPool,
    message_ids: &[&str],
) -> Result<Vec<ReactionGroupRow>, sqlx::Error> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "select message_id::text, emoji, \
             array_agg(actor order by created_at), \
             array_agg(actor_type order by created_at) \
         from message_reactions where message_id = any($1::uuid[]) \
         group by message_id, emoji \
         order by min(created_at)",
    )
    .bind(message_ids)
    .fetch_all(pg)
    .await
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
/// so a person cannot keep the conversation after losing the report. Ticket
/// threads are channels now — their audience is board membership, decided on
/// the channels side.
pub async fn get_conversation(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<(ConversationDetail, Vec<MessageRow>)>, sqlx::Error> {
    let conv: Option<(
        String,
        String,
        Option<String>,
        i64,
        String,
        serde_json::Value,
    )> = sqlx::query_as(
        "select c.id::text, c.agent_model, c.title, \
                (trunc(extract(epoch from c.updated_at) * 1000))::bigint, \
                case when c.user_id = $2::uuid then 'owner' else 'collaborator' end, \
                c.pinned_files \
         from conversations c \
         where c.id = $1::uuid and c.kind in ('chat', 'plan', 'research', 'work') \
           and ( \
             c.user_id = $2::uuid \
             or (c.kind in ('plan', 'work') and ( \
               exists(select 1 from conversation_members cm \
                      where cm.conversation_id = c.id and cm.user_id = $2::uuid) \
               or exists(select 1 from conversation_teams ct \
                         join team_members tm on tm.team_id = ct.team_id \
                         where ct.conversation_id = c.id and tm.user_id = $2::uuid))) \
             or (c.kind = 'research' and exists( \
               select 1 from research_runs r \
                 left join research_members rm on rm.run_id = r.id and rm.user_id = $2::uuid \
                where r.conversation_id = c.id \
                  and (r.owner_user_id = $2::uuid or rm.user_id is not null \
                    or exists(select 1 from research_teams rt \
                              join team_members tm on tm.team_id = rt.team_id \
                              where rt.run_id = r.id and tm.user_id = $2::uuid)))) \
           )",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    let Some((id, agent_model, title, updated_ms, role, pinned_files)) = conv else {
        return Ok(None);
    };
    // The message select's row, pre-mapping into MessageRow.
    type MessageSelRow = (
        String,
        i64,
        String,
        String,
        String,
        Value,
        String,
        i32,
        Value,
        Option<Value>,
        Value,
        Value,
        Option<String>,
        Option<String>,
    );
    let messages: Vec<MessageSelRow> = sqlx::query_as(
        "select m.id::text, (trunc(extract(epoch from m.created_at) * 1000))::bigint, \
                    m.role, m.content, m.reasoning, m.tools, m.status, m.seq, \
                    m.attachments, m.guard, m.metadata, m.chips, \
                    m.author_user_id::text, coalesce(u.name, u.email) \
             from messages m left join users u on u.id = m.author_user_id \
             where m.conversation_id = $1::uuid order by m.seq asc",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    let mut messages: Vec<MessageRow> = messages
        .into_iter()
        .map(
            |(
                message_id,
                created_ms,
                role,
                content,
                reasoning,
                tools,
                status,
                seq,
                attachments,
                guard,
                metadata,
                chips,
                author_user_id,
                author_label,
            )| {
                MessageRow {
                    id: message_id,
                    created_at: epoch_ms_to_iso(created_ms),
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
                    chips,
                    reactions: Vec::new(),
                }
            },
        )
        .collect();
    let ids: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
    let groups = message_reaction_groups(pg, &ids).await?;
    attach_reactions(&mut messages, groups);
    Ok(Some((
        ConversationDetail {
            id,
            agent_model,
            title,
            updated_at: epoch_ms_to_iso(updated_ms),
            role,
            pinned_files,
        },
        messages,
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

#[derive(Debug, serde::Serialize)]
pub struct PlanTeam {
    pub id: String,
    pub name: String,
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

/// What the Work pane's file list is derived from: every message that carries
/// chips, in conversation order, as `(created_at ISO, chips)`, and the
/// conversation's `pinned_files`. Rows with no chips are skipped in the
/// query — most of a long thread carries none. The caller has already been
/// access-checked; this only reads.
pub async fn conversation_file_trail(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<(Vec<(String, Value)>, Value), sqlx::Error> {
    let rows: Vec<(i64, Value)> = sqlx::query_as(
        "select (trunc(extract(epoch from created_at) * 1000))::bigint, chips \
         from messages \
         where conversation_id = $1::uuid \
           and chips <> '[]'::jsonb \
         order by seq asc",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    let pinned: Option<(Value,)> =
        sqlx::query_as("select pinned_files from conversations where id = $1::uuid")
            .bind(conversation_id)
            .fetch_optional(pg)
            .await?;
    Ok((
        rows.into_iter()
            .map(|(ms, chips)| (epoch_ms_to_iso(ms), chips))
            .collect(),
        pinned
            .map(|(p,)| p)
            .unwrap_or_else(|| Value::Array(Vec::new())),
    ))
}

/// Replace the conversation's pinned files — the document pane's memory.
/// Whole-array, not a patch: the pane's order is part of what is stored, and a
/// per-entry API would have to describe moves as well as adds. The caller has
/// already been access-checked; this only writes.
pub async fn set_pinned_files(
    pg: &PgPool,
    conversation_id: &str,
    files: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("update conversations set pinned_files = $2 where id = $1::uuid")
        .bind(conversation_id)
        .bind(files)
        .execute(pg)
        .await?;
    Ok(())
}

/// The caller's standing on a SHARED conversation — a plan or a work session:
/// owner, collaborator, or nothing. Nothing is also "not a shareable kind":
/// the kind clause keeps plain chats out, and callers treat null as absent.
///
/// This was `plan_role` and grew a second kind rather than a second function,
/// because its only callers are the membership and team routes — the question
/// they ask is "may you share this", which is the same question for both
/// surfaces. The plan's living document and its ticket drafts gate themselves
/// elsewhere on `kind = 'plan'`, so widening here does not reach them.
pub async fn shared_conversation_role(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "select case when c.user_id = $2::uuid then 'owner' \
                    when cm.user_id is not null then 'collaborator' \
                    when exists(select 1 from conversation_teams ct \
                                join team_members tm on tm.team_id = ct.team_id \
                                where ct.conversation_id = c.id and tm.user_id = $2::uuid) \
                         then 'collaborator' end as role \
         from conversations c \
         left join conversation_members cm on cm.conversation_id = c.id and cm.user_id = $2::uuid \
         where c.id = $1::uuid and c.kind in ('plan', 'work')",
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

pub async fn list_plan_teams(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Vec<PlanTeam>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select t.id::text, t.name from conversation_teams ct \
         join teams t on t.id = ct.team_id \
         where ct.conversation_id = $1::uuid order by t.name",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| PlanTeam { id, name })
        .collect())
}

pub async fn add_plan_team(
    pg: &PgPool,
    conversation_id: &str,
    team_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into conversation_teams (conversation_id, team_id) \
         values ($1::uuid, $2::uuid) on conflict do nothing",
    )
    .bind(conversation_id)
    .bind(team_id)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn remove_plan_team(
    pg: &PgPool,
    conversation_id: &str,
    team_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "delete from conversation_teams where conversation_id = $1::uuid and team_id = $2::uuid",
    )
    .bind(conversation_id)
    .bind(team_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// The caller's total unread messages across conversations of one kind — a
/// rail badge's number. The SAME audience predicate `list_conversations_in`
/// filters by, including the plan/work membership scoping, the research run's
/// own audience, and the -1 no-cursor floor.
///
/// KEEP THE TWO IN LOCKSTEP, and the research arm is why that instruction had
/// teeth: without it a badge counted only the research threads you OWN while
/// the list showed every one shared with you, so the number under the icon
/// disagreed with the rows behind it. A comment asking a human to remember is
/// the weakest form of this guarantee; it is here because the two queries
/// answer different shapes (a count, and the rows) and have no third place to
/// share.
///
/// `kind` is a parameter rather than a list for the same reason: Work's badge
/// is this read with `"work"` and nothing else, because the shared-kind
/// scoping below already spans plans and work sessions — a session someone
/// else started and shared with you counts here without a predicate of its
/// own.
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
           and (c.user_id = $1::uuid \
             or (c.kind in ('plan', 'work') and ( \
             exists(select 1 from conversation_members cm \
                    where cm.conversation_id = c.id and cm.user_id = $1::uuid) \
             or exists(select 1 from conversation_teams ct \
                       join team_members tm on tm.team_id = ct.team_id \
                       where ct.conversation_id = c.id and tm.user_id = $1::uuid))) \
             or (c.kind = 'research' and exists( \
               select 1 from research_runs r \
                 left join research_members rm on rm.run_id = r.id and rm.user_id = $1::uuid \
                where r.conversation_id = c.id \
                  and (r.owner_user_id = $1::uuid or rm.user_id is not null \
                    or exists(select 1 from research_teams rt \
                              join team_members tm on tm.team_id = rt.team_id \
                              where rt.run_id = r.id and tm.user_id = $1::uuid)))))",
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

/// The conversation's in-flight assistant reply, if any. A streaming row that
/// has not MOVED in 15 minutes is a crashed persist — mark it errored and
/// report none, so a dead stream can never wedge the conversation's
/// turn-taking. The signal is `streamed_at` (stamped on every flush), not the
/// row's age: a long agent turn — the kind that builds out a knowledgebase —
/// streams tool progress for an hour, and age alone would kill it at ten
/// minutes while it was still working. Fifteen minutes of SILENCE is the
/// honest bar: the drain loop owns a 10-minute idle ceiling (chat_persist's
/// STREAM_IDLE), so a row this quiet means its writer is gone.
pub async fn active_streaming_assistant(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String, bool)> = sqlx::query_as(
        "select id::text, streamed_at < now() - interval '15 minutes' as stale \
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
                // The reason lands only on an EMPTY row: a writer that died
                // mid-prose keeps its prose (the mark renders beside it), and
                // this sweep is the last-resort writer — the normal death
                // paths put a better sentence in the row themselves.
                sqlx::query(
                    "update messages set status = 'error', \
                        content = case when content = '' \
                          then '(agent error: the stream writer vanished mid-turn)' \
                          else content end \
                     where id = $1::uuid",
                )
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

/// Whether an insert lost the race for a seq: `UNIQUE (conversation_id, seq)`
/// refused it because another writer — a second tab, a chain racing a live
/// turn — took the number between `next_seq` and the insert. The one error
/// a caller may answer with "try again" rather than "something broke".
pub fn is_seq_collision(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}

/// Create the assistant row, status='streaming'; returns its id.
///
/// RETRIES THE SEQ ONCE. `next_seq` and this insert are two statements with
/// no lock between them, so two tabs sending at the same instant can both
/// compute the same number; the user row that lost is the caller's to refuse
/// (`is_seq_collision`), but an assistant row that loses AFTER its user row
/// landed would leave a turn with a question and no reply row — the shape
/// the chain only notices a turn later. The reply takes the next free
/// number instead: it still follows its user row, because the collision
/// means the other writer's row sits between them and nothing else does.
pub async fn insert_streaming_assistant(
    pg: &PgPool,
    conversation_id: &str,
    seq: i32,
    metadata: &Value,
) -> Result<String, sqlx::Error> {
    async fn insert(
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
    match insert(pg, conversation_id, seq, metadata).await {
        Err(e) if is_seq_collision(&e) => {
            let fresh = next_seq(pg, conversation_id).await?;
            insert(pg, conversation_id, fresh, metadata).await
        }
        other => other,
    }
}

/// Stamp the Hermes run behind a streaming assistant row, the moment the
/// stream names it (`run.started`). It is the handle a stop or a steer needs:
/// Hermes addresses a running turn by its own run id, and nothing else in the
/// row says which one that is.
pub async fn set_message_run_id(
    pg: &PgPool,
    message_id: &str,
    run_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("update messages set hermes_run_id = $2 where id = $1::uuid")
        .bind(message_id)
        .bind(run_id)
        .execute(pg)
        .await?;
    Ok(())
}

/// The conversation's in-flight assistant row and the Hermes run behind it —
/// what a stop or a steer has to name. `None` is "nothing is streaming", which
/// is the whole of the refusal both routes answer with.
///
/// A PLAIN READ, deliberately unlike `active_streaming_assistant`: that one
/// sweeps a row that has gone quiet for fifteen minutes into an error, because
/// its question is "may a NEW turn start". This one's question is "what is the
/// reader looking at right now", and the reader is looking at a spinner over
/// exactly this row — killing it out from under a stop would answer "nothing is
/// streaming" to a person who can see that something is. The run id is absent
/// for a turn that went through the full-history chat arm, which Hermes
/// addresses by nothing either route could name.
pub async fn streaming_run(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "select id::text, hermes_run_id from messages \
         where conversation_id = $1::uuid and role = 'assistant' and status = 'streaming' \
         order by seq desc limit 1",
    )
    .bind(conversation_id)
    .fetch_optional(pg)
    .await?;
    Ok(row)
}

/// Does a user row that is still OWED AN ANSWER sit after this one? The
/// tail chain's question, asked before it fires: a message queued while this
/// reply streamed is owed the next turn, but a steer the running agent
/// ACCEPTED was answered inside this very turn — firing the chain on it would
/// answer it twice. And a stopped turn with nobody having spoken since must
/// leave the chain quiet: an empty stopped row is invisible to
/// `prior_messages`, so the chain would re-ask the message the stopped turn
/// was answering, forever. A row that is not there answers false (the
/// subselect is null, so the comparison is), which is the direction that
/// stays quiet.
pub async fn unanswered_user_after(
    pg: &PgPool,
    conversation_id: &str,
    message_id: &str,
) -> Result<bool, sqlx::Error> {
    let (after,): (bool,) = sqlx::query_as(
        "select exists ( \
            select 1 from messages \
            where conversation_id = $1::uuid and role = 'user' \
              and seq > (select seq from messages where id = $2::uuid) \
              and not (coalesce(metadata, '{}'::jsonb) @> '{\"steer_accepted\": true}'::jsonb))",
    )
    .bind(conversation_id)
    .bind(message_id)
    .fetch_one(pg)
    .await?;
    Ok(after)
}

/// The steer route's stamp once Hermes took the words: the row was answered
/// by the turn it was said into, so `unanswered_user_after` leaves it alone.
pub async fn mark_steer_accepted(pg: &PgPool, message_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages \
         set metadata = coalesce(metadata, '{}'::jsonb) || '{\"steer_accepted\": true}'::jsonb \
         where id = $1::uuid",
    )
    .bind(message_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Whether a conversation can be driven through a pinned Hermes session.
///
/// Hermes holds the history for a pinned conversation, so a conversation
/// whose earlier turns went through the full-history path has a transcript
/// Hermes never saw; sending it only the new message would answer from an
/// empty room. The row itself says which it is: an assistant row with a run
/// id was pinned-driven, a conversation with no assistant rows at all is
/// fresh, and assistant rows without one are the legacy transcript — which
/// stays on the full-history path, flip or no flip.
pub async fn pinned_history_fits(
    pg: &PgPool,
    conversation_id: &str,
    // The row to leave out — the auto-resume names the dead attempt's own
    // row, which is in the table already and says nothing about how the
    // conversation was driven before it. None everywhere else.
    exclude: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let (assistants, pinned): (i64, i64) = sqlx::query_as(
        "select count(*), count(hermes_run_id) from messages \
         where conversation_id = $1::uuid and role = 'assistant' \
           and ($2::text is null or id::text <> $2)",
    )
    .bind(conversation_id)
    .bind(exclude)
    .fetch_one(pg)
    .await?;
    Ok(pinned_history_fits_counts(assistants, pinned))
}

/// The rule behind `pinned_history_fits`, on the two counts.
pub fn pinned_history_fits_counts(assistant_rows: i64, pinned_rows: i64) -> bool {
    assistant_rows == 0 || pinned_rows > 0
}

/// Flush accumulated assistant state — throttled during streaming, final at
/// end. Every flush also stamps `streamed_at`, which is the row's liveness
/// signal everywhere liveness is asked about (the sweep above, the sidebar's
/// working flag): a streaming row that is still writing is still alive,
/// however long the turn runs.
pub async fn update_assistant(
    pg: &PgPool,
    message_id: &str,
    content: &str,
    reasoning: &str,
    tools: &[ToolCall],
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages set content = $2, reasoning = $3, tools = $4, status = $5, \
            streamed_at = now() \
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

/// Put a dead turn back to work: same row, blank slate, streaming again. The
/// auto-resume path's move — a resurrected turn keeps its place in the
/// transcript (no duplicate bubble) and the working indicators come back on
/// (the stamp below is what they read). Content and tools clear because the
/// re-driven turn re-derives them; a half-arrival left behind would read as
/// the agent's own words.
pub async fn resurrect_streaming_assistant(
    pg: &PgPool,
    message_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages set content = '', reasoning = '', tools = '[]'::jsonb, \
            status = 'streaming', streamed_at = now() \
         where id = $1::uuid",
    )
    .bind(message_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Stamp a turn as having been auto-resumed. Merged like every metadata
/// stamp; the UI reads it to mark the turn, and the resume path reads it to
/// retry exactly once — a turn that dies twice is a human's to look at.
pub async fn mark_message_resumed(pg: &PgPool, message_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages \
         set metadata = coalesce(metadata, '{}'::jsonb) || '{\"resumed\": true}'::jsonb \
         where id = $1::uuid",
    )
    .bind(message_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Stamp a turn as STOPPED — a person pressed stop. (An agent stuck calling
/// the same thing over and over is halted inside Hermes by its own loop
/// guard, which ends the turn with a reply rather than a stop.) The row
/// itself lands COMPLETE, not errored: whatever the agent managed to say is
/// what the person chose to keep, and an error status would put a retry
/// affordance on a turn nobody wants retried. This stamp is what tells the
/// difference, and the UI reads it to mark the reply as cut short. Merged like
/// every metadata stamp.
pub async fn mark_message_stopped(pg: &PgPool, message_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages \
         set metadata = coalesce(metadata, '{}'::jsonb) || '{\"stopped\": true}'::jsonb \
         where id = $1::uuid",
    )
    .bind(message_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// The resume gate's re-read: is the row still the errored turn the backoff
/// left behind — neither nudged complete by some other writer, nor already
/// resumed once? Key-existence, not a cast of the value: the stamp is written
/// by mark_message_resumed and read by no SQL that cares about its value.
pub async fn message_still_errored(pg: &PgPool, message_id: &str) -> Result<bool, sqlx::Error> {
    let (live,): (bool,) = sqlx::query_as(
        "select exists ( \
            select 1 from messages \
            where id = $1::uuid and status = 'error' \
              and not coalesce(metadata, '{}'::jsonb) ? 'resumed')",
    )
    .bind(message_id)
    .fetch_one(pg)
    .await?;
    Ok(live)
}

/// Pin confab-guard findings to a reply. Metadata only — the UI renders a
/// caveat; transcripts never see it.
pub async fn set_message_guard(
    pg: &PgPool,
    message_id: &str,
    findings: &[talaria_gateway::guard::Finding],
) -> Result<(), sqlx::Error> {
    sqlx::query("update messages set guard = $2 where id = $1::uuid")
        .bind(message_id)
        .bind(serde_json::to_value(findings).unwrap_or(Value::Null))
        .execute(pg)
        .await?;
    Ok(())
}

/// Append ONE upload to a message's `attachments`, keeping whatever is
/// already stamped there.
///
/// The stamp is what makes an upload readable at all: `can_access_upload`
/// answers "may this reader see these bytes" by probing the attachments JSON
/// of the rows the reader can reach, so an upload nobody's message points at
/// is stored and invisible — the owner sees it, the conversation does not.
/// The agent uploads door writes through here once the bytes are saved, with
/// the same `{id, filename, mime, size}` object /api/chat stamps for a
/// person's attachment; one shape, because the probe and the renderer both
/// read it.
///
/// The concatenation is type-guarded because `jsonb || jsonb` on two objects
/// is a MERGE: a row whose column somehow held an object rather than an array
/// would have the attachment folded into it instead of added to it, quietly
/// rewriting the stamp rather than extending it.
pub async fn append_message_attachment(
    pg: &PgPool,
    message_id: &str,
    attachment: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages set attachments = \
           case when jsonb_typeof(attachments) = 'array' then attachments else '[]'::jsonb end \
           || jsonb_build_array($2::jsonb) \
         where id = $1::uuid",
    )
    .bind(message_id)
    .bind(attachment)
    .execute(pg)
    .await?;
    Ok(())
}

/// The ticket gate's hold stamp, written when the relevance judge decided a
/// message is not the assigned agent's business. The message STAYS — the
/// humans in the room read it — but the stamp is the durable record of the
/// verdict, and the continuation chain reads it back (below) so no queued
/// copy of the message is ever answered. Merged, never replaced: the effort
/// stamp rides the same object.
pub async fn hold_ticket_message(pg: &PgPool, message_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages \
         set metadata = coalesce(metadata, '{}'::jsonb) || '{\"ticket_hold\": true}'::jsonb \
         where id = $1::uuid",
    )
    .bind(message_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Whether the newest user message carries the hold stamp — the chain's
/// question, asked before it builds a turn that would answer it. A read that
/// cannot happen answers false, the same direction the gate itself fails:
/// toward answering.
pub async fn last_user_message_held(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Option<bool>,)> = sqlx::query_as(
        "select (metadata->>'ticket_hold')::bool from messages \
         where conversation_id = $1::uuid and role = 'user' \
         order by seq desc limit 1",
    )
    .bind(conversation_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.and_then(|(held,)| held).unwrap_or(false))
}

/// One complete assistant turn, written server-side — the outreach-DM shape,
/// available to any plane that owes a person a line without streaming one
/// (the research run's scope question, its scope ack, its report-ready
/// notice). `marker` is the idempotency key: a run step that posts and then
/// crashes before its park or checkpoint lands is re-entered by the reclaim
/// sweep, and the re-entry must not say the same thing twice. Returns the
/// message id — the existing row's when the marker already posted, the new
/// row's when it did not.
pub async fn post_agent_turn(
    pg: &PgPool,
    conversation_id: &str,
    marker: &str,
    body: &str,
) -> Result<String, sqlx::Error> {
    // Already said: hand back the same id so the caller's idea of "the turn I
    // posted" stays stable across re-entries.
    let said: Option<(String,)> = sqlx::query_as(
        "select id::text from messages \
         where conversation_id = $1::uuid and metadata->>'marker' = $2 \
         limit 1",
    )
    .bind(conversation_id)
    .bind(marker)
    .fetch_optional(pg)
    .await?;
    if let Some((id,)) = said {
        return Ok(id);
    }
    let seq = next_seq(pg, conversation_id).await?;
    let id = insert_streaming_assistant(
        pg,
        conversation_id,
        seq,
        &serde_json::json!({ "marker": marker }),
    )
    .await?;
    update_assistant(pg, &id, body, "", &[], "complete").await?;
    touch_conversation(pg, conversation_id, None).await?;
    Ok(id)
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

/// Which session kinds carry a plan template on their row.
///
/// THE LIVING DOCUMENT DECIDES IT, not the name of the column. A plan and a
/// work session are both served the document beside the chat, and both offer
/// the template picker on a new session's zero state, so both must keep what
/// was chosen there. The kinds with no document — chat, ticket, research —
/// would be storing an id nothing ever reads.
pub fn kind_carries_plan_template(kind: &str) -> bool {
    kind == "plan" || kind == "work"
}

/// The row /api/chat inserts for a brand-new conversation. An explicit
/// template is the highest link in the template resolution chain — null falls
/// through to the agent's binding — and only a document-bearing kind carries
/// one: the bind goes through `kind_carries_plan_template`, decided here so no
/// caller re-decides it.
///
/// THE 'work' KIND IS IN THAT SET DELIBERATELY. The first turn is what creates
/// the conversation, so this insert is the only place a template chosen on a
/// new work session can land — binding for kind='plan' alone threw the choice
/// away, and the person who made it saw nothing to say it had been ignored.
pub async fn create_conversation(
    pg: &PgPool,
    user_id: &str,
    agent_model: &str,
    title: &str,
    kind: &str,
    plan_template_id: Option<&str>,
) -> Result<String, sqlx::Error> {
    let template = if kind_carries_plan_template(kind) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, seq: i32) -> MessageRow {
        MessageRow {
            id: id.into(),
            created_at: "2026-10-05T09:30:00.000Z".into(),
            role: "assistant".into(),
            content: "hi".into(),
            reasoning: String::new(),
            tools: serde_json::json!([]),
            status: "complete".into(),
            seq,
            attachments: serde_json::json!([]),
            guard: None,
            metadata: serde_json::json!({}),
            chips: serde_json::json!([]),
            author_user_id: None,
            author_label: None,
            reactions: Vec::new(),
        }
    }

    #[test]
    fn only_a_document_bearing_kind_carries_a_plan_template() {
        // Both surfaces that serve the living document keep the picker's
        // choice — a work session's template was being dropped at the insert.
        assert!(kind_carries_plan_template("plan"));
        assert!(kind_carries_plan_template("work"));
        for kind in ["chat", "ticket", "research", ""] {
            assert!(
                !kind_carries_plan_template(kind),
                "{kind} has no document to seed"
            );
        }
    }

    #[test]
    fn message_row_wire_carries_id_time_and_reactions() {
        let mut m = row("m1", 2);
        m.reactions.push(MessageReaction {
            emoji: "🙌".into(),
            actors: vec!["a@x".into()],
            actor_types: vec!["user".into()],
        });
        let v: Value = serde_json::to_value(&m).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.first(), Some(&"id"));
        assert_eq!(keys.get(1), Some(&"createdAt"));
        assert_eq!(keys.last(), Some(&"reactions"));
        assert_eq!(v["createdAt"], "2026-10-05T09:30:00.000Z");
        assert_eq!(
            v["reactions"],
            serde_json::json!([{ "emoji": "🙌", "actors": ["a@x"], "actorTypes": ["user"] }])
        );
        // Nobody reacted: an empty array, never absent.
        assert_eq!(
            serde_json::to_value(row("m2", 3)).unwrap()["reactions"],
            serde_json::json!([])
        );
    }

    #[test]
    fn reactions_land_on_their_own_message_in_row_order() {
        let mut ms = vec![row("m1", 0), row("m2", 1)];
        attach_reactions(
            &mut ms,
            vec![
                (
                    "m2".into(),
                    "✅".into(),
                    vec!["a@x".into()],
                    vec!["user".into()],
                ),
                (
                    "m1".into(),
                    "👀".into(),
                    vec!["b@x".into()],
                    vec!["user".into()],
                ),
                (
                    "m2".into(),
                    "🙌".into(),
                    vec!["a@x".into(), "b@x".into()],
                    vec!["user".into(), "user".into()],
                ),
                // A row for a message not on the page is dropped.
                (
                    "m9".into(),
                    "✅".into(),
                    vec!["c@x".into()],
                    vec!["user".into()],
                ),
            ],
        );
        let emojis = |m: &MessageRow| {
            m.reactions
                .iter()
                .map(|r| r.emoji.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(emojis(&ms[0]), vec!["👀"]);
        assert_eq!(emojis(&ms[1]), vec!["✅", "🙌"]);
        assert_eq!(ms[1].reactions[1].actors, vec!["a@x", "b@x"]);
    }

    #[test]
    fn a_reaction_actor_is_the_channel_identity() {
        assert_eq!(reaction_actor(Some("a@x"), Some("A")), "a@x");
        assert_eq!(reaction_actor(None, Some("A")), "A");
        assert_eq!(reaction_actor(None, None), "user");
    }
}
