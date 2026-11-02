// The brief nudge every write and read rides: clear the sweep throttle and
// ring the bell.

use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;

/// Mark these people's current brief as needing a sweep, AND tell
/// their open pages it moved.
///
/// Bounded to the last 48 hours so a long-lived account does not have every
/// brief it has ever had rewritten by one DM. Only today's is ever swept, so
/// older rows are noise either way — the window is about the size of the
/// UPDATE, not about correctness. What it DOES NOT DO is sweep: a sweep is
/// four scoped queries and up to one model call; this clears the throttle and
/// rings the bell, and the next read does the work.
pub async fn mark_brief_stale(deps: &NotifyDeps, user_ids: &[String]) -> Result<(), sqlx::Error> {
    // Deduped, and an empty id is nobody's brief, not a row to look up.
    let mut ids: Vec<String> = user_ids.to_vec();
    ids.sort();
    ids.dedup();
    ids.retain(|id| !id.is_empty());
    if ids.is_empty() {
        return Ok(());
    }
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "update daily_briefs set last_swept_at = null \
         where user_id = any($1::uuid[]) and created_at > now() - interval '48 hours' \
         returning id::text, user_id::text, last_seq::int8",
    )
    .bind(&ids)
    .fetch_all(&deps.pg)
    .await?;
    // Id-shaped, like every other user event: the payload says a brief moved
    // and the client re-reads through the ordinary route with the ordinary ACL.
    for (brief_id, user_id, seq) in rows {
        publish_user(
            &deps.realtime,
            &user_id,
            &UserEvent::Brief { brief_id, seq },
        );
    }
    Ok(())
}
