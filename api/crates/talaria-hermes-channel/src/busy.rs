// WHETHER AN AGENT IS IN USE — the rollout restarts an agent, so it passes
// over one that is mid-turn or mid-conversation and comes back to it later.
//
// From the database, so every api instance answers the same: a reply being
// written (`messages.status = 'streaming'`, still inside its writer's
// lease), a channel reply being written, a live work session, a message in
// one of its conversations within the last few minutes (a person in a
// conversation with it), or a model call booked to it within the last few
// minutes (a cron or a delegated turn Talaria does not drive is still
// metered at the relay, so this is the catch-all).

use sqlx::PgPool;

/// A conversation with a message this recent is a person mid-conversation.
pub const RECENT_CHAT_MINUTES: i32 = 10;

/// A model call this recent is the agent working.
pub const RECENT_CALL_MINUTES: i32 = 5;

/// Why the agent (its persona id, `agent_defs.model`) is in use, or None.
pub async fn busy(pg: &PgPool, model: &str) -> Result<Option<String>, sqlx::Error> {
    let row: (bool, bool, bool, bool, bool) = sqlx::query_as(
        "select \
           exists (select 1 from messages m join conversations c on c.id = m.conversation_id \
                   where c.agent_model = $1 and m.status = 'streaming' \
                     and m.streamed_at > now() - interval '450 seconds'), \
           exists (select 1 from channel_messages cm \
                   where cm.author_type = 'agent' and cm.author = $1 and cm.status = 'streaming' \
                     and cm.created_at > now() - interval '15 minutes'), \
           exists (select 1 from runs r where r.kind = 'work-session' \
                   and r.state in ('queued', 'running', 'awaiting') \
                   and r.input->>'agentModel' = $1), \
           exists (select 1 from messages m join conversations c on c.id = m.conversation_id \
                   where c.agent_model = $1 \
                     and m.created_at > now() - make_interval(mins => $2)), \
           exists (select 1 from usage_events u where u.agent_model = $1 \
                   and u.created_at > now() - make_interval(mins => $3))",
    )
    .bind(model)
    .bind(RECENT_CHAT_MINUTES)
    .bind(RECENT_CALL_MINUTES)
    .fetch_one(pg)
    .await?;
    Ok(why_busy(row))
}

/// The sentence for what the probe found.
pub fn why_busy(found: (bool, bool, bool, bool, bool)) -> Option<String> {
    let (streaming, channel, work, chat, call) = found;
    if streaming {
        Some("it is writing a reply".into())
    } else if channel {
        Some("it is writing a channel reply".into())
    } else if work {
        Some("it has a work session in progress".into())
    } else if chat {
        Some(format!(
            "a conversation with it had a message in the last {RECENT_CHAT_MINUTES} minutes"
        ))
    } else if call {
        Some(format!(
            "it called a model in the last {RECENT_CALL_MINUTES} minutes"
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_busiest_reason_is_the_one_said() {
        assert_eq!(why_busy((false, false, false, false, false)), None);
        assert_eq!(
            why_busy((true, false, true, true, true)).as_deref(),
            Some("it is writing a reply")
        );
        assert!(
            why_busy((false, false, false, true, false))
                .unwrap()
                .contains("a conversation with it")
        );
        assert!(
            why_busy((false, false, false, false, true))
                .unwrap()
                .contains("called a model")
        );
    }
}
