// The plan's living document, server-side — port of ui/src/server/plan-doc.ts,
// REDUCED to the half the chat plane needs: the plan-mode harness prompt, the
// routing-awareness block, and the @mention fanout. The document itself (the
// find-or-create, the rewrite harness, the data-loss guard) belongs to the
// plans surface's own family and crosses with it — `notify_plan_mentions`
// re-enters this module for the doc's read boundary, so that half will land
// here too when the family crosses.

use crate::artifacts::{artifacts_for_target, guarded};
use crate::conversations::list_plan_members;
use crate::kb_perms::{can_read, list_editors};
use crate::mentions::{Mentionee, notify_mentions};
use crate::notify::NotifyDeps;
use crate::users::list_users;
use crate::workflows::routing_context;

/// The plan-mode harness, prepended to every plan-conversation turn
/// (plan-doc.ts PLAN_MODE_PROMPT). Without it the agent treats a planning
/// chat like any other request and starts CREATING things (tickets, docs) —
/// planning must stay side-effect free.
pub const PLAN_MODE_PROMPT: &str = "This is a PLANNING conversation on the Plan surface. Your job is to think and decide WITH the teammate: clarify the goal, surface options and risks, and converge on scope, steps, and owners. A living plan document sits beside this chat and is rewritten from the conversation after each of your turns, so put decisions and structure into your words here.
Planning is side-effect free. Do NOT create or modify anything: no tickets, no documents or artifacts, no knowledge-base entries or spaces, no emails, calendar events, or channel posts. Reading is encouraged (search knowledge, read docs, list boards and tickets) to ground the plan in what actually exists.
When the plan is settled, the teammate turns it into tickets with the \"Draft tickets\" control on this surface. If asked to create tickets or other work products here, point to that control instead of doing it yourself.";

/// Routing awareness for plan surfaces (planRoutingBlock): the org's
/// workflow map, framed as a FINAL aside — never something that reshapes
/// the plan itself. A read failure is no block at all.
pub async fn plan_routing_block(pg: &sqlx::PgPool) -> String {
    let Ok(ctx) = routing_context(pg).await else {
        return String::new();
    };
    if ctx.is_empty() {
        return String::new();
    }
    format!(
        "\n\nThe org routes ticket work through workflows (match rules → skills → agents):\n{ctx}\nWhen converging on owners, prefer routing work where a workflow already covers it — and say so in passing, not as the plan's centerpiece."
    )
}

/// Notify teammates a plan message @mentions (notifyPlanMentions) — only
/// members who can actually READ the plan's document (owner-private plans
/// mention silently until the doc is shared). Before the doc exists, the
/// plan's own membership is the read boundary. Fire-and-forget friendly.
pub async fn notify_plan_mentions(
    notify: &NotifyDeps,
    pg: &sqlx::PgPool,
    conversation_id: &str,
    sender_id: &str,
    sender_label: &str,
    content: &str,
    plan_title: Option<&str>,
) {
    if !content.contains('@') {
        return;
    }
    let eligible: Vec<Mentionee> = match plan_doc_for(pg, conversation_id).await {
        Ok(Some(doc)) => {
            let grants = list_editors(pg, "artifact", &doc.id)
                .await
                .unwrap_or_default();
            let item = guarded(&doc);
            list_users(pg)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|(id, email, name)| {
                    can_read(
                        &item,
                        Some(id),
                        email.as_deref().or(name.as_deref()),
                        &grants,
                    )
                })
                .map(|(user_id, name, email)| Mentionee {
                    user_id,
                    name,
                    email,
                })
                .collect()
        }
        _ => list_plan_members(pg, conversation_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|m| Mentionee {
                user_id: m.user_id,
                name: m.name,
                email: m.email,
            })
            .collect(),
    };
    notify_mentions(
        notify,
        &eligible,
        sender_id,
        sender_label,
        content,
        &format!("a plan ({})", plan_title.unwrap_or("Untitled")),
        &format!("/plan/{conversation_id}"),
    )
    .await;
}

/// The plan's linked doc artifact, if one exists yet (planDocFor).
pub async fn plan_doc_for(
    pg: &sqlx::PgPool,
    conversation_id: &str,
) -> Result<Option<crate::artifacts::Artifact>, sqlx::Error> {
    Ok(artifacts_for_target(pg, "plan", conversation_id)
        .await?
        .into_iter()
        .find(|a| a.kind == "doc"))
}
