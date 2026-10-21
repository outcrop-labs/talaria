// Server-side persistence of an assistant stream.
// Runs detached from the client response (fed
// by a teed branch), so an in-progress reply is saved even if the client
// disconnects/reloads. Throttled flushes during streaming; final on end.
// Also records the turn in the token ledger (real usage or a char estimate).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use sqlx::PgPool;
use talaria_agent_auth::now_ms;
use talaria_agent_defs::pinned_sessions_for_model;
use talaria_body::utf16_len;
use talaria_conversations::{
    active_streaming_assistant, content_js_length, insert_streaming_assistant,
    last_user_message_effort, mark_message_resumed, mark_message_stopped, merge_message_chips,
    message_still_errored, next_seq, pinned_history_fits, prior_messages,
    resurrect_streaming_assistant, set_message_guard, set_message_run_id, touch_conversation,
    unanswered_user_after, update_assistant,
};
use talaria_fleet_agents::routed_model_for;
use talaria_fleet_layout::describe_agent;
use talaria_gateway::fleet_chat::{
    AgentStreamEvent, AgentStreamParser, ChatStream, SteerOutcome, ToolCall, chat_payload,
    merge_tool, proxy_chat, proxy_session_turn, steer_run, stop_run,
};
use talaria_gateway::guard::{
    Finding, GuardMode, Spread, guard_chat_reply, needs_redaction, redact_findings, redact_secrets,
};
use talaria_gateway::usage::{TokenCounts, UsageInput, estimate_tokens, record_usage};
use talaria_model_efforts::efforts_for_model;
use talaria_notify::{NotifyDeps, fan_conversation_event, notify_agent_reply};
use talaria_plan_doc::{
    PLAN_MODE_PROMPT, PlanOwner, notify_plan_mentions, plan_doc_for, plan_routing_block,
    sync_plan_doc,
};
use talaria_retrieval_index::IndexDoc;
use talaria_retrieval_sources::index_activity;
use talaria_state::AppState;
use talaria_titler::maybe_retitle_conversation;
use talaria_workspace_handles::{HANDLE_TURN_NOTE, mentions_handle};

/// Set for plan conversations: replies feed
/// the activity brain, owner-scoped.
#[derive(Debug, Clone)]
pub struct PlanMeta {
    pub owner_user_id: String,
    pub title: Option<String>,
    /// The conversation's kind — `"plan"` or `"work"`. Both have a living
    /// document rewritten after a landed turn; the kind decides which prompt
    /// the rewrite uses and whether a plan template seeds it.
    pub surface: String,
}

// TALA-33: the server-side auto-sync of a plan's living document. The
// document rewrites when a turn LANDS, not only when a client that watched
// the stream happens to still be open to fire the manual POST — a reload, a
// poller cap, or a mid-queue landing left the document silently stale.
// Everything below runs DETACHED from the persist: the hot path's only
// extra work is passing along the plan facts it already holds.
/// One auto-sync per plan at a time, in-process. Overlapping triggers SKIP
/// rather than wait — the next landed turn re-fires the sync, so a dropped
/// overlap costs one turn of staleness, not a queue of double-burns. The
/// manual POST /plans/:id/doc route stays unchanged and races benignly with
/// this guard: the document is versioned and the data-loss guard
/// (harness/defs/plan_doc.rs) refuses a gutted rewrite, so the worst case of
/// a manual+auto overlap is one refused save, not damage.
static AUTO_SYNCING: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

/// The recency window: a document saved this recently is skipped, so the
/// manual sync a client fires on turn-complete (the same moment this path
/// fires) does not immediately double-burn the rewrite.
const AUTO_SYNC_RECENT_MS: i64 = 5_000;

/// Should an auto-sync start for this turn? Pure decision — kind check,
/// in-flight guard, recency — so the tests cover exactly the policy without
/// a live model behind `sync_plan_doc` (the harness runs through the persona
/// gateway; no test seam reaches it end-to-end from here).
fn auto_sync_should_start(
    plan: Option<&PlanMeta>,
    in_flight: bool,
    doc_updated_ms: Option<i64>,
    now_ms: i64,
) -> bool {
    plan.is_some()
        && !in_flight
        && doc_updated_ms.is_none_or(|ms| now_ms - ms > AUTO_SYNC_RECENT_MS)
}

/// The detached auto-sync kick, for a turn that just completed. Called on
/// the completed path only — NEVER on a frame_error (the agent refused;
/// nothing landed worth folding in) and never on a death (the row is an
/// error, not a turn). All database reads live here, inside the task, so the
/// hot path stays as it was.
fn spawn_plan_doc_auto_sync(
    state: &AppState,
    conversation_id: &str,
    plan: PlanMeta,
    agent_model: String,
    tier: Option<String>,
) {
    let state = state.clone();
    let conversation_id = conversation_id.to_string();
    tokio::spawn(async move {
        // The guard claim is split around the one await: a std MutexGuard
        // is not Send, so the doc read runs BETWEEN a check and the claim,
        // and the claim re-checks before inserting — the overlap window is
        // one fetch, exactly the window the manual route already races.
        let in_flight = AUTO_SYNCING
            .lock()
            .expect("auto-sync guard uncontended")
            .contains(&conversation_id);
        let doc = plan_doc_for(&state.pg, &conversation_id)
            .await
            .ok()
            .flatten();
        // `updated_at` is an ISO string; an unparseable one reads as "not
        // freshly saved" and lets the sync through (None = no document yet).
        let doc_updated_ms = doc
            .as_ref()
            .and_then(|d| talaria_agent_auth::iso_to_epoch_ms(&d.updated_at));
        if !auto_sync_should_start(Some(&plan), in_flight, doc_updated_ms, now_ms()) {
            return;
        }
        if AUTO_SYNCING
            .lock()
            .expect("auto-sync guard uncontended")
            .contains(&conversation_id)
        {
            return;
        }
        AUTO_SYNCING
            .lock()
            .expect("auto-sync guard uncontended")
            .insert(conversation_id.clone());
        // The label that lands in the artifact's version history is the AGENT
        // persona's, not a human's: the rewrite is the plan's own agent doing
        // the work (same attribution `sync_plan_doc` itself gives the saved
        // revision), and the plan owner is only the identity the document
        // belongs to. A human label here would read in the artifact history
        // as the owner having written the agent's rewrite.
        // TIERED PLANS METER LIKE THE MANUAL ROUTE: `routed_model_for` builds
        // `<base>-<tier>` the same way /api/chat does, so an auto-sync on a
        // tiered plan attributes and prices exactly as a manual sync would,
        // instead of silently pinning the base model. A resolution failure
        // falls back to the base agent rather than skipping the sync.
        let routed_model = routed_model_for(&state.pg, &agent_model, tier.as_deref())
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| agent_model.clone());
        let agent_label = describe_agent(&agent_model).label;
        let base_model = agent_model.clone();
        let result = sync_plan_doc(
            &state,
            &conversation_id,
            PlanOwner {
                id: &plan.owner_user_id,
                label: &agent_label,
            },
            plan.title.as_deref(),
            &base_model,
            &routed_model,
            None,
            &plan.surface,
        )
        .await;
        AUTO_SYNCING
            .lock()
            .expect("auto-sync guard uncontended")
            .remove(&conversation_id);
        if let Err(e) = result {
            // Auto-sync failure must never read as a failed turn — the
            // reply is landed and the conversation is fine; the document is
            // merely one turn behind until the next one lands.
            tracing::warn!("[plan-doc] auto-sync after turn failed: {e}");
        }
    });
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
    /// Who spoke the queued turn this chain answers, threaded from /api/chat.
    /// None on turns that predate the stamp or arrive without one — the
    /// chained turn then composes without it, as before.
    pub sender: Option<talaria_sender_identity::SenderIdentity>,
}

// One continuation at a time per conversation (in-process guard — the check
// below re-reads the DB, this just closes the tiny double-start window).
static CONTINUING: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

/// The silence ceiling on an agent turn's stream, `TALARIA_AGENT_IDLE_SECS`
/// floored at a minute, read per turn so a config change needs no restart.
/// Frames flowing means the agent is working — tool progress, deltas — and a
/// turn may stream for however long the work takes; only this much SILENCE
/// ends it. This replaces the total 600s request timeout that killed every
/// turn longer than ten minutes mid-flight, API hanging up on agents that
/// were still working (the "interrupted" knowledgebase turns). The sweep in
/// conversations::active_streaming_assistant keys off the same liveness and
/// waits past this ceiling for the writer's own end-state.
fn stream_idle() -> Duration {
    Duration::from_secs(idle_secs(std::env::var("TALARIA_AGENT_IDLE_SECS").ok()))
}

/// Pure half of `stream_idle` (tested below): unset or unparsable falls back
/// to ten minutes, and nothing can pull the ceiling under a minute — below
/// that the idle read would race the container's own turn-taking.
fn idle_secs(raw: Option<String>) -> u64 {
    raw.and_then(|v| v.parse().ok()).unwrap_or(600).max(60)
}

/// How long a dead stream waits before the auto-resume re-drives the turn.
/// Long enough that a blipped container comes back; short enough that the
/// person watching sees the working indicator return inside a minute.
const RESUME_BACKOFF: Duration = Duration::from_secs(15);

/// How a turn's stream ended badly, when it did. Both members are the stream
/// DYING — as opposed to the agent refusing (the failure frame), which
/// carries a reason and must not be retried.
#[derive(Debug, Clone, Copy)]
enum TurnDeath {
    Idle,
    Reset,
}

impl TurnDeath {
    /// The honest sentence for the row — written as the turn's content so a
    /// resume that never comes (or a crash during the backoff) leaves an
    /// explanation, not a bare error status the UI renders as "interrupted".
    fn reason(&self) -> String {
        match self {
            TurnDeath::Idle => {
                format!(
                    "the agent's stream went silent for {}s mid-turn",
                    stream_idle().as_secs()
                )
            }
            TurnDeath::Reset => "the agent's stream connection died mid-turn".to_string(),
        }
    }
}

/// Claude-style flow: messages sent while a reply
/// streamed queued into history — start the NEXT turn covering them. Called
/// when a reply finishes and when a queued message lands with nothing in
/// flight; chains until the conversation goes quiet. No-op unless the last
/// message is the user's.
/// Whether THIS turn goes through the agent's pinned Hermes session.
///
/// Two things have to hold: the agent is pinned (its def says so), and the
/// conversation's history is one Hermes can answer from — fresh, or driven
/// through the session before. A conversation that ran on the full-history
/// path before the flip stays there: Hermes never saw that transcript, and a
/// turn that sent only the new message would answer from an empty room.
pub async fn pinned_turn(
    pg: &PgPool,
    agent_model: &str,
    conversation_id: &str,
    resume_row: Option<&str>,
) -> bool {
    match pinned_sessions_for_model(pg, agent_model).await {
        Ok(true) => pinned_history_fits(pg, conversation_id, resume_row)
            .await
            .unwrap_or(false),
        _ => false,
    }
}

/// The pinned turn, cut out of the full-history message list the chat path
/// already builds: the system rows fold into one ephemeral system message
/// (Hermes takes it per turn, so the mode prompt, the sender block and the
/// handle note ride exactly as they did), the trailing run of user rows is
/// the message, and everything between — the history — is Hermes' to hold.
///
/// The trailing RUN, not the last row: two messages queued while a reply
/// was streaming are both unanswered, and the chained turn covers both.
/// A string run joins with blank lines; a run whose last row is content
/// parts (an image turn) keeps the parts and carries the earlier text as a
/// leading text part.
pub fn session_turn_parts(messages: &[Value]) -> (Option<String>, Value) {
    let system: Vec<&str> = messages
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .filter(|c| !c.trim().is_empty())
        .collect();
    let system_message = (!system.is_empty()).then(|| system.join("\n\n"));
    let tail: Vec<&Value> = messages
        .iter()
        .rev()
        .take_while(|m| m["role"] == "user")
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let message = match tail.split_last() {
        None => Value::String(String::new()),
        Some((last, earlier)) => {
            let earlier_text: Vec<&str> = earlier
                .iter()
                .filter_map(|m| m["content"].as_str())
                .filter(|c| !c.trim().is_empty())
                .collect();
            match &last["content"] {
                Value::Array(parts) => {
                    let mut out: Vec<Value> = Vec::new();
                    if !earlier_text.is_empty() {
                        out.push(json!({ "type": "text", "text": earlier_text.join("\n\n") }));
                    }
                    out.extend(parts.iter().cloned());
                    Value::Array(out)
                }
                other => {
                    let mut texts = earlier_text;
                    let last_text = other.as_str().unwrap_or_default();
                    if !last_text.trim().is_empty() {
                        texts.push(last_text);
                    }
                    Value::String(texts.join("\n\n"))
                }
            }
        }
    };
    (system_message, message)
}

/// Send the turn the way the agent takes it: through its pinned session, or
/// as the full-history completion every agent speaks. Returns the stream and
/// how many characters of prompt actually left — the ledger's estimate when
/// no usage frame comes back.
pub async fn send_turn(
    pinned: bool,
    agent_model: &str,
    routed_model: &str,
    conversation_id: &str,
    messages: &[Value],
    effort: Option<&str>,
) -> (ChatStream, usize) {
    if pinned {
        let (system_message, message) = session_turn_parts(messages);
        let prompt_chars = system_message
            .as_deref()
            .map(|s| s.encode_utf16().count())
            .unwrap_or(0)
            + content_js_length(&message);
        let upstream = proxy_session_turn(
            agent_model,
            conversation_id,
            system_message.as_deref(),
            &message,
            None,
        )
        .await;
        return (upstream, prompt_chars);
    }
    let prompt_chars: usize = messages
        .iter()
        .map(|m| content_js_length(&m["content"]))
        .sum();
    let upstream = proxy_chat(
        &chat_payload(routed_model, &Value::Array(messages.to_vec()), effort),
        None,
    )
    .await;
    (upstream, prompt_chars)
}

pub async fn continue_conversation(state: &AppState, conversation_id: &str, meta: &TurnMeta) {
    guarded_drive(state, conversation_id, meta, None).await;
}

/// The auto-resume drive, through the same guard: a resumed turn and a queued
/// turn can never both drive one conversation, whichever wins the race.
pub async fn resume_conversation(
    state: &AppState,
    conversation_id: &str,
    meta: &TurnMeta,
    row_id: &str,
) {
    guarded_drive(state, conversation_id, meta, Some(row_id)).await;
}

async fn guarded_drive(
    state: &AppState,
    conversation_id: &str,
    meta: &TurnMeta,
    resume: Option<&str>,
) {
    {
        let mut set = CONTINUING.lock().unwrap();
        if set.contains(conversation_id) {
            return;
        }
        set.insert(conversation_id.to_string());
    }
    continue_inner(state, conversation_id, meta, resume).await;
    CONTINUING.lock().unwrap().remove(conversation_id);
}

async fn continue_inner(
    state: &AppState,
    conversation_id: &str,
    meta: &TurnMeta,
    resume: Option<&str>,
) {
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
    let prior = match prior_messages(&state.pg, &sb, conversation_id, resume).await {
        Ok(p) => p,
        Err(_) => return,
    };
    let Some(last) = prior.last() else {
        return;
    };
    if last.role != "user" {
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
    if mentions_handle(&last.content) {
        messages.push(json!({ "role": "system", "content": HANDLE_TURN_NOTE }));
    }
    // TALA-80: the queued message may name a relay or answer a person from
    // memory — the chained turn must still know who actually spoke, from
    // the authenticated row that stamped the queue, not from what the
    // message itself claims.
    if let Some(sender) = &meta.sender {
        messages.push(talaria_sender_identity::sender_identity_message(sender));
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
    // A resume keeps the dead turn's row (its place in the thread, its
    // metadata stamp); a chain makes a fresh one. The active-streaming check
    // above already ran — a resume that lost the race to a newer turn stops
    // here silently, the error row it was going to revive left as the honest
    // record.
    // Decided before the row below exists: a fresh conversation's first
    // assistant row is the one being made here, and it must not count.
    let pinned = pinned_turn(&state.pg, &meta.agent_model, conversation_id, resume).await;
    let assistant_id = match resume {
        Some(row) => match resurrect_streaming_assistant(&state.pg, row).await {
            Ok(()) => row.to_string(),
            Err(_) => return,
        },
        None => {
            let Ok(seq) = next_seq(&state.pg, conversation_id).await else {
                return;
            };
            let Ok(id) =
                insert_streaming_assistant(&state.pg, conversation_id, seq, &json!({})).await
            else {
                return;
            };
            id
        }
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
    let (upstream, prompt_chars) = send_turn(
        pinned,
        &meta.agent_model,
        &routed,
        conversation_id,
        &messages,
        honored.as_deref(),
    )
    .await;
    // The row is marked error and the chain STOPS
    // HERE, before any persist. Persisting the error body instead parses zero
    // events, flushes an EMPTY 'complete' row, and the tail re-chains — and
    // prior_messages reads through empty rows, so "the last message is the
    // user's" stays true forever: a provider answering 429 turned one queued
    // message into a turn every ~550ms, 2,000 rows in ten minutes. The error
    // row is the surfacing too — the UI renders it as the turn that failed.
    // A RESUME that cannot even start keeps its explanation: the container
    // that blipped is still down, and an empty row here would render as the
    // routability hint — a diagnosis nothing in this path supports.
    if !(200..300).contains(&upstream.status) {
        let content = match resume {
            Some(_) => format!(
                "(agent error: the resumed turn could not start (HTTP {}))",
                upstream.status
            ),
            None => String::new(),
        };
        let _ = update_assistant(&state.pg, &assistant_id, &content, "", &[], "error").await;
        return;
    }
    let usage_meta = PersistMeta {
        agent_model: meta.agent_model.clone(),
        prompt_chars,
        tier: meta.tier.clone(),
        plan: meta.plan.clone(),
        research_prompt: meta.research_prompt,
        sender: meta.sender.clone(),
        resumed: resume.is_some(),
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
    body: talaria_gateway::fleet_chat::ByteStream,
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
    /// Rides through to the continuation's TurnMeta — an auto-resumed turn
    /// answers the same person, so the stamp re-rides. See TurnMeta.
    pub sender: Option<talaria_sender_identity::SenderIdentity>,
    /// Whether this drive is an auto-resume of an earlier dead attempt on the
    /// same row. The death path reads it: a resumed turn that dies again
    /// STAYS dead — one quiet retry, then the error is a person's to look at.
    pub resumed: bool,
}

// ── The doom-loop guard ─────────────────────────────────────────────────────
//
// An agent that calls the same thing with the same arguments over and over is
// not working, it is stuck — and nothing downstream notices. The turn looks
// busy, every frame resets the drain's idle ceiling, and the person watching
// gets a spinner that never ends. So the stream is counted as it goes past, and
// a pair that keeps coming back earns a word first and the plug second.

/// Identical calls that earn the agent a word, and the count that ends the run.
const LOOP_WARN_AT: u32 = 3;
const LOOP_STOP_AT: u32 = 5;

/// What a repeat count has earned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopAction {
    /// Say so: a chip for the person reading, and a steer into the live run.
    Warn,
    /// Past talking — interrupt the run, and say in the reply why it ends.
    Stop,
}

/// The guard's whole decision, pure. Each threshold fires on the EXACT count
/// that reaches it, so a pair stuck at twelve repeats still produces one nudge
/// and one stop rather than a nudge on every frame after the third.
pub fn loop_verdict(count: u32) -> Option<LoopAction> {
    match count {
        LOOP_WARN_AT => Some(LoopAction::Warn),
        LOOP_STOP_AT => Some(LoopAction::Stop),
        _ => None,
    }
}

/// A short stable key for a label, for the notice chip's id: the chip is
/// per (tool, arguments), and the arguments are prose too long and too odd
/// for an id.
fn label_key(label: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    label.trim().hash(&mut h);
    h.finish()
}

/// Count one call against this turn's tally and hand back what it earned.
///
/// The pair is (tool name, label), and the label is the frame's rendering of
/// the call's own arguments — which is what makes two identical pairs the SAME
/// call twice rather than the same tool used twice. A call whose frame carried
/// no label is not counted at all: every unlabelled call of one tool would
/// collapse into a single pair, and a tool legitimately used five times in a
/// turn would read as a loop.
fn count_repeat(
    tally: &mut HashMap<(String, String), u32>,
    name: &str,
    label: &str,
) -> Option<LoopAction> {
    if label.trim().is_empty() {
        return None;
    }
    let n = tally
        .entry((name.to_string(), label.to_string()))
        .or_insert(0);
    *n += 1;
    loop_verdict(*n)
}

/// Drain an assistant stream into the reply row.
/// `forward`, when present, is the client's teed branch: every chunk rides to
/// the caller first (a send failure is the client hanging up — the drain
/// keeps going, exactly what a tee does to its other branch), while the
/// parser accumulates the persisted copy.
#[allow(clippy::too_many_arguments)] // the persist's inputs name the turn's own facts
pub async fn persist_assistant_stream(
    state: AppState,
    body: talaria_gateway::fleet_chat::ByteStream,
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
    // The run was interrupted — a person on /stop, or the guard below. Not an
    // error and not a death: it decides how the turn LANDS, nothing more.
    let mut stopped = false;
    // The live run's id, held as well as stamped: a steer or a stop fired from
    // inside this loop has to name it, and the row is not where to go looking
    // for something the loop just read.
    let mut hermes_run: Option<String> = None;
    // Identical (name, label) calls seen in this turn — the doom-loop tally —
    // and the call ids already counted, so a call is one count however many
    // progress and completion frames it goes on to send.
    let mut repeats: HashMap<(String, String), u32> = HashMap::new();
    let mut seen_calls: HashSet<String> = HashSet::new();
    // Set by the guard's stop task once the container ACCEPTED the stop — a
    // run still going is a run that will write more, so the sentence that
    // says why the turn ended is written at landing, under this, after the
    // last delta rather than in the middle of them.
    let loop_stop_ok = Arc::new(AtomicBool::new(false));
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

    // THE DRAIN HAS NO TOTAL TIMEOUT. Frames flowing — deltas, tool progress,
    // keep-alives — mean the agent is working, however long the work takes;
    // only `stream_idle()` of SILENCE ends the read. The total 600s request
    // timeout this replaced killed every turn longer than ten minutes
    // mid-flight: the API hung up on a container that was still working (the
    // "· interrupted" knowledgebase turns) and dropped its partial reply.
    let death: Option<TurnDeath> = loop {
        match tokio::time::timeout(stream_idle(), stream.next()).await {
            // Silence past the ceiling: the agent's gateway stopped talking.
            Err(_) => {
                if let Some(tx) = &forward {
                    let _ = tx
                        .send(Err(std::io::Error::other("upstream stream went silent")))
                        .await;
                }
                break Some(TurnDeath::Idle);
            }
            // Clean end-of-stream.
            Ok(None) => break None,
            // The connection reset mid-stream.
            Ok(Some(Err(_))) => {
                if let Some(tx) = &forward {
                    let _ = tx
                        .send(Err(std::io::Error::other("upstream stream errored")))
                        .await;
                }
                break Some(TurnDeath::Reset);
            }
            Ok(Some(Ok(chunk))) => {
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
                            tools =
                                merge_tool(&tools, id.as_deref(), &name, &label, status.as_deref());
                            // THE DOOM-LOOP GUARD, counted once per CALL. The
                            // parser gives every call its own id and sends
                            // that id again on each progress and completion
                            // frame, so the first sight of an id is the call
                            // starting — counting every frame would trip the
                            // guard on a single chatty call. A frame with no
                            // id (the completions arm's older shape) is never
                            // counted. Only this loop counts — the finish loop
                            // below runs after the body has ended, where there
                            // is no run left to steer or stop.
                            let first_sight = id
                                .as_deref()
                                .is_some_and(|i| seen_calls.insert(i.to_string()));
                            if first_sight
                                && status.as_deref() == Some("running")
                                && let Some(action) = count_repeat(&mut repeats, &name, &label)
                            {
                                let model = usage_meta.as_ref().map(|m| m.agent_model.clone());
                                match action {
                                    LoopAction::Warn => {
                                        // The chip lands whether or not the
                                        // agent can be reached: it is the
                                        // person's explanation for a turn going
                                        // nowhere, and the full-history arm has
                                        // no run id to steer at all. Keyed by
                                        // the call's arguments too, so a tool
                                        // that loops on one input and later on
                                        // another earns a notice each time.
                                        let _ = merge_message_chips(
                                            &state.pg,
                                            &message_id,
                                            &[json!({
                                                "id": format!("loop:{name}:{:x}", label_key(&label)),
                                                "kind": "notice",
                                                "label": "Repeating the same call",
                                                "detail": format!(
                                                    "{name} has been called with the same \
                                                     arguments {LOOP_WARN_AT} times"
                                                ),
                                            })],
                                        )
                                        .await;
                                        // Detached: a steer is answered in
                                        // milliseconds or not at all, and the
                                        // drain must keep reading the stream
                                        // while it is.
                                        if let (Some(model), Some(run)) =
                                            (model.clone(), hermes_run.clone())
                                        {
                                            let said = format!(
                                                "You have called {name} with the same arguments \
                                                 {LOOP_WARN_AT} times and it is not getting you \
                                                 further. Change your approach, or stop and say \
                                                 what is blocking you."
                                            );
                                            let row = message_id.clone();
                                            let tool = name.clone();
                                            tokio::spawn(async move {
                                                match steer_run(&model, &run, &said).await {
                                                    SteerOutcome::Accepted => tracing::info!(
                                                        "[chat_persist] {row}: steered a looping {tool}"
                                                    ),
                                                    other => tracing::warn!(
                                                        "[chat_persist] {row}: the loop steer did \
                                                         not land ({other:?})"
                                                    ),
                                                }
                                            });
                                        }
                                    }
                                    LoopAction::Stop => {
                                        // Detached for the same reason, and the
                                        // flag is what the landing reads: the
                                        // sentence saying why the turn ended
                                        // rides only on an ACCEPTED stop, and
                                        // only after the last delta.
                                        if let (Some(model), Some(run)) =
                                            (model, hermes_run.clone())
                                        {
                                            let ok = loop_stop_ok.clone();
                                            let row = message_id.clone();
                                            let tool = name.clone();
                                            tokio::spawn(async move {
                                                match stop_run(&model, &run).await {
                                                    Ok(()) => ok.store(true, Ordering::Relaxed),
                                                    Err(e) => tracing::warn!(
                                                        "[chat_persist] {row}: stopping a looping \
                                                         {tool} failed: {e}"
                                                    ),
                                                }
                                            });
                                        }
                                    }
                                }
                            }
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
                        // Stamped the moment the session names its run: a
                        // stop or a steer that arrives mid-turn needs it,
                        // and mid-turn is the only time either makes sense.
                        AgentStreamEvent::RunStarted { run_id } => {
                            let _ = set_message_run_id(&state.pg, &message_id, &run_id).await;
                            hermes_run = Some(run_id);
                        }
                        // Interrupted. The flag is read once, after the loops.
                        AgentStreamEvent::Stopped => stopped = true,
                    }
                }
                // Stamped per CHUNK, not per parsed event: a keep-alive-only
                // stretch still proves the writer is alive, and streamed_at —
                // not created_at — is what the stale sweep and the working
                // flags read. last_flush starts unset, so the first chunk
                // flushes immediately, then every 400ms.
                if last_flush.is_none_or(|t| t.elapsed() > Duration::from_millis(400)) {
                    last_flush = Some(Instant::now());
                    flush!("streaming");
                }
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
            // The run id is still stamped from here — a run that only named
            // itself in the parser's leftovers still happened — but it is not
            // held: the body has ended, and there is nothing left to steer.
            AgentStreamEvent::RunStarted { run_id } => {
                let _ = set_message_run_id(&state.pg, &message_id, &run_id).await;
            }
            AgentStreamEvent::Stopped => stopped = true,
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
    // A stream that DIED (as opposed to the agent refusing above) ends the
    // attempt as an error whose content names how it died — never a bare
    // error status the UI renders as "interrupted" with nothing else — and
    // then, once, quietly tries to bring the turn back. The container
    // outliving the API's read on it is exactly the failure the resume
    // covers; a resume that dies again stays down (`resumed` gates it).
    if let Some(died) = death {
        content = format!("(agent error: {})", died.reason());
        flush!("error");
        ledger!();
        if let Some(meta) = usage_meta.filter(|m| !m.resumed) {
            maybe_resume(&state, &conversation_id, &message_id, &meta);
        }
        return;
    }
    // The guard's stop, once the container accepted it: the turn is a
    // stopped one even if the stream ended on a verdict other than
    // run.cancelled, and the reply says why it ends — after its last word.
    if loop_stop_ok.load(Ordering::Relaxed) {
        content.push_str(&format!(
            "\n\n(stopped: the agent repeated the same call {LOOP_STOP_AT} times)"
        ));
        stopped = true;
    }
    // A STOPPED TURN IS NOT A FAULT, so it lands like any finished one.
    // Somebody pressed stop — or the guard above pulled the plug on a loop —
    // and the partial reply is exactly what they chose to keep: the row
    // lands COMPLETE with the content, reasoning and tools that arrived, and
    // the stamp beside it is what says the turn was cut short. An error
    // status would put a retry affordance on a turn nobody wants retried,
    // and the death path's auto-resume would re-drive the very run that was
    // just stopped. Everything the completed path does — the guard over the
    // words, the rail's event, the reply notification, the chain — applies to
    // the words a stopped turn managed to say, with two exceptions below.
    flush!("complete");
    if stopped {
        let _ = mark_message_stopped(&state.pg, &message_id).await;
    }
    let _ = touch_conversation(&state.pg, &conversation_id, None).await;
    ledger!();
    // TALA-33: a landed plan turn rewrites the plan's living document
    // server-side (detached; see `spawn_plan_doc_auto_sync`). Only the
    // completed path — the error and death paths above return early — and
    // not a stopped one: half a plan turn must not rewrite the document.
    if !stopped
        && let Some((plan, agent_model, tier)) = usage_meta.as_ref().and_then(|m| {
            m.plan
                .clone()
                .map(|p| (p, m.agent_model.clone(), m.tier.clone()))
        })
    {
        spawn_plan_doc_auto_sync(&state, &conversation_id, plan, agent_model, tier);
    }
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
                let qd = talaria_retrieval_qdrant::real_deps();
                let ed = talaria_retrieval_embed::real_deps();
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
    // Messages queued while this reply streamed become the next turn — the
    // ones still owed an answer. A steer the running agent accepted was
    // answered inside this turn, and after a STOP with nobody having spoken
    // since the chain must stay quiet: `prior_messages` sees through an empty
    // row, so a stopped turn that said nothing would leave its message
    // looking forever unanswered and the chain would re-fire it without end —
    // the loop the stop was pressed to get out of.
    if let Some(meta) = usage_meta
        && unanswered_user_after(&state.pg, &conversation_id, &message_id)
            .await
            .unwrap_or(false)
    {
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
                    sender: meta.sender.clone(),
                },
            )
            .await;
        });
    }
}

/// One quiet attempt to bring a dead turn back, ON THE SAME ROW. After a
/// backoff long enough for a blipped container to return, the row is re-read
/// — still errored, never yet resumed — stamped, and re-driven; the
/// resurrection (not a fresh insert) keeps the thread's shape, and the stamp
/// is the retry-once gate: a turn that dies twice stays dead where a person
/// can see it. Detached, so the dying persist returns and the client branch
/// closes on schedule.
fn maybe_resume(state: &AppState, conversation_id: &str, message_id: &str, meta: &PersistMeta) {
    let state = state.clone();
    let conversation_id = conversation_id.to_string();
    let message_id = message_id.to_string();
    let turn = TurnMeta {
        agent_model: meta.agent_model.clone(),
        tier: meta.tier.clone(),
        plan: meta.plan.clone(),
        research_prompt: meta.research_prompt,
        sender: meta.sender.clone(),
    };
    tokio::spawn(async move {
        tokio::time::sleep(RESUME_BACKOFF).await;
        // Still the errored row we left, and never yet resumed? Anything else
        // — a person nudged it, some other writer landed it — leaves it be.
        if !message_still_errored(&state.pg, &message_id)
            .await
            .unwrap_or(false)
        {
            return;
        }
        let _ = mark_message_resumed(&state.pg, &message_id).await;
        // The stamp precedes the drive on purpose: if the drive then loses to
        // a newer turn (the active-streaming check inside), the row stays an
        // explained error rather than re-queueing itself forever.
        resume_conversation(&state, &conversation_id, &turn, &message_id).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pinned turn is cut from the same list the full-history path
    /// sends: system rows fold into the ephemeral system message, the
    /// history in the middle is dropped (Hermes has it), and the tail is
    /// the message.
    mod session_turn {
        use super::*;

        #[test]
        fn system_rows_fold_and_history_is_dropped() {
            let messages = vec![
                json!({ "role": "system", "content": "mode prompt" }),
                json!({ "role": "system", "content": "sender: Jon" }),
                json!({ "role": "user", "content": "earlier question" }),
                json!({ "role": "assistant", "content": "earlier answer" }),
                json!({ "role": "user", "content": "the new one" }),
            ];
            let (system, message) = session_turn_parts(&messages);
            assert_eq!(system.as_deref(), Some("mode prompt\n\nsender: Jon"));
            assert_eq!(message, json!("the new one"));
        }

        /// Two messages queued during one reply are both unanswered; the
        /// chained turn carries both, oldest first.
        #[test]
        fn a_trailing_run_of_user_rows_joins() {
            let messages = vec![
                json!({ "role": "assistant", "content": "reply" }),
                json!({ "role": "user", "content": "first queued" }),
                json!({ "role": "user", "content": "second queued" }),
            ];
            let (system, message) = session_turn_parts(&messages);
            assert_eq!(system, None);
            assert_eq!(message, json!("first queued\n\nsecond queued"));
        }

        /// An image turn keeps its parts; earlier queued text rides ahead
        /// of them as a text part rather than being flattened away.
        #[test]
        fn content_parts_survive_with_earlier_text_ahead() {
            let messages = vec![
                json!({ "role": "user", "content": "look at this" }),
                json!({ "role": "user", "content": [
                    { "type": "text", "text": "what is it?" },
                    { "type": "image_url", "image_url": { "url": "data:x" } }
                ] }),
            ];
            let (_, message) = session_turn_parts(&messages);
            let parts = message.as_array().expect("parts");
            assert_eq!(parts.len(), 3);
            assert_eq!(parts[0]["text"], "look at this");
            assert_eq!(parts[2]["type"], "image_url");
        }

        #[test]
        fn no_user_tail_is_an_empty_message() {
            let messages = vec![json!({ "role": "system", "content": "only prompt" })];
            let (system, message) = session_turn_parts(&messages);
            assert_eq!(system.as_deref(), Some("only prompt"));
            assert_eq!(message, json!(""));
        }
    }

    /// `messages.tools` — the shape this crate persists and re-serves. The
    /// drain loop is a stream plus a database, but the FOLD inside it is the
    /// two lines below, so the vocabulary that lands in the column is testable
    /// without either: parse the frames, merge them, read the words.
    mod persisted_tools {
        use super::*;

        /// Exactly what the drain loop does with a tool event, at lines 820
        /// and 959: parse, then merge by id.
        fn fold(body: &str) -> Vec<ToolCall> {
            let mut parser = AgentStreamParser::new();
            let mut tools: Vec<ToolCall> = Vec::new();
            let mut events = parser.feed(body.as_bytes());
            events.extend(parser.finish());
            for ev in events {
                if let AgentStreamEvent::Tool {
                    id,
                    name,
                    label,
                    status,
                } = ev
                {
                    tools = merge_tool(&tools, id.as_deref(), &name, &label, status.as_deref());
                }
            }
            tools
        }

        /// A failed call persists as `error`, carrying Hermes' reason as its
        /// label — the row used to say "completed", which read as a tool that
        /// worked and left the failure invisible in history.
        #[test]
        fn a_failed_tool_persists_as_error_with_its_reason() {
            let tools = fold(concat!(
                "event: tool.started\n",
                r#"data: {"message_id":"m1","tool_name":"memory","preview":"🧠 remember"}"#,
                "\n\n",
                "event: tool.failed\n",
                r#"data: {"message_id":"m1","tool_name":"memory","preview":"memory: write refused"}"#,
                "\n\n",
            ));
            assert_eq!(tools.len(), 1, "{tools:?}");
            assert_eq!(tools[0].status, "error");
            assert_eq!(tools[0].label, "memory: write refused");
            assert_eq!(
                serde_json::to_value(&tools).unwrap(),
                json!([{
                    "id": "m1:memory#1",
                    "name": "memory",
                    "label": "memory: write refused",
                    "status": "error"
                }])
            );
        }

        /// `error` is terminal in the column too: a straggler progress frame
        /// addressed to the same call — Hermes keeps the ordinal open after a
        /// failure — must not walk the row back to "running" and leave it
        /// spinning in history forever.
        #[test]
        fn a_straggler_frame_never_reopens_a_failed_row() {
            let tools = fold(concat!(
                "event: tool.started\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","preview":"🖥 ls"}"#,
                "\n\n",
                "event: tool.failed\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","preview":"exit 1"}"#,
                "\n\n",
                "event: tool.progress\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","delta":"still talking"}"#,
                "\n\n",
                "event: tool.completed\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","preview":""}"#,
                "\n\n",
            ));
            assert_eq!(tools.len(), 1, "{tools:?}");
            assert_eq!(tools[0].status, "error");
        }

        /// AND THE SAME FOR A COMPLETION. `tool.progress` addresses the call
        /// still open — the ordinal does not advance when a call ends — so a
        /// straggler frame after `tool.completed` would persist the row as
        /// "running" and leave history showing a tool that never finished.
        /// One terminality rule for both endings, shared with the client's
        /// fold (`ui/src/lib/sse-parse.ts`).
        #[test]
        fn a_straggler_frame_never_reopens_a_completed_row() {
            let tools = fold(concat!(
                "event: tool.started\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","preview":"🖥 ls"}"#,
                "\n\n",
                "event: tool.completed\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","preview":""}"#,
                "\n\n",
                "event: tool.progress\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","delta":"still talking"}"#,
                "\n\n",
            ));
            assert_eq!(tools.len(), 1, "{tools:?}");
            assert_eq!(tools[0].status, "completed");
            assert_eq!(
                tools[0].label, "still talking",
                "the chatter still lands; only the verdict is fixed"
            );
        }

        /// The completions arm's own telemetry, same verdict: a failure rides
        /// `hermes.tool.progress` with a status word, and it lands as `error`.
        #[test]
        fn the_completions_arm_persists_error_too() {
            let tools = fold(concat!(
                "event: hermes.tool.progress\n",
                r#"data: {"tool":"search","toolCallId":"c1","emoji":"🔎","status":"running"}"#,
                "\n\n",
                "event: hermes.tool.progress\n",
                r#"data: {"tool":"search","toolCallId":"c1","label":"quota exhausted","status":"error"}"#,
                "\n\n",
            ));
            assert_eq!(tools.len(), 1, "{tools:?}");
            assert_eq!(tools[0].status, "error");
            assert_eq!(tools[0].label, "quota exhausted");
        }

        /// The three words, and only the three: a completed call still
        /// completes, and a call still open still says running.
        #[test]
        fn the_other_two_words_are_unchanged() {
            let tools = fold(concat!(
                "event: tool.started\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","preview":"🖥 ls"}"#,
                "\n\n",
                "event: tool.completed\n",
                r#"data: {"message_id":"m1","tool_name":"terminal","preview":""}"#,
                "\n\n",
                "event: tool.started\n",
                r#"data: {"message_id":"m1","tool_name":"memory","preview":"🧠 remember"}"#,
                "\n\n",
            ));
            assert_eq!(tools.len(), 2, "{tools:?}");
            assert_eq!(tools[0].status, "completed");
            assert_eq!(tools[1].status, "running");
        }
    }

    /// The silence ceiling parses like a knob: unset defaults to ten minutes,
    /// garbage falls back to the same, and the floor holds at a minute.
    #[test]
    fn idle_secs_floor_and_fallbacks() {
        assert_eq!(idle_secs(None), 600);
        assert_eq!(idle_secs(Some("garbage".into())), 600);
        assert_eq!(idle_secs(Some("3600".into())), 3600);
        assert_eq!(idle_secs(Some("5".into())), 60);
        assert_eq!(idle_secs(Some("0".into())), 60);
    }

    /// The death sentences say what actually happened — the row's content is
    /// the reader's only explanation when no resume comes, so each must name
    /// the STREAM dying, never imply the agent was interrupted or refused.
    #[test]
    fn death_reasons_name_the_stream() {
        assert_eq!(
            TurnDeath::Reset.reason(),
            "the agent's stream connection died mid-turn"
        );
        assert!(TurnDeath::Idle.reason().contains("went silent for "));
        assert!(TurnDeath::Idle.reason().ends_with("s mid-turn"));
    }

    /// The doom-loop guard, at its pure seam. The counting and the verdict are
    /// the whole policy — the steer and the stop behind them are HTTP calls
    /// into a live container, which no unit test here can stand in for.
    mod doom_loop {
        use super::*;

        /// A fresh tally, replayed over a list of calls, so the bookkeeping is
        /// exercised through the verdict rather than beside it.
        fn replay(calls: &[(&str, &str)]) -> Vec<Option<LoopAction>> {
            let mut tally = HashMap::new();
            calls
                .iter()
                .map(|(name, label)| count_repeat(&mut tally, name, label))
                .collect()
        }

        /// The thresholds, on the bare count: a nudge at three, the plug at
        /// five, and silence everywhere else — including PAST five, so a pair
        /// stuck at a dozen repeats still stops once.
        #[test]
        fn verdict_fires_once_per_threshold() {
            assert_eq!(loop_verdict(0), None);
            assert_eq!(loop_verdict(1), None);
            assert_eq!(loop_verdict(2), None);
            assert_eq!(loop_verdict(3), Some(LoopAction::Warn));
            assert_eq!(loop_verdict(4), None);
            assert_eq!(loop_verdict(5), Some(LoopAction::Stop));
            assert_eq!(loop_verdict(6), None);
            assert_eq!(loop_verdict(50), None);
        }

        /// The same call five times: one word, then the plug.
        #[test]
        fn a_repeating_pair_warns_then_stops() {
            let calls = [("read_file", "a.rs"); 5];
            assert_eq!(
                replay(&calls),
                vec![
                    None,
                    None,
                    Some(LoopAction::Warn),
                    None,
                    Some(LoopAction::Stop)
                ]
            );
        }

        /// The ARGUMENTS are what make it a loop. One tool used five times on
        /// five different files is an agent working, and must never be touched.
        #[test]
        fn one_tool_on_different_arguments_is_not_a_loop() {
            let verdicts = replay(&[
                ("read_file", "a.rs"),
                ("read_file", "b.rs"),
                ("read_file", "c.rs"),
                ("read_file", "d.rs"),
                ("read_file", "e.rs"),
            ]);
            assert!(verdicts.iter().all(Option::is_none));
        }

        /// Two pairs are counted apart: interleaving them does not add up to
        /// one loop, and the pair that does repeat still reaches its own
        /// threshold.
        #[test]
        fn pairs_are_counted_separately() {
            let verdicts = replay(&[
                ("grep", "needle"),
                ("read_file", "a.rs"),
                ("grep", "needle"),
                ("read_file", "a.rs"),
                ("grep", "needle"),
            ]);
            assert_eq!(verdicts[4], Some(LoopAction::Warn));
            assert_eq!(verdicts[3], None);
        }

        /// An unlabelled frame carries no arguments to compare, so it is not
        /// counted — every such call of one tool would collide into a single
        /// pair and read as a loop the moment the tool was used three times.
        #[test]
        fn unlabelled_calls_are_never_counted() {
            let verdicts = replay(&[("bash", ""), ("bash", "   "), ("bash", ""), ("bash", "")]);
            assert!(verdicts.iter().all(Option::is_none));
        }
    }

    /// TALA-33: the auto-sync trigger policy, at the seam the tests can
    /// reach. `sync_plan_doc` runs the rewrite through the persona gateway
    /// (run_harness → real transport), which no unit test can fake from this
    /// module — so the DECISION is pure and tested here, and the wiring is
    /// the single call site on the `flush!("complete")` path.
    mod plan_auto_sync {
        use super::*;

        fn plan_meta() -> PlanMeta {
            PlanMeta {
                owner_user_id: "owner-1".into(),
                title: Some("The plan".into()),
                surface: "plan".into(),
            }
        }

        fn now() -> i64 {
            1_000_000
        }

        /// A completed plan turn with a document not freshly saved syncs.
        #[test]
        fn plan_completion_triggers_sync() {
            assert!(auto_sync_should_start(
                Some(&plan_meta()),
                false,
                Some(now() - AUTO_SYNC_RECENT_MS - 1),
                now()
            ));
        }

        /// No plan meta (every other conversation kind) never syncs.
        #[test]
        fn non_plan_does_not_sync() {
            assert!(!auto_sync_should_start(None, false, None, now()));
        }

        /// An auto-sync already in flight for the plan skips — overlapping
        /// triggers never queue up behind each other.
        #[test]
        fn overlapping_trigger_skips() {
            assert!(!auto_sync_should_start(
                Some(&plan_meta()),
                true,
                Some(now() - AUTO_SYNC_RECENT_MS - 1),
                now()
            ));
        }

        /// A document saved within the recency window skips — the client's
        /// manual turn-complete sync just landed and must not double-burn.
        #[test]
        fn fresh_manual_save_skips() {
            assert!(!auto_sync_should_start(
                Some(&plan_meta()),
                false,
                Some(now() - 1_000),
                now()
            ));
        }

        /// A first-ever sync (no document yet) proceeds: `ensure_plan_doc`
        /// creates it, and there is nothing fresh to protect.
        #[test]
        fn no_document_yet_syncs() {
            assert!(auto_sync_should_start(
                Some(&plan_meta()),
                false,
                None,
                now()
            ));
        }

        /// The error paths (frame_error and death) return before the call
        /// site, so the policy is never consulted for them — this pins that
        /// by construction: the only trigger lives after the complete flush.
        #[test]
        fn boundary_exactly_at_window_is_recent() {
            // Exactly 5s old is still "fresh" — the window is a strict >.
            assert!(!auto_sync_should_start(
                Some(&plan_meta()),
                false,
                Some(now() - AUTO_SYNC_RECENT_MS),
                now()
            ));
        }
    }
}
