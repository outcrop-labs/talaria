// Channels — the port of ui/src/server/channels.ts, grown slice by slice.
// The role read landed with the runs watch gate; `insert_channel_message`
// lands here because the brief's delegation layer sends replies through it.
// The agents listing and the message-page read land with the plan-draft
// plane (a channel draft reads the room's main flow as its transcript).
// The CRUD, read-marking, and stream-fill planes land with the chat/channels
// family's own batch.

use crate::agent_writes::{WriteAuthor, guard_agent_write};
use crate::notify::{NotifyDeps, briefs_follow_message};
use crate::realtime::{ChannelEvent, publish_channel};
use sqlx::PgPool;

/// channels.ts isChannelId: exactly the hyphenated uuid shape, hex in either
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

/// channels.ts channelRole: the caller's row in `channel_members`, or null.
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

/// What an insert handed back — the two facts every caller needs (the brief's
/// delegation records the message id on the draft it sent).
#[derive(Debug, Clone)]
pub struct InsertedMessage {
    pub id: String,
    pub seq: i64,
}

/// Insert a message, drawing seq from the channel's counter
/// (channels.ts insertChannelMessage — the shape the brief calls: status
/// always `complete`, no attachments, no thread root).
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
/// ungated post.
///
/// Findings are recorded, not PINNED to the row. Agents read channels through
/// the same API humans do — so pinning a finding here would put its `snippet`
/// (a verbatim excerpt of the flagged span) on a path back into a model's
/// context, which is the one thing guardrails forbids outright.
pub async fn insert_channel_message(
    deps: &NotifyDeps,
    channel_id: &str,
    author_type: &str,
    author: &str,
    content: &str,
) -> Result<InsertedMessage, sqlx::Error> {
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
    let row: (String, i32) = sqlx::query_as(
        "insert into channel_messages (channel_id, seq, author_type, author, content, status, attachments, thread_root_id) \
         values ($1::uuid, $2, $3, $4, $5, 'complete', '[]'::jsonb, null) \
         returning id::text, seq",
    )
    .bind(channel_id)
    .bind(seq)
    .bind(author_type)
    .bind(author)
    .bind(&body)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let inserted = InsertedMessage {
        id: row.0,
        seq: row.1 as i64,
    };
    publish_channel(
        &deps.realtime,
        channel_id,
        &ChannelEvent {
            kind_tag: "message",
            message_id: Some(inserted.id.clone()),
            seq: Some(inserted.seq),
            deleted: None,
        },
    );
    // THE BRIEF FOLLOWS THE CONVERSATION. A brief line says who is waiting on a
    // reply, and until this existed it learned that the reply had happened on
    // the next scheduled sweep — up to five minutes of the document telling you
    // to answer somebody you had just answered. This clears the sweep throttle
    // and rings the bell; it does not sweep.
    briefs_follow_message(deps.clone(), channel_id.to_string());
    Ok(inserted)
}

/// A channel's roster of agent models (channels.ts listChannelAgents) — the
/// set the plan-draft POST checks membership in. Empty for a non-uuid id
/// rather than a database error, exactly as TS guards it (see is_channel_id).
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

/// One row of a channel's MAIN flow, in the columns a transcript reads
/// (channels.ts ChannelMessage, reduced to what planFromChannel consumes).
#[derive(Debug, Clone)]
pub struct ChannelMessage {
    pub author_type: String,
    pub author: String,
    pub status: String,
    pub content: String,
}

/// The main flow (thread replies excluded), oldest first — channels.ts
/// listChannelMessages for the one call shape the plan draft makes:
/// `listChannelMessages(channelId, -1, 80)`, which is the LAST 80 main-flow
/// messages (the query is `order by seq desc limit 80`, reversed).
///
/// DECORATION DELIBERATELY SKIPPED: decorateMessages bolts reaction rollups
/// and thread rollups onto each row, and the transcript reads none of them —
/// it filters on `status`/`content` and interpolates `authorType`/`author`.
/// Skipping the two extra queries is byte-identical output for this caller;
/// when a decorated reader crosses (the chat family), this grows the full
/// row and the decoration in place.
pub async fn list_channel_messages(
    pg: &PgPool,
    channel_id: &str,
    since_seq: i64,
    limit: i64,
) -> Result<Vec<ChannelMessage>, sqlx::Error> {
    let mut rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "select author_type, author, status, content from channel_messages \
         where channel_id = $1::uuid and seq > $2 and thread_root_id is null \
         order by seq desc limit $3",
    )
    .bind(channel_id)
    .bind(since_seq)
    .bind(limit)
    .fetch_all(pg)
    .await?;
    rows.reverse();
    Ok(rows
        .into_iter()
        .map(|(author_type, author, status, content)| ChannelMessage {
            author_type,
            author,
            status,
            content,
        })
        .collect())
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
}
