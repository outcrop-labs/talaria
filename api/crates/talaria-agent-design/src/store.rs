// The design record in Redis: one hash per person, `agent-design:<user>`,
// holding `json` (the record) and `rev` (its revision), with a TTL.
//
// REDIS IS THE TRUTH, NOT A CACHE. Any api process may serve the GET, run the
// step, or take the discard, so nothing about a design lives in process
// memory. Each write is a compare-and-set on `rev` in one Lua script — read,
// decide in Rust (record.rs), swap only if nobody wrote in between, retry if
// they did — so a run landing, a refine starting and a discard arriving at
// once on three processes cannot interleave into a record none of them wrote.
// A hash rather than a JSON string so the script compares a field and never
// parses the record.

use redis::aio::ConnectionManager;
use redis::{AsyncCommands, RedisResult};
use std::sync::LazyLock;

use crate::record::AgentDesign;

/// How long a design waits to be hired or discarded: three days, refreshed on
/// every write. Long enough for a design started on Friday to be reviewed on
/// Monday morning; past it the record goes, and so does its link.
pub const DESIGN_TTL_SECS: u64 = 72 * 3600;

/// How long one "the modal is open on this design" mark lasts. The open
/// modal's poll renews it every few seconds; a landing that finds it skips
/// the bell, because the person is already looking at the answer.
pub const WATCH_TTL_SECS: u64 = 12;

/// How many times a write re-reads after losing a race before it gives up.
const SWAP_TRIES: usize = 8;

pub fn key(user_id: &str) -> String {
    format!("agent-design:{user_id}")
}

pub(crate) fn watch_key(user_id: &str) -> String {
    format!("agent-design:{user_id}:watch")
}

/// ARGV: expected rev ('' = no record), new rev ('' = delete), json, ttl.
static SWAP: LazyLock<redis::Script> = LazyLock::new(|| {
    redis::Script::new(
        r"
local rev = redis.call('HGET', KEYS[1], 'rev') or ''
if rev ~= ARGV[1] then return 0 end
if ARGV[2] == '' then
  redis.call('DEL', KEYS[1])
  return 1
end
redis.call('HSET', KEYS[1], 'rev', ARGV[2], 'json', ARGV[3])
redis.call('EXPIRE', KEYS[1], ARGV[4])
return 1
",
    )
});

/// The record and the revision a swap must name.
#[derive(Debug, Clone)]
pub struct Stored {
    pub rev: u64,
    pub design: AgentDesign,
}

/// The person's design, if they have one. A record that no longer parses (a
/// shape from a build that predates a field) is removed rather than left to
/// refuse every write behind it.
pub async fn read(conn: &mut ConnectionManager, user_id: &str) -> RedisResult<Option<Stored>> {
    let (rev, json): (Option<String>, Option<String>) = redis::cmd("HMGET")
        .arg(key(user_id))
        .arg("rev")
        .arg("json")
        .query_async(conn)
        .await?;
    let (Some(rev), Some(json)) = (rev, json) else {
        return Ok(None);
    };
    match (
        rev.parse::<u64>(),
        serde_json::from_str::<AgentDesign>(&json),
    ) {
        (Ok(rev), Ok(design)) => Ok(Some(Stored { rev, design })),
        _ => {
            tracing::warn!("[agent-design] unreadable record for {user_id}; removed");
            let _: () = conn.del(key(user_id)).await?;
            Ok(None)
        }
    }
}

/// What a write decided, given the record as it stands.
pub enum Write<T> {
    /// Replace the record (or create it). Boxed: a record is large and the
    /// other two carry only the answer.
    Put(Box<AgentDesign>, T),
    /// Remove the record.
    Delete(T),
    /// Change nothing.
    Keep(T),
}

impl<T> Write<T> {
    /// `Put`, boxing the record.
    pub fn put(design: AgentDesign, out: T) -> Self {
        Write::Put(Box::new(design), out)
    }
}

/// Read, decide, swap — again from the top whenever another writer got there
/// first. `decide` may run more than once, so it must not have side effects.
pub async fn update<T>(
    conn: &mut ConnectionManager,
    user_id: &str,
    mut decide: impl FnMut(Option<&AgentDesign>) -> Write<T>,
) -> RedisResult<T> {
    for _ in 0..SWAP_TRIES {
        let current = read(conn, user_id).await?;
        let expected = current
            .as_ref()
            .map(|s| s.rev.to_string())
            .unwrap_or_default();
        let next_rev = current.as_ref().map_or(1, |s| s.rev + 1).to_string();
        let (new_rev, json, out) = match decide(current.as_ref().map(|s| &s.design)) {
            Write::Keep(out) => return Ok(out),
            Write::Delete(out) => (String::new(), String::new(), out),
            Write::Put(design, out) => (
                next_rev,
                serde_json::to_string(&design).expect("a design serializes"),
                out,
            ),
        };
        let swapped: i32 = SWAP
            .key(key(user_id))
            .arg(expected)
            .arg(new_rev)
            .arg(json)
            .arg(DESIGN_TTL_SECS)
            .invoke_async(conn)
            .await?;
        if swapped == 1 {
            return Ok(out);
        }
    }
    Err(redis::RedisError::from((
        redis::ErrorKind::Client,
        "agent design write kept losing the race",
    )))
}

/// The open modal is looking at this person's design right now.
pub async fn mark_watching(conn: &mut ConnectionManager, user_id: &str) -> RedisResult<()> {
    conn.set_ex(watch_key(user_id), 1, WATCH_TTL_SECS).await
}

pub async fn watched(conn: &mut ConnectionManager, user_id: &str) -> RedisResult<bool> {
    conn.exists(watch_key(user_id)).await
}
