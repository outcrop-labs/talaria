// The plan's living document, server-side — port of ui/src/server/plan-doc.ts.
// The document IS a `doc` artifact linked to the plan conversation
// (artifact_links target_type='plan') — no separate model. This module
// finds/creates that artifact, lets the plan's own agent rewrite it from the
// conversation, keeps it in the activity index, and notifies teammates the
// plan @mentions (only ones who can read the document).
//
// The rewrite prompt, the reply's contract and the data-loss guard live in
// harness/defs/plan_doc.rs and run through `run_harness`. Read that file's
// header before touching `sync_plan_doc`: the model is asked for the WHOLE
// document, so a truncated or gutted reply does not produce a worse document,
// it destroys a good one — the regression check below is the only thing
// standing between the two.

use crate::artifacts::{
    Artifact, SaveArtifactPatch, agent_category_folder, artifacts_for_target, attach_artifact,
    create_artifact, guarded, index_plan_doc, save_artifact,
};
use crate::conversations::{list_plan_members, prior_messages};
use crate::fleet::describe_agent;
use crate::harness::defs::plan_doc::{PlanDocInput, plan_doc_harness, plan_doc_regression};
use crate::harness::run::{HarnessError, RunContext, RunLedger, run_harness};
use crate::harness::transport::LedgerSource;
use crate::kb::perms::{EditorGrant, can_read, list_editors, set_editors};
use crate::mentions::{Mentionee, notify_mentions};
use crate::notify::NotifyDeps;
use crate::state::AppState;
use crate::templates::{ResolveContext, resolve_template, template_prompt};
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

/// Who a plan document belongs to and writes as — the plan's OWNER, whoever
/// touched the doc first (the route passes the acting user's label but the
/// conversation owner's id).
pub struct PlanOwner<'a> {
    pub id: &'a str,
    pub label: &'a str,
}

/// Find-or-create the plan's document (ensurePlanDoc), seeded from the plan's
/// template — the explicit per-plan pick if set, else the agent's bound plan
/// template; the skeleton is the starting structure. Owned by the plan's
/// owner. Collaborators already on the plan get editor grants on the doc the
/// moment it exists (later shares grant at share time).
pub async fn ensure_plan_doc(
    pg: &sqlx::PgPool,
    conversation_id: &str,
    owner: PlanOwner<'_>,
    plan_title: Option<&str>,
    agent_model: Option<&str>,
    template_id: Option<&str>,
) -> Result<Artifact, sqlx::Error> {
    if let Some(existing) = plan_doc_for(pg, conversation_id).await? {
        return Ok(existing);
    }
    let agent_model = agent_model.filter(|m| !m.is_empty());
    let template_id = template_id.filter(|t| !t.is_empty());
    let template = if agent_model.is_some() || template_id.is_some() {
        resolve_template(
            pg,
            "plan",
            &ResolveContext {
                explicit_id: template_id,
                agent_model,
                board_id: None,
            },
        )
        .await?
    } else {
        None
    };
    // Filed under the plan agent's cabinet, not dumped at the root.
    let folder_id = match agent_model {
        Some(m) => agent_category_folder(pg, &describe_agent(m).label, "Plans", owner.label).await,
        None => None,
    };
    let artifact = create_artifact(
        pg,
        Some("doc"),
        Some(&format!("Plan — {}", plan_title.unwrap_or("Untitled"))),
        owner.label,
        Some(owner.id),
        folder_id.as_deref(),
    )
    .await?;
    attach_artifact(pg, &artifact.id, "plan", conversation_id, owner.label).await?;
    let collaborators = list_plan_members(pg, conversation_id)
        .await?
        .into_iter()
        .filter(|m| m.role == "collaborator")
        .collect::<Vec<_>>();
    if !collaborators.is_empty() {
        let grants = collaborators
            .iter()
            .map(|m| EditorGrant {
                principal_type: "user".into(),
                principal_id: m.user_id.clone(),
                role: "editor".into(),
            })
            .collect::<Vec<_>>();
        set_editors(pg, "artifact", &artifact.id, &grants).await?;
    }
    if template.as_ref().is_some_and(|t| !t.body.trim().is_empty()) {
        let saved = save_artifact(
            pg,
            &artifact.id,
            SaveArtifactPatch {
                body: template.as_ref().map(|t| t.body.as_str()),
                ..Default::default()
            },
            owner.label,
        )
        .await?;
        return Ok(saved.unwrap_or(artifact));
    }
    Ok(artifact)
}

/// The alias NAME of a routed persona id, or None when no tier was picked —
/// the inverse of `routed_model_for`, the only thing that builds one
/// (`{agent}-{tier}`, or the agent unchanged). Both plan surfaces arrive
/// holding the PAIR, and `RunContext.tier` wants the two halves apart.
///
/// A function rather than a slice at each call site because getting it wrong
/// is invisible rather than loud: the ledger prices a row by finding
/// `agent_defs.model = agent_model` and then the alias named by `tier`, so a
/// run handed "engineer-engineering-opus" as its model with no tier misses
/// BOTH lookups — the row lands on an agent that does not exist, with no
/// endpoint class, which means no price. A plan drafted on a tier would
/// quietly be free.
pub fn plan_tier(agent_model: &str, routed_model: &str) -> Option<String> {
    if routed_model == agent_model {
        return None;
    }
    // `routed_model_for` only ever builds `{agent}-{tier}` or returns the
    // agent unchanged, so the prefix is guaranteed on every real input;
    // strip_prefix is the same slice as the TS without its panic on a
    // multi-byte boundary.
    routed_model
        .strip_prefix(agent_model)
        .and_then(|rest| rest.strip_prefix('-'))
        .map(str::to_string)
}

/// Rewrite the plan document from the conversation (syncPlanDoc), via the
/// plan's own agent — persona gateway, metered like any chat turn. Returns
/// the saved artifact.
///
/// THIS FUNCTION OVERWRITES A DOCUMENT A TEAM HAS BEEN BUILDING, and every
/// refusal below exists for that. It errors rather than returning the
/// unchanged artifact so the Plan surface can say what happened — the route
/// maps an error to a 502 with this message, and silently returning the old
/// document would show a "synced" document that never synced. The error
/// String is exactly the sentence that lands in that 502.
pub async fn sync_plan_doc(
    state: &AppState,
    conversation_id: &str,
    owner: PlanOwner<'_>,
    plan_title: Option<&str>,
    agent_model: &str,
    routed_model: &str,
    template_id: Option<&str>,
) -> Result<Artifact, String> {
    let pg = state.pg.clone();
    let doc = ensure_plan_doc(
        &pg,
        conversation_id,
        PlanOwner {
            id: owner.id,
            label: owner.label,
        },
        plan_title,
        Some(agent_model),
        template_id,
    )
    .await
    .map_err(|e| e.to_string())?;
    let label = describe_agent(agent_model).label;
    let sb = state.secretbox().await.unwrap_or_default();
    let msgs = prior_messages(&pg, &sb, conversation_id)
        .await
        .map_err(|e| e.to_string())?;
    let transcript = msgs
        .iter()
        .filter(|m| !m.content.is_empty())
        .map(|m| {
            format!(
                "{}: {}",
                if m.role == "assistant" {
                    label.as_str()
                } else {
                    "User"
                },
                m.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if transcript.trim().is_empty() {
        return Ok(doc);
    }

    let template = resolve_template(
        &pg,
        "plan",
        &ResolveContext {
            explicit_id: template_id,
            agent_model: Some(agent_model),
            board_id: None,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let current = doc.body.trim().to_string();
    let routing_map = routing_context(&pg).await.ok().filter(|m| !m.is_empty());
    let input = serde_json::to_value(PlanDocInput {
        current: current.clone(),
        transcript,
        template_prompt: template
            .as_ref()
            .map(|t| template_prompt(t, "the plan document")),
        routing_map,
    })
    .expect("the plan-doc input serializes");

    // The plan's OWN agent writes the document, so the model is pinned rather
    // than resolved from a chain. `tier` is named separately from the base
    // agent because the runner needs both: it calls `<agent>-<alias>` and
    // meters the spend against `<agent>`. Nothing here supplies a transport —
    // the runner routes a persona tier itself and carries this attribution on
    // the request (see `plan_tier` above).
    let tier = plan_tier(agent_model, routed_model);
    let ctx = RunContext {
        caller: format!("plan:{conversation_id}"),
        model: Some(agent_model.to_string()),
        tier: tier.clone(),
        ledger: Some(RunLedger {
            source: Some(LedgerSource::Chat),
            ref_id: Some(conversation_id.to_string()),
            task_id: None,
        }),
        ..Default::default()
    };
    let result = run_harness(state, &plan_doc_harness(), &input, ctx)
        .await
        .map_err(|HarnessError(e)| e)?;

    // The runner reports a failure in harness terms, which is the right
    // sentence for `harness_runs` and the wrong one for a toast on the Plan
    // surface. A reply that arrived and carried nothing keeps the wording
    // this route has always thrown; anything else means we never got an
    // answer, and the runner's sentence names why. `HarnessResult.answered`
    // is that fact under its own name.
    let Some(body) = result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return Err(if result.answered {
            "the agent returned an empty document".into()
        } else {
            result
                .error
                .unwrap_or_else(|| "the agent could not be reached".into())
        });
    };
    // THE DATA-LOSS GUARD (see harness/defs/plan_doc.rs). The reply is a whole
    // document and it is about to replace one, so a rewrite that lost most of
    // its sections, or dropped sections while coming back shorter, or kept the
    // headings and threw away the substance, is not saved at all. The document
    // the plan already has is always the better answer to "that reply was
    // damage".
    if let Some(regression) = plan_doc_regression(&current, &body) {
        return Err(format!(
            "the agent returned {regression}; the existing document was kept"
        ));
    }

    let saved = save_artifact(
        &pg,
        &doc.id,
        SaveArtifactPatch {
            body: Some(body.as_str()),
            ..Default::default()
        },
        &label,
    )
    .await
    .map_err(|e| e.to_string())?;
    let saved = saved.unwrap_or(doc);
    // `.catch(() => {})` — the brain is a consumer, not the answer.
    let _ = index_plan_doc(
        &pg,
        &crate::retrieval::qdrant::real_deps(),
        &crate::retrieval::embed::real_deps(),
        &saved,
        conversation_id,
    )
    .await;
    Ok(saved)
}
