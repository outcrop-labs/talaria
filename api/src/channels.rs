// Channels — the comms family's row plane: the member/user listings with
// their wire views, CRUD (create/DM/rename/archive/delete), membership and
// read cursors, the agent access paths (elevation, owner-proxy), the message
// page with its reaction/thread decoration, and the toggle/edit/delete/react
// row gates.

use crate::agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model};
use crate::agent_writes::{WriteAuthor, guard_agent_write};
use crate::notify::{NotifyDeps, briefs_follow_message};
use crate::realtime::{ChannelEvent, publish_channel};
use crate::users::{assistant_owner_for, is_elevated_assistant};
use serde_json::Value;
use sqlx::PgPool;

/// Exactly the hyphenated uuid shape, hex in either
/// case. Hand-rolled rather than `Uuid::parse_str` because the crate's parser
/// is WIDER than the regex — it also takes the braced, urn, and hyphen-less
/// spellings, and those are not channel ids anywhere in this product.
pub fn is_channel_id(id: &str) -> bool {
    let b = id.as_bytes();
    // 8-4-4-4-12 hex groups joined by single hyphens: 36 bytes.
    b.len() == 36
        && b[..8].iter().all(u8::is_ascii_hexdigit)
        && b[8] == b'-'
        && b[9..13].iter().all(u8::is_ascii_hexdigit)
        && b[13] == b'-'
        && b[14..18].iter().all(u8::is_ascii_hexdigit)
        && b[18] == b'-'
        && b[19..23].iter().all(u8::is_ascii_hexdigit)
        && b[23] == b'-'
        && b[24..36].iter().all(u8::is_ascii_hexdigit)
}

/// The caller's row in `channel_members`, or null.
/// A non-uuid id is not a membership question, and handing it to Postgres is a
/// 500 (`invalid input syntax for type uuid`) — answering null makes callers
/// say forbidden instead, the honest answer for an id that cannot name a
/// channel. See `is_channel_id`.
pub async fn channel_role(
    pg: &PgPool,
    user_id: &str,
    channel_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    if !is_channel_id(channel_id) {
        return Ok(None);
    }
    let row: Option<(String,)> = sqlx::query_as(
        "select role from channel_members where channel_id = $1::uuid and user_id = $2::uuid",
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(role,)| role))
}

// ── The channel wire views ────────────────────────────────────────────────────
//
// Three shapes leave the API, each an exact key set:
// the member listing (role + unread + peer), the agent
// listing (bare — no role, the agent has none), and a fresh create/DM
// (role 'owner' appended last). `peer` on the member view is an
// ALWAYS-PRESENT nullable — null for non-DMs.

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelPeer {
    pub user_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// The member-listing row — `unreadCount` then `peer` last.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberChannel {
    pub id: String,
    pub name: String,
    pub topic: Option<String>,
    pub kind: String,
    pub role: String,
    pub created_at: String,
    pub updated_at: String,
    pub unread_count: i32,
    pub peer: Option<ChannelPeer>,
}

/// The agent listing and the elevated view — the bare channel columns.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChannel {
    pub id: String,
    pub name: String,
    pub topic: Option<String>,
    pub kind: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The create/DM return — the row plus `role` appended last.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedChannel {
    pub id: String,
    pub name: String,
    pub topic: Option<String>,
    pub kind: String,
    pub created_at: String,
    pub updated_at: String,
    pub role: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMember {
    pub user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
}

/// Channels/relays/DMs the user belongs to, newest activity first. DMs carry
/// the other person's identity so the UI can label them. unreadCount is the
/// member's read cursor vs. others' complete messages — the not-self clause
/// compares against the caller's OWN email/name spelling, so a self-posted
/// message never counts unread for its author.
pub async fn list_channels(pg: &PgPool, user_id: &str) -> Result<Vec<MemberChannel>, sqlx::Error> {
    #[allow(clippy::type_complexity)] // the listing's own columns, one each
    type MemberRow = (
        String,
        String,
        Option<String>,
        String,
        String,
        i64,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        i32,
    );
    let rows: Vec<MemberRow> = sqlx::query_as(
        "select c.id::text, c.name, c.topic, c.kind, m.role, \
             (trunc(extract(epoch from c.created_at) * 1000))::bigint, \
             (trunc(extract(epoch from c.updated_at) * 1000))::bigint, \
             pu.id::text, pu.name, pu.email, \
             (select count(*)::int from channel_messages msg \
               where msg.channel_id = c.id and msg.seq > m.last_read_seq \
                 and msg.status = 'complete' \
                 and not (msg.author_type = 'user' \
                   and msg.author = coalesce(self.email, self.name, 'user'))) \
         from channels c \
         join channel_members m on m.channel_id = c.id and m.user_id = $1::uuid \
         join users self on self.id = $1::uuid \
         left join channel_members p on c.kind = 'dm' and p.channel_id = c.id and p.user_id <> $1::uuid \
         left join users pu on pu.id = p.user_id \
         where c.archived_at is null \
         order by c.updated_at desc",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                name,
                topic,
                kind,
                role,
                created_ms,
                updated_ms,
                peer_id,
                peer_name,
                peer_email,
                unread,
            )| {
                MemberChannel {
                    id,
                    name,
                    topic,
                    kind,
                    role,
                    created_at: epoch_ms_to_iso(created_ms),
                    updated_at: epoch_ms_to_iso(updated_ms),
                    unread_count: unread,
                    peer: peer_id.map(|user_id| ChannelPeer {
                        user_id,
                        name: peer_name,
                        email: peer_email,
                    }),
                }
            },
        )
        .collect())
}

/// Advance the member's read cursor (never backwards).
pub async fn mark_channel_read(
    pg: &PgPool,
    channel_id: &str,
    user_id: &str,
    seq: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update channel_members set last_read_seq = greatest(last_read_seq, $3) \
         where channel_id = $1::uuid and user_id = $2::uuid",
    )
    .bind(channel_id)
    .bind(user_id)
    .bind(seq)
    .execute(pg)
    .await?;
    Ok(())
}

/// Create a channel or a Relay; the creator becomes its owner (and first
/// member).
pub async fn create_channel(
    pg: &PgPool,
    user_id: &str,
    name: &str,
    topic: Option<&str>,
    kind: &str,
) -> Result<CreatedChannel, sqlx::Error> {
    let mut tx = pg.begin().await?;
    let (id, name, topic, kind, created_ms, updated_ms): (
        String,
        String,
        Option<String>,
        String,
        i64,
        i64,
    ) = sqlx::query_as(
        "insert into channels (name, topic, kind, created_by) values ($1, $2, $3, $4::uuid) \
         returning id::text, name, topic, kind, \
           (trunc(extract(epoch from created_at) * 1000))::bigint, \
           (trunc(extract(epoch from updated_at) * 1000))::bigint",
    )
    .bind(name)
    .bind(topic)
    .bind(kind)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "insert into channel_members (channel_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(CreatedChannel {
        id,
        name,
        topic,
        kind,
        created_at: epoch_ms_to_iso(created_ms),
        updated_at: epoch_ms_to_iso(updated_ms),
        role: "owner".into(),
    })
}

/// Find-or-create the DM between two people (deduped on the sorted id pair).
/// Both sides are owners — a DM has no hierarchy. `Err` carries the
/// cannot-DM-yourself sentence the route answers as a 400.
pub async fn ensure_dm(
    pg: &PgPool,
    user_id: &str,
    other_user_id: &str,
) -> Result<CreatedChannel, String> {
    if user_id == other_user_id {
        return Err("cannot DM yourself".into());
    }
    let mut pair = [user_id, other_user_id];
    pair.sort(); // canonical order for the dm key; uuids are ASCII lowercase
    let dm_key = format!("{}:{}", pair[0], pair[1]);
    let existing: Option<(String, String, Option<String>, String, i64, i64)> = sqlx::query_as(
        "select id::text, name, topic, kind, \
             (trunc(extract(epoch from created_at) * 1000))::bigint, \
             (trunc(extract(epoch from updated_at) * 1000))::bigint \
         from channels where dm_key = $1",
    )
    .bind(&dm_key)
    .fetch_optional(pg)
    .await
    .map_err(|e| crate::error::pg_message(&e))?;
    if let Some((id, name, topic, kind, created_ms, updated_ms)) = existing {
        // Un-archive on revisit — a DM never really ends.
        sqlx::query("update channels set archived_at = null where id = $1::uuid and archived_at is not null")
            .bind(&id)
            .execute(pg)
            .await
            .map_err(|e| crate::error::pg_message(&e))?;
        return Ok(CreatedChannel {
            id,
            name,
            topic,
            kind,
            created_at: epoch_ms_to_iso(created_ms),
            updated_at: epoch_ms_to_iso(updated_ms),
            role: "owner".into(),
        });
    }
    let mut tx = pg.begin().await.map_err(|e| crate::error::pg_message(&e))?;
    let (id, name, topic, kind, created_ms, updated_ms): (
        String,
        String,
        Option<String>,
        String,
        i64,
        i64,
    ) = sqlx::query_as(
        "insert into channels (name, topic, kind, dm_key, created_by) \
             values ('', null, 'dm', $1, $2::uuid) \
             on conflict (dm_key) where dm_key is not null do update set updated_at = now() \
         returning id::text, name, topic, kind, \
           (trunc(extract(epoch from created_at) * 1000))::bigint, \
           (trunc(extract(epoch from updated_at) * 1000))::bigint",
    )
    .bind(&dm_key)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| crate::error::pg_message(&e))?;
    for member in [user_id, other_user_id] {
        sqlx::query(
            "insert into channel_members (channel_id, user_id, role) \
             values ($1::uuid, $2::uuid, 'owner') on conflict do nothing",
        )
        .bind(&id)
        .bind(member)
        .execute(&mut *tx)
        .await
        .map_err(|e| crate::error::pg_message(&e))?;
    }
    tx.commit()
        .await
        .map_err(|e| crate::error::pg_message(&e))?;
    Ok(CreatedChannel {
        id,
        name,
        topic,
        kind,
        created_at: epoch_ms_to_iso(created_ms),
        updated_at: epoch_ms_to_iso(updated_ms),
        role: "owner".into(),
    })
}

/// Rename / set topic — separate statements (each present field its own
/// update), one publish after.
pub async fn update_channel(
    deps: &NotifyDeps,
    channel_id: &str,
    name: Option<&str>,
    topic: Option<Option<&str>>,
) -> Result<(), sqlx::Error> {
    if let Some(name) = name {
        sqlx::query("update channels set name = $2, updated_at = now() where id = $1::uuid")
            .bind(channel_id)
            .bind(name)
            .execute(&deps.pg)
            .await?;
    }
    if let Some(topic) = topic {
        sqlx::query("update channels set topic = $2, updated_at = now() where id = $1::uuid")
            .bind(channel_id)
            .bind(topic)
            .execute(&deps.pg)
            .await?;
    }
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "channel",
            message_id: None,
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

pub async fn archive_channel(deps: &NotifyDeps, channel_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("update channels set archived_at = now(), updated_at = now() where id = $1::uuid")
        .bind(channel_id)
        .execute(&deps.pg)
        .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "channel",
            message_id: None,
            seq: None,
            deleted: Some(true),
        },
    );
    Ok(())
}

pub async fn delete_channel(deps: &NotifyDeps, channel_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from channels where id = $1::uuid")
        .bind(channel_id)
        .execute(&deps.pg)
        .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "channel",
            message_id: None,
            seq: None,
            deleted: Some(true),
        },
    );
    Ok(())
}

// ── Members (humans) & agents ────────────────────────────────────────────────

pub async fn list_channel_members(
    pg: &PgPool,
    channel_id: &str,
) -> Result<Vec<ChannelMember>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select m.user_id::text, u.email, u.name, m.role \
         from channel_members m join users u on u.id = m.user_id \
         where m.channel_id = $1::uuid \
         order by (m.role = 'owner') desc, u.email asc",
    )
    .bind(channel_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, email, name, role)| ChannelMember {
            user_id,
            email,
            name,
            role,
        })
        .collect())
}

/// Add a member by email (they must have signed in before). `Some(sentence)`
/// is the 400 the route answers; `None` is ok.
pub async fn add_channel_member(
    deps: &NotifyDeps,
    channel_id: &str,
    email: &str,
) -> Result<Option<String>, sqlx::Error> {
    let user: Option<(String,)> =
        sqlx::query_as("select id::text from users where lower(email) = $1")
            .bind(email.trim().to_lowercase())
            .fetch_optional(&deps.pg)
            .await?;
    let Some((user_id,)) = user else {
        return Ok(Some("No user with that email has signed in yet".into()));
    };
    sqlx::query(
        "insert into channel_members (channel_id, user_id) values ($1::uuid, $2::uuid) \
         on conflict do nothing",
    )
    .bind(channel_id)
    .bind(&user_id)
    .execute(&deps.pg)
    .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "channel",
            message_id: None,
            seq: None,
            deleted: None,
        },
    );
    Ok(None)
}

pub async fn remove_channel_member(
    deps: &NotifyDeps,
    channel_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    // Never remove the owner via remove-member.
    sqlx::query(
        "delete from channel_members \
         where channel_id = $1::uuid and user_id = $2::uuid and role <> 'owner'",
    )
    .bind(channel_id)
    .bind(user_id)
    .execute(&deps.pg)
    .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "channel",
            message_id: None,
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

/// A channel's roster of agent models — empty for a non-uuid id rather than a
/// database error (see `is_channel_id`).
pub async fn list_channel_agents(
    pg: &PgPool,
    channel_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    if !is_channel_id(channel_id) {
        return Ok(Vec::new());
    }
    let rows: Vec<(String,)> = sqlx::query_as(
        "select agent_model from channel_agents where channel_id = $1::uuid order by agent_model",
    )
    .bind(channel_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(m,)| m).collect())
}

/// Channels a given agent has been added to. Takes the SUBJECT, never a bare
/// name — org-wide reach is only for a caller that PROVED its identity.
///
/// A PERSONAL ASSISTANT sees its owner's channels instead — the identity-proxy
/// model: the briefing chat's whole subject is the owner's attention state,
/// and a tool view that listed only the assistant's own memberships answered
/// that question with an empty list every time. This is the owner's OWN view,
/// DMs included; org-wide reach past the owner's memberships remains what it
/// always was: `elevated`, checked first.
pub async fn list_channels_for_agent(
    pg: &PgPool,
    subject: &AgentSubject,
) -> Result<Vec<AgentChannel>, sqlx::Error> {
    let model = subject_model(subject);
    // An elevated assistant sees every live channel and relay — never DMs
    // (human↔human direct messages stay private regardless of elevation).
    if is_elevated_assistant(pg, subject).await? {
        let rows: Vec<(String, String, Option<String>, String, i64, i64)> = sqlx::query_as(
            "select c.id::text, c.name, c.topic, c.kind, \
                 (trunc(extract(epoch from c.created_at) * 1000))::bigint, \
                 (trunc(extract(epoch from c.updated_at) * 1000))::bigint \
             from channels c where c.archived_at is null and c.kind <> 'dm' \
             order by c.updated_at desc",
        )
        .fetch_all(pg)
        .await?;
        return Ok(rows.into_iter().map(channel_of_row).collect());
    }
    if let Some(owner) = assistant_owner_for(pg, subject).await? {
        // The owner's member view carries role/unread/peer the agent view
        // doesn't — reduce to the bare columns the agent listing serves.
        return Ok(list_channels(pg, &owner)
            .await?
            .into_iter()
            .map(|c| AgentChannel {
                id: c.id,
                name: c.name,
                topic: c.topic,
                kind: c.kind,
                created_at: c.created_at,
                updated_at: c.updated_at,
            })
            .collect());
    }
    let rows: Vec<(String, String, Option<String>, String, i64, i64)> = sqlx::query_as(
        "select c.id::text, c.name, c.topic, c.kind, \
             (trunc(extract(epoch from c.created_at) * 1000))::bigint, \
             (trunc(extract(epoch from c.updated_at) * 1000))::bigint \
         from channels c join channel_agents a on a.channel_id = c.id and a.agent_model = $1 \
         where c.archived_at is null order by c.updated_at desc",
    )
    .bind(model)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(channel_of_row).collect())
}

fn channel_of_row(
    (id, name, topic, kind, created_ms, updated_ms): (
        String,
        String,
        Option<String>,
        String,
        i64,
        i64,
    ),
) -> AgentChannel {
    AgentChannel {
        id,
        name,
        topic,
        kind,
        created_at: epoch_ms_to_iso(created_ms),
        updated_at: epoch_ms_to_iso(updated_ms),
    }
}

/// May this agent read/post in this channel? Membership — or org-wide
/// elevation, which still never reaches DMs. Takes the SUBJECT, not its
/// model: elevation is only for a proven identity.
///
/// A personal assistant may read/post where its OWNER is a member — same
/// identity-proxy reach as `list_channels_for_agent`. Posts stay attributed
/// to the agent, so this grants the assistant its owner's VIEW, not the
/// ability to speak as its owner.
pub async fn agent_may_access_channel(
    pg: &PgPool,
    channel_id: &str,
    subject: &AgentSubject,
) -> Result<bool, sqlx::Error> {
    if !is_channel_id(channel_id) {
        return Ok(false);
    }
    if list_channel_agents(pg, channel_id)
        .await?
        .iter()
        .any(|m| m == subject_model(subject))
    {
        return Ok(true);
    }
    if is_elevated_assistant(pg, subject).await? {
        let row: Option<(i32,)> = sqlx::query_as(
            "select 1 from channels where id = $1::uuid and kind <> 'dm' and archived_at is null",
        )
        .bind(channel_id)
        .fetch_optional(pg)
        .await?;
        return Ok(row.is_some());
    }
    match assistant_owner_for(pg, subject).await? {
        Some(owner) => Ok(channel_role(pg, &owner, channel_id).await?.is_some()),
        None => Ok(false),
    }
}

pub async fn add_channel_agent(
    deps: &NotifyDeps,
    channel_id: &str,
    model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into channel_agents (channel_id, agent_model) values ($1::uuid, $2) \
         on conflict do nothing",
    )
    .bind(channel_id)
    .bind(model)
    .execute(&deps.pg)
    .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "channel",
            message_id: None,
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

pub async fn remove_channel_agent(
    deps: &NotifyDeps,
    channel_id: &str,
    model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from channel_agents where channel_id = $1::uuid and agent_model = $2")
        .bind(channel_id)
        .bind(model)
        .execute(&deps.pg)
        .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "channel",
            message_id: None,
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

// ── Messages ─────────────────────────────────────────────────────────────────

/// One message row in MSG_SELECT's key order (attachments before guard before
/// threadRootId/editedAt — the select's own column order); `reactions` and
/// `thread` arrive LAST, bolted on by decoration, and only when present.
/// This is the PAGE shape: guard/editedAt serialize even when null, because
/// MSG_SELECT selects them. The INSERT shape (neither key present) is
/// `inserted_wire` below.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessageWire {
    pub id: String,
    pub seq: i32,
    pub author_type: String,
    pub author: String,
    pub content: String,
    pub status: String,
    pub created_at: String,
    pub attachments: Value,
    pub guard: Option<Value>,
    pub thread_root_id: Option<String>,
    pub edited_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reactions: Option<Vec<ReactionRollup>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadRollup>,
}

/// The INSERT response's shape. The insert's RETURNING selects neither guard
/// nor editedAt, and decoration hasn't run — those keys are ABSENT, not null,
/// while the page select (MSG_SELECT) always carries guard/editedAt as nulls.
/// One struct can't say both, so the wire serializes the page shape and the
/// insert path projects down to its own.
pub fn inserted_wire(m: &ChannelMessageWire) -> Value {
    let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.remove("guard");
        obj.remove("editedAt");
    }
    v
}

/// Rolled-up reactions, actors in insertion order.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionRollup {
    pub emoji: String,
    pub actors: Vec<String>,
    pub actor_types: Vec<String>,
}

/// The rollup a thread root with replies carries in the main flow.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRollup {
    pub count: i32,
    pub authors: Vec<String>,
    pub last_at: String,
}

// The page queries, spelled whole so they stay `&'static str` — sqlx's
// SqlSafeStr lint rejects runtime-built SQL, and the only thing a composed
// version would interpolate is the thread filter. The three share MSG_SELECT's
// column order; `msg_wire` maps rows in exactly that order.
const MSG_PAGE_MAIN_FLOW: &str = "select id::text, seq, author_type, author, content, status, \
     (trunc(extract(epoch from created_at) * 1000))::bigint, attachments, guard, \
     thread_root_id::text, (trunc(extract(epoch from edited_at) * 1000))::bigint \
     from channel_messages where channel_id = $1::uuid and seq > $2 \
     and thread_root_id is null order by seq desc limit $3";
const MSG_PAGE_ALL: &str = "select id::text, seq, author_type, author, content, status, \
     (trunc(extract(epoch from created_at) * 1000))::bigint, attachments, guard, \
     thread_root_id::text, (trunc(extract(epoch from edited_at) * 1000))::bigint \
     from channel_messages where channel_id = $1::uuid and seq > $2 \
     order by seq desc limit $3";
const MSG_PAGE_THREAD: &str = "select id::text, seq, author_type, author, content, status, \
     (trunc(extract(epoch from created_at) * 1000))::bigint, attachments, guard, \
     thread_root_id::text, (trunc(extract(epoch from edited_at) * 1000))::bigint \
     from channel_messages where channel_id = $1::uuid \
     and (id = $2::uuid or thread_root_id = $2::uuid) order by seq asc limit 300";

type MsgRow = (
    String,
    i32,
    String,
    String,
    String,
    String,
    i64,
    Value,
    Option<Value>,
    Option<String>,
    Option<i64>,
);

fn msg_wire(
    (
        id,
        seq,
        author_type,
        author,
        content,
        status,
        created_ms,
        attachments,
        guard,
        thread_root_id,
        edited_ms,
    ): MsgRow,
) -> ChannelMessageWire {
    ChannelMessageWire {
        id,
        seq,
        author_type,
        author,
        content,
        status,
        created_at: epoch_ms_to_iso(created_ms),
        attachments,
        guard,
        thread_root_id,
        edited_at: edited_ms.map(epoch_ms_to_iso),
        reactions: None,
        thread: None,
    }
}

/// Bolt reaction rollups + thread rollups onto a fetched page of messages.
async fn decorate_messages(
    pg: &PgPool,
    messages: &mut [ChannelMessageWire],
) -> Result<(), sqlx::Error> {
    if messages.is_empty() {
        return Ok(());
    }
    let ids: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
    let reactions: Vec<(String, String, Vec<String>, Vec<String>)> = sqlx::query_as(
        "select message_id::text, emoji, \
             array_agg(actor order by created_at), \
             array_agg(actor_type order by created_at) \
         from channel_message_reactions where message_id = any($1::uuid[]) \
         group by message_id, emoji \
         order by min(created_at)",
    )
    .bind(&ids)
    .fetch_all(pg)
    .await?;
    let threads: Vec<(String, i32, i64, Vec<String>)> = sqlx::query_as(
        "select thread_root_id::text, count(*)::int, \
             (trunc(extract(epoch from max(created_at)) * 1000))::bigint, \
             (array_agg(distinct author))[1:4] \
         from channel_messages where thread_root_id = any($1::uuid[]) \
         group by thread_root_id",
    )
    .bind(&ids)
    .fetch_all(pg)
    .await?;
    for m in messages.iter_mut() {
        let rs: Vec<ReactionRollup> = reactions
            .iter()
            .filter(|(id, _, _, _)| id == &m.id)
            .map(|(_, emoji, actors, actor_types)| ReactionRollup {
                emoji: emoji.clone(),
                actors: actors.clone(),
                actor_types: actor_types.clone(),
            })
            .collect();
        if !rs.is_empty() {
            m.reactions = Some(rs);
        }
        if let Some((_, count, last_ms, authors)) = threads.iter().find(|(id, _, _, _)| id == &m.id)
        {
            m.thread = Some(ThreadRollup {
                count: *count,
                authors: authors.clone(),
                last_at: epoch_ms_to_iso(*last_ms),
            });
        }
    }
    Ok(())
}

/// A channel's MAIN flow (thread replies live in their panels), oldest first.
/// `since_seq` fetches only newer ones. `include_threads` flattens everything
/// back in — the distill/conclude summarizers want the whole conversation.
pub async fn list_channel_messages(
    pg: &PgPool,
    channel_id: &str,
    since_seq: i64,
    limit: i64,
    include_threads: bool,
) -> Result<Vec<ChannelMessageWire>, sqlx::Error> {
    let sql = if include_threads {
        MSG_PAGE_ALL
    } else {
        MSG_PAGE_MAIN_FLOW
    };
    let mut rows: Vec<MsgRow> = sqlx::query_as(sql)
        .bind(channel_id)
        .bind(since_seq)
        .bind(limit)
        .fetch_all(pg)
        .await?;
    rows.reverse(); // newest-first fetch, oldest-first wire
    let mut messages: Vec<ChannelMessageWire> = rows.into_iter().map(msg_wire).collect();
    decorate_messages(pg, &mut messages).await?;
    Ok(messages)
}

/// One thread: its root + replies, oldest first.
pub async fn list_thread_messages(
    pg: &PgPool,
    channel_id: &str,
    root_id: &str,
) -> Result<Vec<ChannelMessageWire>, sqlx::Error> {
    let mut messages: Vec<ChannelMessageWire> = sqlx::query_as(MSG_PAGE_THREAD)
        .bind(channel_id)
        .bind(root_id)
        .fetch_all(pg)
        .await?
        .into_iter()
        .map(msg_wire)
        .collect();
    decorate_messages(pg, &mut messages).await?;
    Ok(messages)
}

/// Insert a message, drawing seq from the channel's counter.
///
/// THE AGENT-POST GUARD LIVES HERE. `mcp post_to_channel` arrives as a tool
/// ARGUMENT — model output that never went through a harness — and lands in a
/// room everybody reads. `guard_agent_write` runs the gate-safe rules over it,
/// records what they find against the posting agent, and in strict mode stores
/// the redacted body. It runs on agent posts only: a person's message is not
/// model output, and guard_findings.model has to keep meaning "this model's
/// confabulation rate".
///
/// Inside the insert rather than at the route, so no caller can express the
/// ungated post — and the streamed reply path costs nothing for it, because
/// that path inserts an EMPTY row and fills it in through
/// `update_channel_message`, where the guard already stands.
///
/// Findings are recorded, not PINNED to the row. Agents read channels through
/// the same API humans do — so pinning a finding here would put its `snippet`
/// (a verbatim excerpt of the flagged span) on a path back into a model's
/// context, which is the one thing guardrails forbids outright.
#[allow(clippy::too_many_arguments)] // the insert's inputs are the row's — all eight name one
pub async fn insert_channel_message(
    deps: &NotifyDeps,
    channel_id: &str,
    author_type: &str,
    author: &str,
    content: &str,
    status: &str,
    attachments: &Value,
    thread_root_id: Option<&str>,
) -> Result<ChannelMessageWire, sqlx::Error> {
    let body = if author_type == "agent" {
        guard_agent_write(
            &deps.pg,
            "channel-post",
            WriteAuthor::Agent(author),
            content,
            None,
        )
        .await
        .text
    } else {
        content.to_string()
    };

    let mut tx = deps.pg.begin().await?;
    let seq: i32 = sqlx::query_scalar(
        "update channels set msg_seq = msg_seq + 1, updated_at = now() where id = $1::uuid returning msg_seq",
    )
    .bind(channel_id)
    .fetch_one(&mut *tx)
    .await?;
    let row: (String, i32, String, String, String, String, i64, Value, Option<String>) =
        sqlx::query_as(
            "insert into channel_messages (channel_id, seq, author_type, author, content, status, attachments, thread_root_id) \
             values ($1::uuid, $2, $3, $4, $5, $6, $7, $8::uuid) \
             returning id::text, seq, author_type, author, content, status, \
               (trunc(extract(epoch from created_at) * 1000))::bigint, attachments, \
               thread_root_id::text",
        )
        .bind(channel_id)
        .bind(seq)
        .bind(author_type)
        .bind(author)
        .bind(&body)
        .bind(status)
        .bind(attachments)
        .bind(thread_root_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    let message = ChannelMessageWire {
        id: row.0,
        seq: row.1,
        author_type: row.2,
        author: row.3,
        content: row.4,
        status: row.5,
        created_at: epoch_ms_to_iso(row.6),
        attachments: row.7,
        guard: None,
        thread_root_id: row.8,
        edited_at: None,
        reactions: None,
        thread: None,
    };
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "message",
            message_id: Some(message.id.clone()),
            seq: Some(message.seq as i64),
            deleted: None,
        },
    );
    // THE BRIEF FOLLOWS THE CONVERSATION. A brief line says who is waiting on a
    // reply, and until this existed it learned that the reply had happened on
    // the next scheduled sweep — up to five minutes of the document telling you
    // to answer somebody you had just answered. This clears the sweep throttle
    // and rings the bell; it does not sweep.
    briefs_follow_message(deps.clone(), channel_id.to_string());
    Ok(message)
}

/// The row an edit/delete/react targets, for permission checks.
#[derive(Debug, Clone)]
pub struct MessageRef {
    pub id: String,
    pub author_type: String,
    pub author: String,
    pub thread_root_id: Option<String>,
}

pub async fn get_channel_message(
    pg: &PgPool,
    channel_id: &str,
    message_id: &str,
) -> Result<Option<MessageRef>, sqlx::Error> {
    let row: Option<(String, String, String, Option<String>)> = sqlx::query_as(
        "select id::text, author_type, author, thread_root_id::text \
         from channel_messages where id = $1::uuid and channel_id = $2::uuid",
    )
    .bind(message_id)
    .bind(channel_id)
    .fetch_optional(pg)
    .await?;
    Ok(
        row.map(|(id, author_type, author, thread_root_id)| MessageRef {
            id,
            author_type,
            author,
            thread_root_id,
        }),
    )
}

/// Toggle a reaction (add if absent, remove if present). Anyone in the room —
/// human or agent — reacts under their own identity.
pub async fn toggle_reaction(
    deps: &NotifyDeps,
    channel_id: &str,
    message_id: &str,
    emoji: &str,
    actor: &str,
    actor_type: &str,
) -> Result<(), sqlx::Error> {
    let removed = sqlx::query(
        "delete from channel_message_reactions \
         where message_id = $1::uuid and emoji = $2 and actor = $3 returning 1",
    )
    .bind(message_id)
    .bind(emoji)
    .bind(actor)
    .execute(&deps.pg)
    .await?;
    if removed.rows_affected() == 0 {
        sqlx::query(
            "insert into channel_message_reactions (message_id, emoji, actor, actor_type) \
             values ($1::uuid, $2, $3, $4) on conflict do nothing",
        )
        .bind(message_id)
        .bind(emoji)
        .bind(actor)
        .bind(actor_type)
        .execute(&deps.pg)
        .await?;
    }
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "message",
            message_id: Some(message_id.to_string()),
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

/// Author-gated edit: new content, edited marker, republish.
pub async fn edit_channel_message(
    deps: &NotifyDeps,
    channel_id: &str,
    message_id: &str,
    content: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("update channel_messages set content = $2, edited_at = now() where id = $1::uuid")
        .bind(message_id)
        .bind(content)
        .execute(&deps.pg)
        .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "message",
            message_id: Some(message_id.to_string()),
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

/// Hard delete (author or channel owner). A thread root takes its replies
/// with it (FK cascade) — the confirm dialog says so.
pub async fn delete_channel_message(
    deps: &NotifyDeps,
    channel_id: &str,
    message_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from channel_messages where id = $1::uuid")
        .bind(message_id)
        .execute(&deps.pg)
        .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "message",
            message_id: Some(message_id.to_string()),
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

/// Pin confab-guard findings to an agent reply (annotate/strict); strict may
/// also pass secret-redacted content to overwrite the saved copy. Republishes
/// so live viewers pick the caveat up.
///
/// THE FINDINGS ARE ALREADY SCRUBBED when they get here — the caller pins
/// `redact_findings`' output, never the raw list, because a pinned finding
/// carries a verbatim excerpt of the flagged span and `zero_tool_claim` does
/// not truncate its own.
pub async fn set_channel_message_guard(
    deps: &NotifyDeps,
    channel_id: &str,
    message_id: &str,
    findings: &[crate::gateway::guard::Finding],
    redacted_content: Option<&str>,
) -> Result<(), sqlx::Error> {
    let guard = serde_json::to_value(findings).unwrap_or(Value::Array(Vec::new()));
    match redacted_content {
        Some(content) => {
            sqlx::query("update channel_messages set guard = $3, content = $4 where id = $1::uuid")
                .bind(message_id)
                .bind(channel_id)
                .bind(&guard)
                .bind(content)
                .execute(&deps.pg)
                .await?;
        }
        None => {
            sqlx::query("update channel_messages set guard = $3 where id = $1::uuid")
                .bind(message_id)
                .bind(channel_id)
                .bind(&guard)
                .execute(&deps.pg)
                .await?;
        }
    }
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "message",
            message_id: Some(message_id.to_string()),
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

/// Flush accumulated agent-reply state (throttled during streaming, final at
/// end).
pub async fn update_channel_message(
    deps: &NotifyDeps,
    channel_id: &str,
    message_id: &str,
    content: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("update channel_messages set content = $3, status = $4 where id = $1::uuid")
        .bind(message_id)
        .bind(content)
        .bind(status)
        .execute(&deps.pg)
        .await?;
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "message",
            message_id: Some(message_id.to_string()),
            seq: None,
            deleted: None,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_channel_id_matches_exactly_the_hyphenated_uuid_shape() {
        let good = "6f9619ff-8b86-d011-b42d-00c04fc964ff";
        assert!(is_channel_id(good));
        // The regex is case-insensitive: uppercase hex is still hex.
        assert!(is_channel_id("6F9619FF-8B86-D011-B42D-00C04FC964FF"));
        // Everything else a looser parser would let through, and shouldn't.
        assert!(!is_channel_id("6f9619ff8b86d011b42d00c04fc964ff")); // no hyphens
        assert!(!is_channel_id("{6f9619ff-8b86-d011-b42d-00c04fc964ff}")); // braced
        assert!(!is_channel_id(
            "urn:uuid:6f9619ff-8b86-d011-b42d-00c04fc964ff"
        ));
        assert!(!is_channel_id("")); // empty
        assert!(!is_channel_id("c1")); // a test-fixture id is not a channel id
        assert!(!is_channel_id("6f9619ff-8b86-d011-b42d-00c04fc964fg")); // g is not hex
    }

    #[test]
    fn member_view_serializes_in_the_wire_key_order() {
        // id..updatedAt, unreadCount, then peer LAST,
        // peer null for non-DMs.
        let ch = MemberChannel {
            id: "c1".into(),
            name: "general".into(),
            topic: None,
            kind: "channel".into(),
            role: "owner".into(),
            created_at: "2026-08-29T10:00:00.000Z".into(),
            updated_at: "2026-08-29T10:00:00.000Z".into(),
            unread_count: 0,
            peer: None,
        };
        assert_eq!(
            serde_json::to_string(&ch).unwrap(),
            r#"{"id":"c1","name":"general","topic":null,"kind":"channel","role":"owner","createdAt":"2026-08-29T10:00:00.000Z","updatedAt":"2026-08-29T10:00:00.000Z","unreadCount":0,"peer":null}"#
        );
    }

    #[test]
    fn created_view_appends_role_last() {
        let ch = CreatedChannel {
            id: "c1".into(),
            name: "".into(),
            topic: None,
            kind: "dm".into(),
            created_at: "2026-08-29T10:00:00.000Z".into(),
            updated_at: "2026-08-29T10:00:00.000Z".into(),
            role: "owner".into(),
        };
        assert!(
            serde_json::to_string(&ch)
                .unwrap()
                .ends_with(r#""role":"owner"}"#)
        );
    }

    #[test]
    fn inserted_message_has_no_guard_editedat_or_decoration_keys() {
        // The insert's RETURNING carries neither guard nor editedAt, and
        // decoration hasn't run — those keys are absent, not null.
        let m = ChannelMessageWire {
            id: "m1".into(),
            seq: 3,
            author_type: "user".into(),
            author: "a@x".into(),
            content: "hi".into(),
            status: "complete".into(),
            created_at: "2026-08-29T10:00:00.000Z".into(),
            attachments: serde_json::json!([]),
            guard: None,
            thread_root_id: None,
            edited_at: None,
            reactions: None,
            thread: None,
        };
        let wire = serde_json::to_string(&inserted_wire(&m)).unwrap();
        assert_eq!(
            wire,
            r#"{"id":"m1","seq":3,"authorType":"user","author":"a@x","content":"hi","status":"complete","createdAt":"2026-08-29T10:00:00.000Z","attachments":[],"threadRootId":null}"#
        );
        assert!(!wire.contains(r#""guard""#));
        assert!(!wire.contains(r#""editedAt""#));
        assert!(!wire.contains(r#""reactions""#));
    }

    #[test]
    fn decorated_message_keeps_msg_select_order_then_appends_rollups() {
        let m = ChannelMessageWire {
            id: "m1".into(),
            seq: 1,
            author_type: "agent".into(),
            author: "assistant-operations".into(),
            content: "done".into(),
            status: "complete".into(),
            created_at: "2026-08-29T10:00:00.000Z".into(),
            attachments: serde_json::json!([]),
            guard: None,
            thread_root_id: None,
            edited_at: Some("2026-08-29T11:00:00.000Z".into()),
            reactions: Some(vec![ReactionRollup {
                emoji: "🎉".into(),
                actors: vec!["a@x".into()],
                actor_types: vec!["user".into()],
            }]),
            thread: Some(ThreadRollup {
                count: 2,
                authors: vec!["a@x".into(), "b@x".into()],
                last_at: "2026-08-29T12:00:00.000Z".into(),
            }),
        };
        let wire = serde_json::to_string(&m).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&wire).unwrap();
        let keys: Vec<&str> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "id",
                "seq",
                "authorType",
                "author",
                "content",
                "status",
                "createdAt",
                "attachments",
                "guard",
                "threadRootId",
                "editedAt",
                "reactions",
                "thread"
            ]
        );
    }
}
