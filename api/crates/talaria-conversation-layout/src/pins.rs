// WHAT ONE PERSON PINNED TO THE TOP OF THEIR COMMS RAIL (`comms_pins`).
//
// The Comms rail's Pinned section lists a person's channels (a relay and a DM
// are channels), agents and single agent threads, in the order they pinned
// them. It is the same idea as the `pinned` flag on a thread's layout row
// above, at the rail's grain: a pin can name an agent, which no layout row can.
// It lived in each browser's localStorage until this table, so a pin made on
// one machine never reached another.
//
// KEYS ARE THE RAIL'S OWN GRAMMAR, validated here and nowhere else:
//
//   channel:<uuid>   a channel, a relay, or a DM
//   agent:<model>    an agent
//   thread:<uuid>    one thread with an agent
//
// A key is the caller's own filing, so writing one reveals nothing and needs
// no access check: a pin on a channel the person cannot see lists nothing,
// because the rail only draws pins for rows it already has.

use sqlx::PgPool;

/// Enough for anybody's rail, small enough that the list stays one read.
pub const MAX_PINS: i64 = 200;
/// An agent model is a slug-like id; the longest in a fleet is far shorter.
pub const MAX_AGENT_MODEL: usize = 200;

/// The canonical hyphenated uuid the rail holds (36 characters).
fn canonical_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The pin key as stored, or the 400 sentence that refuses it. A uuid is
/// lowercased, so one channel cannot be pinned twice under two spellings.
pub fn parse_pin_key(raw: &str) -> Result<String, String> {
    let Some((kind, id)) = raw.split_once(':') else {
        return Err("key must be channel:<id>, agent:<model> or thread:<id>".into());
    };
    match kind {
        "channel" | "thread" => {
            if canonical_uuid(id) {
                Ok(format!("{kind}:{}", id.to_ascii_lowercase()))
            } else {
                Err(format!("a {kind} pin names a uuid"))
            }
        }
        "agent" => {
            if id.is_empty()
                || id.chars().count() > MAX_AGENT_MODEL
                || id.chars().any(char::is_control)
            {
                Err(format!(
                    "an agent pin names a model of 1 to {MAX_AGENT_MODEL} characters"
                ))
            } else {
                Ok(raw.to_string())
            }
        }
        _ => Err("key must be channel:<id>, agent:<model> or thread:<id>".into()),
    }
}

/// The person's pins, oldest first — the order the rail lists them in.
pub async fn list_pins(pg: &PgPool, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "select pin_key from comms_pins where user_id = $1::uuid order by created_at, pin_key",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await
}

/// What a pin write did.
#[derive(Debug, PartialEq, Eq)]
pub enum PinAdd {
    /// Pinned now, or already pinned (idempotent, and it keeps its place).
    Pinned,
    /// The person is at `MAX_PINS` and this key is not one of them.
    Full,
}

/// Pin `key` (already through `parse_pin_key`) for the person.
///
/// THE CAP HOLDS UNDER CONCURRENT WRITES: the count and the insert run in one
/// transaction behind a lock on the person's own `users` row, so two pins
/// sent at once cannot both read 199 and land 201. A pin whose channel,
/// thread or agent has since gone still counts toward the cap until it is
/// unpinned — the rail draws nothing for it, so a person can meet the cap
/// with fewer rows on screen; the refusal names the cap for that reason.
pub async fn add_pin(pg: &PgPool, user_id: &str, key: &str) -> Result<PinAdd, sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("select 1 from users where id = $1::uuid for update")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    // A key already held conflicts into a no-op that keeps its place; a new
    // one lands only while the person holds fewer than the cap.
    let inserted = sqlx::query(
        "insert into comms_pins (user_id, pin_key) \
         select $1::uuid, $2 \
         where (select count(*) from comms_pins where user_id = $1::uuid) < $3 \
         on conflict (user_id, pin_key) do nothing",
    )
    .bind(user_id)
    .bind(key)
    .bind(MAX_PINS)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let outcome = if inserted > 0 {
        PinAdd::Pinned
    } else {
        let held: bool = sqlx::query_scalar(
            "select exists (select 1 from comms_pins where user_id = $1::uuid and pin_key = $2)",
        )
        .bind(user_id)
        .bind(key)
        .fetch_one(&mut *tx)
        .await?;
        if held { PinAdd::Pinned } else { PinAdd::Full }
    };
    tx.commit().await?;
    Ok(outcome)
}

/// Unpin `key`. Unpinning what was never pinned is the same answer.
pub async fn remove_pin(pg: &PgPool, user_id: &str, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from comms_pins where user_id = $1::uuid and pin_key = $2")
        .bind(user_id)
        .bind(key)
        .execute(pg)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_take_the_rails_grammar_and_nothing_else() {
        let id = "0F8FAD5B-D9CB-469F-A165-70867728950E";
        assert_eq!(
            parse_pin_key(&format!("channel:{id}")).unwrap(),
            "channel:0f8fad5b-d9cb-469f-a165-70867728950e"
        );
        assert_eq!(
            parse_pin_key(&format!("thread:{}", id.to_lowercase())).unwrap(),
            format!("thread:{}", id.to_lowercase())
        );
        assert_eq!(parse_pin_key("agent:developer").unwrap(), "agent:developer");
        // A model may itself hold a colon; only the first one splits.
        assert_eq!(parse_pin_key("agent:ns:dev").unwrap(), "agent:ns:dev");
        assert_eq!(
            parse_pin_key("channel:nope").unwrap_err(),
            "a channel pin names a uuid"
        );
        assert_eq!(
            parse_pin_key("thread:").unwrap_err(),
            "a thread pin names a uuid"
        );
        assert!(parse_pin_key("agent:").is_err());
        assert!(parse_pin_key("agent:a\nb").is_err());
        assert!(parse_pin_key(&format!("agent:{}", "a".repeat(MAX_AGENT_MODEL + 1))).is_err());
        assert!(parse_pin_key(&format!("agent:{}", "a".repeat(MAX_AGENT_MODEL))).is_ok());
        assert!(parse_pin_key("person:x").is_err());
        assert!(parse_pin_key("channel").is_err());
    }
}
