// WHO IS RESPONSIBLE FOR WHAT AN AGENT MAKES, so a human owns the output.
//
// THE HOLE THIS FILLS. An org agent creating something — a doc, an artifact,
// a research run — used to file it under itself: created_by carried the model
// and owner_user_id stayed NULL, so the item was ownerless. Ownerless agent
// items are governed by whoever is allow-listed to use that agent plus admins
// (kb::perms::can_govern) — which is exactly the complaint: the person who
// asked for the work could not control what came out of it.
//
// THE LADDER, most-specific first, every rung best-effort:
//
//   the OWNER     a personal assistant's output belongs to its owner.
//   the CHATTER   an org agent mid-turn creates for the human it is answering
//                 (research_origin's agent-turn key names the conversation;
//                 conversations.user_id names the human). Same bounded
//                 misdelivery research already accepts: at worst the wrong
//                 rung, never a leak, and always somebody accountable in the
//                 same workspace.
//   the HIRER     nobody is chatting — a cron or a standalone tool call — so
//                 the admin who hired the agent stands behind its output
//                 (runs kind 'agent-hire'; subject_id is the agent SLUG, not
//                 the model, so the join goes through agent_defs).
//   nobody        an agent with no owner and no hire run stays ownerless, and
//                 the existing can_govern fallback governs it.
//
// PROVEN CALLERS ONLY. The whole ladder is gated on subject_proven: a legacy
// shared-key caller is identified but not proven to BE that agent, and may
// not claim its turns or its hires. (assistant_owner_for gates itself the
// same way; the gate here also covers the rungs below it.)
//
// STAMPING, NOT REACH. This answers ONE question — whose row is this? Read
// reach stays where it was: can_read_agent's owner arm is personal-assistant
// only, deliberately. Ownership is the right to govern, not a licence to
// read everything the owner reads.

use redis::aio::ConnectionManager;
use sqlx::PgPool;

use crate::agent_auth::{AgentSubject, subject_model, subject_proven};
use crate::conversations::conversation_owner;
use crate::research_origin::current_agent_turn_on;
use crate::users::assistant_owner_for;

/// The responsible user for anything this subject is about to create, or
/// None when nobody stands behind it. See the module header for the ladder
/// and its bounds. A missing rung is never an error — Err means the database
/// itself failed, which callers treat like any other failure.
pub async fn responsible_user_for(
    pg: &PgPool,
    redis: Option<ConnectionManager>,
    subject: &AgentSubject,
) -> Result<Option<String>, sqlx::Error> {
    if !subject_proven(subject) {
        return Ok(None);
    }
    if let Some(owner) = assistant_owner_for(pg, subject).await? {
        return Ok(Some(owner));
    }
    let model = subject_model(subject);
    // Best-effort turn read: Redis down or the key expired is an ordinary
    // None here, not a failure — the next rung answers.
    if let Some(conversation) = current_agent_turn_on(redis, model).await
        && let Some(user) = conversation_owner(pg, &conversation).await?
    {
        return Ok(Some(user));
    }
    // The hire run names the agent by SLUG and model is slug-department, so
    // the join bridges the two. Latest hire wins: a re-hire is the more
    // recent act of responsibility.
    let hirer: Option<(String,)> = sqlx::query_as(
        "select r.owner_user_id::text from runs r \
         join agent_defs d on d.slug = r.subject_id \
         where r.kind = 'agent-hire' and d.model = $1 \
           and r.owner_user_id is not null \
         order by r.created_at desc limit 1",
    )
    .bind(model)
    .fetch_optional(pg)
    .await?;
    Ok(hirer.map(|(v,)| v))
}
