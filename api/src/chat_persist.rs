// Server-side persistence of an assistant stream.
// Runs detached from the client response (fed
// by a teed branch), so an in-progress reply is saved even if the client
// disconnects/reloads. Throttled flushes during streaming; final on end.
// Also records the turn in the token ledger (real usage or a char estimate).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::boards::list_members;
use crate::body::utf16_len;
use crate::conversations::{
    active_streaming_assistant, content_js_length, insert_streaming_assistant,
    last_user_message_effort, last_user_message_held, next_seq, prior_messages, set_message_guard,
    touch_conversation, update_assistant,
};
use crate::fleet::{describe_agent, routed_model_for};
use crate::gateway::fleet_chat::{
    AgentStreamEvent, AgentStreamParser, ToolCall, chat_payload, merge_tool, proxy_chat,
};
use crate::gateway::guard::{
    Finding, GuardMode, Spread, guard_chat_reply, needs_redaction, redact_findings, redact_secrets,
};
use crate::gateway::usage::{TokenCounts, UsageInput, estimate_tokens, record_usage};
use crate::mentions::{Mentionee, notify_mentions};
use crate::model::efforts::efforts_for_model;
use crate::notify::{NotifyDeps, fan_conversation_event, notify_agent_reply};
use crate::plan_doc::{PLAN_MODE_PROMPT, notify_plan_mentions, plan_routing_block};
use crate::realtime::{BoardEvent, RealtimeDeps, publish_board};
use crate::retrieval::index::IndexDoc;
use crate::retrieval::sources::{CommentSrc, comment_doc, index_activity};
use crate::state::AppState;
use crate::ticket_chat::{TICKET_MODE_PROMPT, TicketMeta, ticket_context_block, ticket_head};
use crate::titler::maybe_retitle_conversation;
use crate::workspace_handles::{HANDLE_TURN_NOTE, mentions_handle};

/// Set for plan conversations: replies feed
/// the activity brain, owner-scoped.
#[derive(Debug, Clone)]
pub struct PlanMeta {
    pub owner_user_id: String,
    pub title: Option<String>,
}

/// The turn's identity, threaded from /api/chat through the chain.
#[derive(Debug, Clone)]
pub struct TurnMeta {
    pub agent_model: String,
    pub tier: Option<String>,
    pub plan: Option<PlanMeta>,
    /// Research conversations carry one of the two research prompts, chosen by
    /// the run's state at the turn — working while the run is, report-mode
    /// once it has finished. None for every other kind.
    pub research_prompt: Option<&'static str>,
    /// Ticket threads carry their room: the head the context block renders
    /// (re-read fresh in the chain, so a turn queued behind a reply speaks to
    /// the ticket as it stands NOW) and the ids the completion fans need.
    /// None for every other kind.
    pub ticket: Option<TicketMeta>,
}

// One continuation at a time per conversation (in-process guard — the check
// below re-reads the DB, this just closes the tiny double-start window).
static CONTINUING: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

/// Claude-style flow: messages sent while a reply
/// streamed queued into history — start the NEXT turn covering them. Called
/// when a reply finishes and when a queued message lands with nothing in
/// flight; chains until the conversation goes quiet. No-op unless the last
/// message is the user's.
pub async fn continue_conversation(state: &AppState, conversation_id: &str, meta: &TurnMeta) {
    {
        let mut set = CONTINUING.lock().unwrap();
        if set.contains(conversation_id) {
            return;
        }
        set.insert(conversation_id.to_string());
    }
    let result = continue_inner(state, conversation_id, meta).await;
    CONTINUING.lock().unwrap().remove(conversation_id);
    result
}

async fn continue_inner(state: &AppState, conversation_id: &str, meta: &TurnMeta) {
    if active_streaming_assistant(&state.pg, conversation_id)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    // A broken secretbox means upload bytes are unreadable; the file leg of
    // the history degrades to nothing per-row, but the box itself being
    // unbuildable is a turn that cannot start.
    let Ok(sb) = state.secretbox().await else {
        return;
    };
    let prior = match prior_messages(&state.pg, &sb, conversation_id).await {
        Ok(p) => p,
        Err(_) => return,
    };
    let Some(last) = prior.last() else {
        return;
    };
    if last.role != "user" {
        return;
    }
    // A HELD TICKET MESSAGE IS NOT AN UNANSWERED ONE. The relevance judge
    // already read it live and passed — the hold stamp is the record — so a
    // message that queued in behind a reply must not be answered by the
    // chain either. Without this check the gate's live verdict had a hole
    // exactly where queueing is most common. A read that fails answers
    // false-held, the direction every fail-open here points: toward
    // answering.
    if meta.ticket.is_some()
        && last_user_message_held(&state.pg, conversation_id)
            .await
            .unwrap_or(false)
    {
        return;
    }
    // Chained plan turns carry the same plan-mode harness as live ones — and
    // the same handle note, for the same reason /api/chat adds it: a relay
    // minted while a reply was still streaming arrives on a QUEUED message,
    // so the turn that finally reads it is this one. A chained research turn
    // carries the same state-chosen prompt the live one got.
    let mut messages: Vec<Value> = Vec::new();
    if meta.plan.is_some() {
        let block = plan_routing_block(&state.pg).await;
        messages.push(json!({ "role": "system", "content": format!("{PLAN_MODE_PROMPT}{block}") }));
    }
    if let Some(prompt) = meta.research_prompt {
        messages.push(json!({ "role": "system", "content": prompt }));
    }
    if let Some(ticket) = &meta.ticket {
        // The head re-read fresh, not the one the door carried in: the agent
        // itself may have triaged the ticket while the reply that queued this
        // message was still streaming, and this turn should speak to the
        // ticket as it stands now. A task that vanished mid-chain leaves the
        // mode prompt alone — the room is still a ticket's room.
        let block = match ticket_head(&state.pg, &ticket.task_id).await {
            Some(head) => ticket_context_block(&head),
            None => String::new(),
        };
        messages.push(json!({
            "role": "system",
            "content": format!("{TICKET_MODE_PROMPT}{block}"),
        }));
    }
    if mentions_handle(&last.content) {
        messages.push(json!({ "role": "system", "content": HANDLE_TURN_NOTE }));
    }
    messages.extend(
        prior
            .iter()
            .map(|m| json!({ "role": m.role, "content": m.content })),
    );
    // A tier the agent no longer declares degrades to the base model rather
    // than dying — a chained turn nobody is watching.
    let routed = match meta.tier.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => routed_model_for(&state.pg, &meta.agent_model, Some(t))
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| meta.agent_model.clone()),
        None => meta.agent_model.clone(),
    };
    let Ok(seq) = next_seq(&state.pg, conversation_id).await else {
        return;
    };
    let Ok(assistant_id) =
        insert_streaming_assistant(&state.pg, conversation_id, seq, &json!({})).await
    else {
        return;
    };
    // THE QUEUED MESSAGE'S OWN EFFORT, not the completed turn's: the message
    // this chain exists to cover picked its level when it was sent (stamped
    // on its row by /api/chat), and the model it will run on is only known
    // now. Re-validated rather than trusted — an agent re-pointed
    // mid-conversation leaves a stale pick on the row, and a chained turn
    // nobody is watching degrades to the default rather than dying on a 400.
    let stamped = last_user_message_effort(&state.pg, conversation_id)
        .await
        .ok()
        .flatten();
    let honored = match stamped {
        Some(e) if efforts_for_model(&state.pg, &routed).await.contains(&e) => Some(e),
        _ => None,
    };
    let prompt_chars: usize = messages
        .iter()
        .map(|m| content_js_length(&m["content"]))
        .sum();
    let upstream = proxy_chat(
        &chat_payload(&routed, &Value::Array(messages), honored.as_deref()),
        None,
    )
    .await;
    // The row is marked error and the chain STOPS
    // HERE, before any persist. Persisting the error body instead parses zero
    // events, flushes an EMPTY 'complete' row, and the tail re-chains — and
    // prior_messages reads through empty rows, so "the last message is the
    // user's" stays true forever: a provider answering 429 turned one queued
    // message into a turn every ~550ms, 2,000 rows in ten minutes. The error
    // row is the surfacing too — the UI renders it as the turn that failed.
    if !(200..300).contains(&upstream.status) {
        let _ = update_assistant(&state.pg, &assistant_id, "", "", &[], "error").await;
        return;
    }
    let usage_meta = PersistMeta {
        agent_model: meta.agent_model.clone(),
        prompt_chars,
        tier: meta.tier.clone(),
        plan: meta.plan.clone(),
        research_prompt: meta.research_prompt,
        ticket: meta.ticket.clone(),
    };
    // Detached — the chain's own tail continues it. The
    // plain-fn seam is load-bearing: this chain is continue → persist →
    // continue …, and routing the spawn through a non-generic helper is what
    // keeps either half's opaque future type from having to prove the other's
    // Send-ness (E0391) — continue_inner sees only the helper's signature.
    spawn_persist(
        state.clone(),
        upstream.body,
        assistant_id,
        conversation_id.to_string(),
        usage_meta,
    );
}

/// The detached persist kick, as its own function (see the call site for why).
fn spawn_persist(
    state: AppState,
    body: crate::gateway::fleet_chat::ByteStream,
    message_id: String,
    conversation_id: String,
    usage_meta: PersistMeta,
) {
    tokio::spawn(async move {
        persist_assistant_stream(
            state,
            body,
            message_id,
            conversation_id,
            Some(usage_meta),
            None,
        )
        .await;
    });
}

/// The usage-side facts the ledger needs about the turn.
#[derive(Debug, Clone)]
pub struct PersistMeta {
    pub agent_model: String,
    pub prompt_chars: usize,
    pub tier: Option<String>,
    pub plan: Option<PlanMeta>,
    /// Rides through to the continuation's TurnMeta — see TurnMeta.
    pub research_prompt: Option<&'static str>,
    /// Rides through to the continuation's TurnMeta — see TurnMeta.
    pub ticket: Option<TicketMeta>,
}

/// Drain an assistant stream into the reply row.
/// `forward`, when present, is the client's teed branch: every chunk rides to
/// the caller first (a send failure is the client hanging up — the drain
/// keeps going, exactly what a tee does to its other branch), while the
/// parser accumulates the persisted copy.
#[allow(clippy::too_many_arguments)] // the persist's inputs name the turn's own facts
pub async fn persist_assistant_stream(
    state: AppState,
    body: crate::gateway::fleet_chat::ByteStream,
    message_id: String,
    conversation_id: String,
    usage_meta: Option<PersistMeta>,
    forward: Option<mpsc::Sender<Result<Bytes, std::io::Error>>>,
) {
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tools: Vec<ToolCall> = Vec::new();
    let mut usage: Option<(i64, i64)> = None;
    // The agent gateway's failure frame, when one lands — the provider call
    // died inside the container and the 200-stream carries the reason. Without
    // this the turn reads as EMPTY COMPLETE, and the chain re-fires it forever
    // (prior_messages sees through empty rows, so the user's message reads as
    // forever unanswered).
    let mut frame_error: Option<String> = None;
    let mut last_flush: Option<Instant> = None;
    let mut parser = AgentStreamParser::new();
    let mut stream = body;

    macro_rules! flush {
        ($status:expr) => {
            let _ = update_assistant(
                &state.pg,
                &message_id,
                &content,
                &reasoning,
                &tools,
                $status,
            )
            .await;
        };
    }
    macro_rules! ledger {
        () => {
            if let Some(meta) = &usage_meta {
                let counts = TokenCounts {
                    prompt_tokens: usage
                        .map(|u| u.0)
                        .unwrap_or_else(|| estimate_tokens(meta.prompt_chars)),
                    completion_tokens: usage.map(|u| u.1).unwrap_or_else(|| {
                        estimate_tokens(utf16_len(&content) + utf16_len(&reasoning))
                    }),
                    ..TokenCounts::default()
                };
                let pg = state.pg.clone();
                let agent_model = meta.agent_model.clone();
                let tier = meta.tier.clone();
                let ref_id = conversation_id.clone();
                let estimated = usage.is_none();
                tokio::spawn(async move {
                    let _ = record_usage(
                        &pg,
                        &UsageInput {
                            agent_model: &agent_model,
                            source: "chat",
                            ref_id: Some(&ref_id),
                            task_id: None,
                            tier: tier.as_deref(),
                            counts,
                            estimated,
                        },
                    )
                    .await;
                });
            }
        };
    }

    let errored = loop {
        let Some(chunk) = stream.next().await else {
            break false;
        };
        let chunk = match chunk {
            Ok(c) => c,
            Err(_) => {
                if let Some(tx) = &forward {
                    let _ = tx
                        .send(Err(std::io::Error::other("upstream stream errored")))
                        .await;
                }
                break true;
            }
        };
        if let Some(tx) = &forward {
            // The client's branch ends on hang-up; ours drains on.
            let _ = tx.send(Ok(chunk.clone())).await;
        }
        for ev in parser.feed(&chunk) {
            match ev {
                AgentStreamEvent::Content { text } => content.push_str(&text),
                AgentStreamEvent::Reasoning { text } => reasoning.push_str(&text),
                AgentStreamEvent::Tool {
                    id,
                    name,
                    label,
                    status,
                } => {
                    tools = merge_tool(&tools, id.as_deref(), &name, &label, status.as_deref());
                }
                AgentStreamEvent::Usage {
                    prompt_tokens,
                    completion_tokens,
                } => {
                    usage = Some((prompt_tokens, completion_tokens));
                }
                AgentStreamEvent::Error { message } => {
                    frame_error = frame_error.or(Some(message));
                }
            }
            // lastFlush starts unset — the first event flushes immediately,
            // then every 400ms.
            if last_flush.is_none_or(|t| t.elapsed() > Duration::from_millis(400)) {
                last_flush = Some(Instant::now());
                flush!("streaming");
            }
        }
    };
    for ev in parser.finish() {
        match ev {
            AgentStreamEvent::Content { text } => content.push_str(&text),
            AgentStreamEvent::Reasoning { text } => reasoning.push_str(&text),
            AgentStreamEvent::Tool {
                id,
                name,
                label,
                status,
            } => {
                tools = merge_tool(&tools, id.as_deref(), &name, &label, status.as_deref());
            }
            AgentStreamEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                usage = Some((prompt_tokens, completion_tokens));
            }
            AgentStreamEvent::Error { message } => {
                frame_error = frame_error.or(Some(message));
            }
        }
    }

    // A failure frame ends the turn as an ERROR whose content IS the reason —
    // the surfaced error Jon asked for, visible in the chat instead of an
    // empty reply nobody can explain — and the tail chain does not fire:
    // retrying a provider that just refused is the loop this closes.
    if let Some(message) = frame_error {
        content = format!("(agent error: {message})");
        flush!("error");
        ledger!();
        return;
    }
    if errored {
        flush!("error");
        ledger!();
        return;
    }
    flush!("complete");
    let _ = touch_conversation(&state.pg, &conversation_id, None).await;
    ledger!();
    // First-exchange naming: the Titler upgrades the mechanical truncated
    // title once there's a real exchange to name.
    {
        let state = state.clone();
        let conversation_id = conversation_id.clone();
        tokio::spawn(async move {
            maybe_retitle_conversation(&state, &conversation_id).await;
        });
    }
    // Confab guard on the final reply (structural). The fleet stream gives
    // tool names, so zero-tool-claim + secret-leak apply here. annotate/strict
    // pin the findings onto the message row (the UI renders a caveat;
    // transcripts never see it); strict also redacts leaked secrets from the
    // SAVED copy so future turns can't re-feed them.
    //
    // AWAITED, AND AHEAD OF THE INDEX AND THE NOTIFICATION, because both take
    // a COPY of `content` — as detached tasks below them, strict mode scrubbed
    // the `messages` row while `indexActivity` had already put the unredacted
    // reply into the owner's brain, where nothing ever re-indexes it, and
    // where `search_knowledge` hands it back to a model — the one thing
    // guardrails' cardinal invariant forbids — and `notifyPlanMentions` had
    // already mailed it. Failure here must not fail the persist, so the whole
    // block is caught rather than flushed as errored.
    if !content.is_empty()
        && let Some(meta) = usage_meta.as_ref()
    {
        let guard = async {
            let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
            let (findings, mode) = guard_chat_reply(
                &state.pg,
                &content,
                &tool_names,
                "",
                &format!("chat:{}", meta.agent_model),
                &meta.agent_model,
                Spread::Contained,
            )
            .await;
            if findings.is_empty() || (mode != GuardMode::Annotate && mode != GuardMode::Strict) {
                return Ok(());
            }
            if mode == GuardMode::Strict && needs_redaction(&findings) {
                content = redact_secrets(&content, None).0;
                reasoning = redact_secrets(&reasoning, None).0;
                flush!("complete");
            }
            // Scrubbed: a pinned finding carries a verbatim excerpt of the
            // flagged span, and `zero_tool_claim` does not truncate its own.
            let scrubbed: Vec<Finding> = redact_findings(&findings);
            set_message_guard(&state.pg, &message_id, &scrubbed).await
        };
        let _ = guard.await;
    }
    // The rail's signal, on every completed reply: this thread's unread pill
    // moves for everyone who can read it, wherever in the app they are. The
    // row is landed complete above, so a client that refetches on the event
    // sees the finished turn; the event is id-shaped, so it carries nothing
    // the refetch doesn't re-read through the ordinary ACL. Detached — the
    // persist path never waits on a fan-out.
    fan_conversation_event(
        NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
        conversation_id.clone(),
    );
    // And the reply that landed while its readers were away rings once: one
    // agent-reply row per audience member whose read cursor doesn't cover it,
    // deduped while one sits unread per thread. AFTER the guard block on
    // purpose — the helper reads the reply as saved, so strict mode's scrub
    // has reached the row before any copy of it files into an inbox. Same
    // detached rule as the fan beside it.
    {
        let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
        let conversation_id = conversation_id.clone();
        let message_id = message_id.clone();
        tokio::spawn(async move {
            notify_agent_reply(&notify, &conversation_id, &message_id).await;
        });
    }
    if let Some(meta) = usage_meta
        .as_ref()
        .filter(|m| m.plan.is_some() && !content.trim().is_empty())
    {
        let plan = meta.plan.as_ref().expect("filtered above");
        // The reply's ambient copy, plan-owner-scoped.
        {
            let pg = state.pg.clone();
            let doc = IndexDoc {
                source_type: "plan".into(),
                source_id: message_id.clone(),
                title: Some(format!(
                    "Plan ({}) · {}",
                    plan.title.clone().unwrap_or_else(|| "Untitled".into()),
                    describe_agent(&meta.agent_model).label
                )),
                text: content.clone(),
                payload: Some(
                    vec![
                        ("planId".to_string(), json!(conversation_id)),
                        ("planOwnerId".to_string(), json!(plan.owner_user_id)),
                    ]
                    .into_iter()
                    .collect(),
                ),
                href: Some("/plan".into()),
            };
            tokio::spawn(async move {
                let qd = crate::retrieval::qdrant::real_deps();
                let ed = crate::retrieval::embed::real_deps();
                let _ = index_activity(&pg, &qd, &ed, &doc).await;
            });
        }
        // An agent turn @mentioning a collaborator notifies like a human one.
        {
            let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
            let conversation_id = conversation_id.clone();
            let label = describe_agent(&meta.agent_model).label;
            let content = content.clone();
            let title = plan.title.clone();
            let pg = state.pg.clone();
            tokio::spawn(async move {
                notify_plan_mentions(
                    &notify,
                    &pg,
                    &conversation_id,
                    "",
                    &label,
                    &content,
                    title.as_deref(),
                )
                .await;
            });
        }
    }
    // A ticket thread's completed reply is a COMMENT in every sense the rest
    // of the platform ever had: the board hears it (the badge counts this
    // turn the moment it lands, the same tag the comments door always sent),
    // the activity brain gains its copy (the exact comment_doc the backfill
    // and reindex write for human comments — agent replies were comments'
    // one missing voice, and the source id is the message id, so re-indexes
    // converge on the same points), and its @mentions notify like a human
    // turn's. All three read `content` as it stands here, AFTER the guard
    // block above — strict mode's scrub reaches the row before any copy of
    // it files into a brain or an inbox.
    if let Some(meta) = usage_meta
        .as_ref()
        .filter(|m| m.ticket.is_some() && !content.trim().is_empty())
    {
        let ticket = meta.ticket.as_ref().expect("filtered above");
        publish_board(
            &RealtimeDeps::publish_only(state.redis().await.ok()),
            &ticket.board_id,
            &BoardEvent {
                kind_tag: "comment",
                task_id: Some(ticket.task_id.clone()),
                deleted: None,
            },
        );
        {
            let pg = state.pg.clone();
            let doc = comment_doc(&CommentSrc {
                id: &message_id,
                task_id: &ticket.task_id,
                board_id: &ticket.board_id,
                ticket_ref: ticket.head.ticket_ref.as_deref(),
                author: &describe_agent(&meta.agent_model).label,
                content: &content,
            });
            tokio::spawn(async move {
                let qd = crate::retrieval::qdrant::real_deps();
                let ed = crate::retrieval::embed::real_deps();
                let _ = index_activity(&pg, &qd, &ed, &doc).await;
            });
        }
        {
            let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
            let pg = state.pg.clone();
            let board_id = ticket.board_id.clone();
            let task_id = ticket.task_id.clone();
            let sender_label = describe_agent(&meta.agent_model).label;
            let where_ = ticket
                .head
                .ticket_ref
                .clone()
                .unwrap_or_else(|| "a ticket".into());
            let content = content.clone();
            tokio::spawn(async move {
                let Ok(members) = list_members(&pg, &board_id).await else {
                    return;
                };
                let members: Vec<Mentionee> = members
                    .into_iter()
                    .map(|m| Mentionee {
                        user_id: m.user_id,
                        name: m.name,
                        email: m.email,
                    })
                    .collect();
                let href = format!("/boards/{board_id}/{task_id}");
                let _ = notify_mentions(
                    &notify,
                    &members,
                    // No user id to exclude — the sender is the agent.
                    "",
                    &sender_label,
                    &content,
                    &where_,
                    &href,
                )
                .await;
            });
        }
    }
    // Messages queued while this reply streamed become the next turn.
    if let Some(meta) = usage_meta {
        let state = state.clone();
        let conversation_id = conversation_id.clone();
        tokio::spawn(async move {
            continue_conversation(
                &state,
                &conversation_id,
                &TurnMeta {
                    agent_model: meta.agent_model,
                    tier: meta.tier,
                    plan: meta.plan,
                    research_prompt: meta.research_prompt,
                    ticket: meta.ticket,
                },
            )
            .await;
        });
    }
}
