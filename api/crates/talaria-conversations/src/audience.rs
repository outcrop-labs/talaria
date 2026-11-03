// A conversation's firehose audience: everyone `audience_sql!` admits, so a
// `conversation` event reaches exactly the people whose list, unread pill or
// working glyph it can move, and nobody the read gates would refuse.

use sqlx::PgPool;
use talaria_notify::NotifyDeps;
use talaria_realtime::{UserEvent, publish_user};

/// Who can read a conversation, as user ids. The read gates' predicate run
/// over every person rather than bound to one.
pub async fn conversation_audience_ids(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(concat!(
        "select u.id::text from conversations c cross join users u \
         where c.id = $1::uuid and ",
        audience_sql!("u.id")
    ))
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Ring the audience: a turn started, landed or changed in this conversation.
/// The unread pill it moves belongs to whoever was NOT looking: the author's
/// own client refetches and finds nothing new, a collaborator's finds their
/// teammate's turn, the owner's finds the agent's reply.
pub fn fan_conversation_event(deps: NotifyDeps, conversation_id: String) {
    tokio::spawn(async move {
        match conversation_audience_ids(&deps.pg, &conversation_id).await {
            Ok(ids) => publish_conversation(&deps, &conversation_id, &ids),
            Err(e) => {
                tracing::error!(
                    "[realtime] could not fan a conversation event for {conversation_id}: {e}"
                );
            }
        }
    });
}

/// Fan a conversation event to an audience already resolved. Hard delete uses
/// this: the usual fan reads the row, and that row is about to be gone.
pub fn fan_known_conversation(deps: NotifyDeps, conversation_id: String, user_ids: Vec<String>) {
    tokio::spawn(async move { publish_conversation(&deps, &conversation_id, &user_ids) });
}

fn publish_conversation(deps: &NotifyDeps, conversation_id: &str, user_ids: &[String]) {
    let event = UserEvent::Conversation {
        conversation_id: conversation_id.to_string(),
    };
    for id in user_ids {
        publish_user(&deps.realtime, id, &event);
    }
}
