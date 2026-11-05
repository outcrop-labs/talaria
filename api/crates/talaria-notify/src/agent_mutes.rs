// A PERSON'S TIMED MUTE OF ONE AGENT'S DIRECT MESSAGES (`agent_dm_mutes`).
//
// An agent's direct messages are two kinds of `dm`-class row: its reply in
// one of the person's CHAT threads with it (`agent-reply` on a `chat`
// conversation, `notify_agent_reply`) and its own check-in (`agent-outreach`,
// which lands in a chat thread too). This is the channel mute's twin for
// those (`channel_members.muted_until`): while a mute holds, both producers
// file their row `quiet` — it lands in-app and counts, and no toast, banner,
// push or mail goes out. The agent's replies anywhere else — a plan, a
// research run, a work session, a ticket — are not its DMs and ring as ever.
// A mute always has an end; "until I turn it back on" is FOREVER_MS, as it is
// for a channel. An expired row reads as no mute; a write prunes the
// person's expired rows, and unmuting deletes the row.

use sqlx::PgPool;

/// Does `user_id` have `agent_model`'s DMs muted right now? A failed read
/// answers false: a mute is a courtesy, and a lost read must not swallow a
/// reply nobody asked to silence.
pub async fn agent_dm_muted(pg: &PgPool, user_id: &str, agent_model: &str) -> bool {
    match sqlx::query_scalar::<_, bool>(
        "select exists (select 1 from agent_dm_mutes \
           where user_id = $1::uuid and agent_model = $2 and muted_until > now())",
    )
    .bind(user_id)
    .bind(agent_model)
    .fetch_one(pg)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[notifications] agent mute read failed: {e}");
            false
        }
    }
}

/// The person's mutes that still hold: (agent model, muted-until epoch ms),
/// by model.
pub async fn list_agent_dm_mutes(
    pg: &PgPool,
    user_id: &str,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as(
        "select agent_model, (trunc(extract(epoch from muted_until) * 1000))::bigint \
         from agent_dm_mutes where user_id = $1::uuid and muted_until > now() \
         order by agent_model",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await
}

/// Mute `agent_model`'s DMs until `until_ms`, or unmute with None. Answers
/// the mute that now holds (None after an unmute).
pub async fn set_agent_dm_mute(
    pg: &PgPool,
    user_id: &str,
    agent_model: &str,
    until_ms: Option<i64>,
) -> Result<Option<i64>, sqlx::Error> {
    // Expired rows answer "no mute" already; a write is where they go, so the
    // table holds only mutes that still mean something.
    sqlx::query("delete from agent_dm_mutes where user_id = $1::uuid and muted_until <= now()")
        .bind(user_id)
        .execute(pg)
        .await?;
    let Some(ms) = until_ms else {
        sqlx::query("delete from agent_dm_mutes where user_id = $1::uuid and agent_model = $2")
            .bind(user_id)
            .bind(agent_model)
            .execute(pg)
            .await?;
        return Ok(None);
    };
    sqlx::query_scalar(
        "insert into agent_dm_mutes (user_id, agent_model, muted_until) \
         values ($1::uuid, $2, to_timestamp($3::bigint / 1000.0)) \
         on conflict (user_id, agent_model) do update set muted_until = excluded.muted_until \
         returning (trunc(extract(epoch from muted_until) * 1000))::bigint",
    )
    .bind(user_id)
    .bind(agent_model)
    .bind(ms)
    .fetch_one(pg)
    .await
    .map(Some)
}
