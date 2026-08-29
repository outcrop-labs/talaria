// Channels — the port of ui/src/server/channels.ts, grown slice by slice.
// This file starts with the one piece the runs watch gate needs (realtime.rs
// resolves a run with a `channel` subject through `channelRole`); the CRUD,
// read-marking, and message planes land with the chat/channels family's own
// batch.

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
