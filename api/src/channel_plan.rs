// Plan chat: turn a channel conversation into ticket proposals. A chosen
// channel agent reads the transcript and drafts structured tickets; a human
// reviews, edits, and creates them (into inbox — planning never assigns).
//
// The prompt, the output schema and
// the coercion live in harness/defs/channel_plan.rs and run through
// run_harness; what stays HERE is gathering the transcript, the template and
// the workflow map, because those are database reads and a harness
// definition must stay pure.

use serde_json::json;

use crate::artifacts::plan_doc_for;
use crate::channels::list_channel_messages;
use crate::conversations::prior_messages;
use crate::fleet::describe_agent;
use crate::harness::defs::channel_plan::{
    ChannelPlanInput, TicketProposal, channel_plan_harness, to_proposals,
};
use crate::harness::run::{RunContext, RunLedger, run_harness};
use crate::harness::transport::LedgerSource;
use crate::state::AppState;
use crate::templates::{ResolveContext, resolve_template, template_prompt};
use crate::workflows::routing_context;

/// The alias NAME of a routed persona id, or None when no tier was picked —
/// the inverse of `routed_model_for`, which is the only thing that builds
/// one (`{agent}-{tier}`, or the agent unchanged). Both plan surfaces arrive
/// holding the PAIR, and `RunContext.tier` wants the two halves apart.
///
/// A function rather than a slice at each call site because getting it wrong
/// is invisible rather than loud: the ledger prices a row by finding
/// `agent_defs.model = agentModel` and then the alias named by `tier`, so a
/// run handed "engineer-engineering-opus" as its model with no tier misses
/// BOTH lookups — the row lands on an agent that does not exist, with no
/// endpoint class, which means no price. A plan drafted on a tier would
/// quietly be free.
pub fn plan_tier(agent_model: &str, routed_model: &str) -> Option<String> {
    if routed_model == agent_model {
        return None;
    }
    // Out-of-range is no-tier, not a panic —
    // `get` instead of a slicing index.
    routed_model
        .get(agent_model.len()..)
        .and_then(|rest| rest.strip_prefix('-'))
        .map(str::to_string)
}

/// A draft's result: the reviewable proposals and the model's reply. `raw`
/// exists for exactly one caller decision — telling "the agent did not
/// return parseable tickets" apart from "nothing to plan yet" (the run step
/// writes one of those two as the draft's note).
pub struct PlanOutcome {
    pub proposals: Vec<TicketProposal>,
    pub raw: String,
}

/// Template context for a draft: where the tickets will land + any explicit
/// pick. Resolution (explicit → agent → board default → none) happens in
/// `template_block`, because it is a database read and stays on this side of
/// the harness boundary.
#[derive(Default)]
pub struct DraftTemplateCtx<'a> {
    pub board_id: Option<&'a str>,
    pub template_id: Option<&'a str>,
}

/// The ticket template as the model is told about it, or nothing.
async fn template_block(
    pg: &sqlx::PgPool,
    agent_model: &str,
    tpl: &DraftTemplateCtx<'_>,
) -> Result<Option<String>, sqlx::Error> {
    let template = resolve_template(
        pg,
        "ticket",
        &ResolveContext {
            explicit_id: tpl.template_id,
            agent_model: Some(agent_model),
            board_id: tpl.board_id,
        },
    )
    .await?;
    Ok(template
        .as_ref()
        .map(|t| template_prompt(t, "ticket descriptions")))
}

/// Draft ticket proposals from a transcript. Shared by the channel Plan
/// button and the first-class Plan surface.
///
/// The early-out sits ahead of the harness on purpose: a conversation with
/// nothing in it has no tickets in it, and this is what stops the Plan button
/// spending a model call and a harness_runs row to discover that.
async fn plan_from_transcript(
    state: &AppState,
    transcript: &str,
    agent_model: &str,
    routed_model: &str,
    ref_id: &str,
    source: DraftSource,
    opts: PlanOpts<'_>,
) -> Result<PlanOutcome, String> {
    if transcript.trim().is_empty() && !opts.plan_doc.is_some_and(|d| !d.trim().is_empty()) {
        return Ok(PlanOutcome {
            proposals: Vec::new(),
            raw: String::new(),
        });
    }

    let input = ChannelPlanInput {
        transcript: transcript.to_string(),
        // Gated on the TRIMMED value, carried raw —
        // whitespace-only context is no context.
        plan_doc: opts
            .plan_doc
            .filter(|d| !d.trim().is_empty())
            .map(str::to_string),
        template_prompt: opts.template_prompt.filter(|t| !t.is_empty()),
        // Never fatal: an org with no workflows, or a failed read, just means
        // no routing labels. Same posture this call has always had.
        routing_map: match routing_context(&state.pg).await {
            Ok(map) if !map.is_empty() => Some(map),
            _ => None,
        },
    };

    // The CHOSEN agent drafts, so the model is pinned rather than resolved
    // from a chain — and the tier is named apart from the base agent because
    // the runner needs both halves: it calls `<agent>-<alias>` and meters the
    // spend against `<agent>` (see `plan_tier`).
    let tier = plan_tier(agent_model, routed_model).filter(|t| !t.is_empty());
    let result = run_harness(
        state,
        &channel_plan_harness(),
        &json!(input),
        RunContext {
            caller: format!("{}:{ref_id}", source.as_str()),
            model: Some(agent_model.to_string()),
            tier: tier.clone(),
            ledger: Some(RunLedger {
                source: Some(source.ledger()),
                ref_id: Some(ref_id.to_string()),
                task_id: None,
            }),
            ..RunContext::default()
        },
    )
    .await
    .map_err(|e| e.0)?;
    // A run that never REACHED a model is not "nothing to plan yet". The run
    // step picks between "the agent did not return parseable tickets" and
    // "nothing to plan yet", and a transport failure is neither — so a
    // restarting agent container answered "nothing to plan yet" on a channel
    // full of work. `answered` is that question with a name on it: a stream
    // that died after three tokens leaves a `raw` behind and would read as a
    // model that answered badly; a model that genuinely answered badly falls
    // through to the parseable-tickets note, which is what this check is for.
    if !result.answered
        && let Some(error) = &result.error
    {
        return Err(error.clone());
    }
    Ok(PlanOutcome {
        proposals: to_proposals(result.value.as_ref().unwrap_or(&serde_json::Value::Null)),
        raw: result.raw.unwrap_or_default(),
    })
}

/// The two surfaces a draft runs from — the channel Plan button ('channel')
/// and the Plan surface ('chat', the ledger's name for a plan conversation).
#[derive(Clone, Copy)]
enum DraftSource {
    Channel,
    Chat,
}

impl DraftSource {
    fn as_str(self) -> &'static str {
        match self {
            DraftSource::Channel => "channel",
            DraftSource::Chat => "chat",
        }
    }
    fn ledger(self) -> LedgerSource {
        match self {
            DraftSource::Channel => LedgerSource::Channel,
            DraftSource::Chat => LedgerSource::Chat,
        }
    }
}

/// The optional context a draft carries.
struct PlanOpts<'a> {
    /// The plan's living document — the authoritative source when present.
    plan_doc: Option<&'a str>,
    /// The resolved ticket template — descriptions must follow its skeleton.
    template_prompt: Option<String>,
}

pub async fn plan_from_channel(
    state: &AppState,
    channel_id: &str,
    agent_model: &str,
    routed_model: &str,
    tpl: &DraftTemplateCtx<'_>,
) -> Result<PlanOutcome, String> {
    let history = list_channel_messages(&state.pg, channel_id, -1, 80, false)
        .await
        .map_err(|e| e.to_string())?;
    let transcript = history
        .iter()
        .filter(|m| m.status == "complete" && !m.content.is_empty())
        .map(|m| {
            format!(
                "{}: {}",
                if m.author_type == "agent" {
                    describe_agent(&m.author).label
                } else {
                    m.author.clone()
                },
                m.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let block = template_block(&state.pg, agent_model, tpl)
        .await
        .map_err(|e| e.to_string())?;
    plan_from_transcript(
        state,
        &transcript,
        agent_model,
        routed_model,
        channel_id,
        DraftSource::Channel,
        PlanOpts {
            plan_doc: None,
            template_prompt: block,
        },
    )
    .await
}

/// Draft tickets from a plan conversation (the Plan surface). The plan's
/// living document, when it has content, is the primary source; chat is
/// context.
pub async fn plan_from_conversation(
    state: &AppState,
    conversation_id: &str,
    agent_model: &str,
    routed_model: &str,
    tpl: &DraftTemplateCtx<'_>,
) -> Result<PlanOutcome, String> {
    let label = describe_agent(agent_model).label;
    // A degraded secretbox is a transcript whose file tail quietly isn't
    // there — per-file catch, not a failure.
    let sb = state.secretbox().await.unwrap_or_default();
    let msgs = prior_messages(&state.pg, &sb, conversation_id)
        .await
        .map_err(|e| e.to_string())?;
    let transcript = msgs
        .iter()
        .filter(|m| !m.content.is_empty())
        .map(|m| {
            format!(
                "{}: {}",
                if m.role == "assistant" {
                    label.clone()
                } else {
                    "User".to_string()
                },
                m.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let doc = plan_doc_for(&state.pg, conversation_id)
        .await
        .map_err(|e| e.to_string())?;
    let block = template_block(&state.pg, agent_model, tpl)
        .await
        .map_err(|e| e.to_string())?;
    plan_from_transcript(
        state,
        &transcript,
        agent_model,
        routed_model,
        conversation_id,
        DraftSource::Chat,
        PlanOpts {
            plan_doc: doc
                .as_ref()
                .map(|d| d.body.as_str())
                .filter(|b| !b.is_empty()),
            template_prompt: block,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_tier_names_the_alias_and_nothing_when_unrouted() {
        assert_eq!(plan_tier("engineer", "engineer"), None);
        assert_eq!(
            plan_tier("engineer", "engineer-opus").as_deref(),
            Some("opus")
        );
        // The dash is required: a different agent is not a tier of this one,
        // and a slice-at-length would swallow the '-' — only an exact
        // prefix + '-' yields a name.
        assert_eq!(plan_tier("engineer", "engineroo"), None);
        // An id that is exactly agent + '-' slices to '' —
        // no tier. strip_prefix returns Some("") here, so the empty
        // filter at the call site is what keeps that answer.
        assert_eq!(plan_tier("engineer", "engineer-").as_deref(), Some(""));
    }
}
