// The inbox.focus engine — policy, sources, timeline, conversation assembly.
pub mod conversation;
pub mod policy;
pub mod sources;
pub mod timeline;
pub mod types;

// The Inbox Focus Queue engine.
//
// A normalized, decision-first read model over Talaria's existing sources.
// Source tables remain authoritative. This module owns only queue ranking,
// per-user focus state, constrained assistant briefs/commands, and allowlisted
// action execution.
//
// THE ADAPTERS are four harness runs with a deadline wrapped around each.
// The deadlines are the entire point of that layer: ten seconds for a turn
// nobody is watching (a card's one-line brief, a structured validation with
// an honest fallback), ninety for a reply somebody asked for and is watching
// words arrive. The timeout wraps the run future and the future's drop is the
// abort — a transport mid-request is dropped rather than signal-noticed, same
// 10s/90s wall clock, same error surfaces.
//
// ERROR MODEL: the engine fails two ways — a database error (→ the
// platform's generic 500) and a product sentence (`That response model is no
// longer available to this account.`, a secretbox that cannot seal).
// `FocusError` keeps the two apart; the routes map both to the house 500,
// except where a caller's own catch is the contract (the actions route's
// `{status:'failed'}` paths are values here, not throws).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::audit::{AuditEntry, log_audit};
use crate::auth::sha256_hex;
use crate::boards::{board_role, can_edit};
use crate::channel_replies::{notify_dm_message, notify_user_mentions, trigger_agent_replies};
use crate::channels::{channel_role, insert_channel_message, mark_channel_read};
use crate::daily_brief::focus::key_of;
use crate::fleet::routed_model_for;
use crate::google::pending_actions::{PendingAction, decide_action, list_pending};
use crate::harness::defs::inbox_focus::{
    CommandTurn, FocusBriefInput, FocusCommandInput, FocusCommandMode, FocusHarnessItem,
    FocusReplyInput, FocusSeatRole, OwnedTurn, allowed_focus_action_ids, inbox_brief_harness,
    inbox_command_harness, inbox_reply_harness, validate_command_object,
};
use crate::harness::run::{
    DeltaFn, HarnessError, RunContext, StreamFn, StreamOptions, run_harness, run_harness_streamed,
};
use crate::harness::transport::{TransportRequest, dispatch_stream};
use crate::inbox_focus::policy::{
    CommandProposal, FocusProposalSource, confirmation_miss_invalidates, dedupe_items,
    deterministic_proposal, proposal_source_for, proposal_source_of, requires_human_confirmation,
    sort_items, valid_brief,
};
use crate::inbox_focus::sources::{approval_items, channel_items, notification_items, task_items};
use crate::inbox_focus::types::{AssistantBrief, FocusAssistant, RawFocusItem, wire_item};
use crate::model::access::gateway_models_for;
use crate::notify::NotifyDeps;
use crate::session::{SessionUser, actor_of};
use crate::state::AppState;
use crate::statuses::status_meta;
use crate::tasks::{TaskDeps, TaskError, complete_quality_review, get_task};

const BRIEF_RETRY_MS: i64 = 2 * 60_000;
const CONFIRMATION_MS: i64 = 10 * 60_000;
const UNDO_MS: i64 = 30_000;

/// The cap on a turn NOBODY IS WATCHING: the one-line brief on a queue card,
/// and the structured command validation behind an action. Both are drawn
/// alongside other content, both have an honest fallback, and a card that is
/// ten seconds late is worse than a card that says it could not be written.
const DEADLINE_MS: u64 = 10_000;

/// The cap on a turn SOMEBODY IS WAITING FOR — a question they typed into the
/// assistant panel. Nine times the brief's, and the asymmetry is the entire
/// point: a comparable question on this workspace's own assistant measures
/// ~23 seconds, so a 10s cap reported the assistant as unavailable when it
/// was merely interrupted. A person who has asked a question will wait.
const REPLY_DEADLINE_MS: u64 = 90_000;

/// A failure out of the engine: a database error, or a product sentence
/// carried as a message.
#[derive(Debug)]
pub enum FocusError {
    Db(sqlx::Error),
    Throw(String),
}

impl From<sqlx::Error> for FocusError {
    fn from(e: sqlx::Error) -> Self {
        FocusError::Db(e)
    }
}

impl std::fmt::Display for FocusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FocusError::Db(e) => write!(f, "{e}"),
            FocusError::Throw(m) => write!(f, "{m}"),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn token_hash(token: &str) -> String {
    sha256_hex(token)
}

/// 24 random bytes, base64url no pad. NOT the session module's 32-byte token;
/// the confirmation token's length is what the cipher column stores.
fn random_token_24() -> String {
    let mut buf = [0u8; 24];
    getrandom::fill(&mut buf).expect("system rng");
    URL_SAFE_NO_PAD.encode(buf)
}

/// The current time as ISO — shared with the conversation module.
pub(crate) fn now_iso() -> String {
    crate::agent_auth::epoch_ms_to_iso(now_ms())
}

// ── The assistant adapters────────────────────────

/// One brief for one queue item, on the owner's own assistant. `valid_brief`
/// still runs on the parsed value: the schema proved
/// the shape, and this clamps the strings to what the card can render and
/// drops a `recommendedActionId` the item does not offer. None means the
/// caller keeps whatever brief it already had — fire-and-forget by design.
pub async fn request_focus_brief(
    state: &AppState,
    model: &str,
    input: &FocusBriefInput,
    caller: &str,
) -> Result<Option<AssistantBrief>, String> {
    let input_value = serde_json::to_value(input).expect("brief input serializes");
    let ctx = RunContext {
        caller: caller.to_string(),
        model: Some(model.to_string()),
        ..Default::default()
    };
    let result = tokio::time::timeout(
        Duration::from_millis(DEADLINE_MS),
        run_harness(state, &inbox_brief_harness(), &input_value, ctx),
    )
    .await
    .map_err(|_| "Inbox assistant timed out".to_string())?
    .map_err(|HarnessError(m)| m)?;
    Ok(valid_brief(result.value.as_ref(), &input.actions))
}

/// One command turn, on whichever model this seat uses. THE AUTHORITY GATE:
/// `validate_command_object` runs AFTER the schema
/// parse and is not negotiable — the schema proved the value has a `message`
/// and an optional `actionId`, and this proves the `actionId` is one the
/// owner's instruction actually authorized. The allowlist comes from
/// `allowed_focus_action_ids` — the SAME call the prompt render made — with
/// the runner's answer about whether this model earned the widened surface.
/// Deriving both from one function is what stops the prompt and the gate from
/// drifting. `effort` is the owner's pick, already validated by the caller.
pub async fn request_focus_command(
    state: &AppState,
    model: &str,
    input: &FocusCommandInput,
    caller: &str,
    effort: Option<&str>,
) -> Result<Option<CommandTurn>, String> {
    let input_value = serde_json::to_value(input).expect("command input serializes");
    let ctx = RunContext {
        caller: caller.to_string(),
        model: Some(model.to_string()),
        effort: effort.map(str::to_string),
        ..Default::default()
    };
    let result = tokio::time::timeout(
        Duration::from_millis(DEADLINE_MS),
        run_harness(state, &inbox_command_harness(), &input_value, ctx),
    )
    .await
    .map_err(|_| "Inbox assistant timed out".to_string())?
    .map_err(|HarnessError(m)| m)?;
    let Some(value) = result.value else {
        return Ok(None);
    };
    let allowed: HashSet<String> = allowed_focus_action_ids(input, result.widened)
        .into_iter()
        .collect();
    Ok(validate_command_object(&value, &allowed))
}

/// A detached conversational reply. `max` is applied to the
/// finished text rather than to the stream: truncating at the end produces
/// the same cap, and a guarded or repaired reply is what gets persisted.
/// `value.slice(0, max) || null` — an empty reply is None.
async fn reply_turn(
    state: &AppState,
    model: &str,
    input: &FocusReplyInput,
    max: usize,
    caller: &str,
) -> Result<Option<String>, String> {
    let input_value = serde_json::to_value(input).expect("reply input serializes");
    let ctx = RunContext {
        caller: caller.to_string(),
        model: Some(model.to_string()),
        ..Default::default()
    };
    let result = tokio::time::timeout(
        Duration::from_millis(REPLY_DEADLINE_MS),
        run_harness(state, &inbox_reply_harness(), &input_value, ctx),
    )
    .await
    .map_err(|_| "Inbox assistant timed out".to_string())?
    .map_err(|HarnessError(m)| m)?;
    Ok(finished_reply(result.value, max))
}

/// The guarded final text every reply shape shares: `slice(0, max) || null`.
fn finished_reply(value: Option<Value>, max: usize) -> Option<String> {
    value
        .and_then(|v| v.as_str().map(str::to_string))
        .map(|text| crate::body::truncate_utf16(&text, max).to_string())
        .filter(|text| !text.is_empty())
}

/// The reply on a FLEET PERSONA (`inbox:{model}` ledger attribution). Two
/// exports rather than one because both signatures are somebody else's call
/// sites; they are one harness underneath.
pub async fn request_text(
    state: &AppState,
    model: &str,
    input: &FocusReplyInput,
    max: usize,
) -> Result<Option<String>, String> {
    reply_turn(state, model, input, max, &format!("inbox:{model}")).await
}

/// The same reply on an ORG GATEWAY model the owner picked; the caller names
/// its own ledger attribution.
pub async fn request_gateway_text(
    state: &AppState,
    model: &str,
    input: &FocusReplyInput,
    caller: &str,
) -> Result<Option<String>, String> {
    reply_turn(state, model, input, 20_000, caller).await
}

/// The same conversational reply, STREAMED: deltas as the model writes them,
/// because a status line that sits there for a twenty-second turn with
/// nothing under it reads exactly like the assistant not replying.
///
/// Shape: a channel of deltas plus the join handle carrying the run's own
/// guarded text — not the concatenated deltas. The consumer drains `deltas`
/// (None = the run finished and dropped its emitter), then awaits `done` for
/// the guarded text or the failure. A timeout drops the run future, which
/// drops the emitter, which closes the channel — the abort and the close are
/// the same event.
pub struct StreamedReply {
    pub deltas: mpsc::UnboundedReceiver<String>,
    pub done: tokio::task::JoinHandle<Result<Option<String>, String>>,
}

pub async fn stream_reply(
    state: &AppState,
    model: &str,
    input: FocusReplyInput,
    caller: &str,
    max: usize,
    effort: Option<&str>,
) -> StreamedReply {
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    let on_delta: DeltaFn = Arc::new(move |delta: &str| {
        let _ = tx.send(delta.to_string());
    });
    // The streaming transport is resolved per call by the same kind lookup
    // the blocking runner uses; wrapping `dispatch_stream` in the runner's
    // StreamFn shape is the whole bridge.
    let stream_state = state.clone();
    let stream: StreamFn = Arc::new(move |req: TransportRequest, emit: DeltaFn| {
        let st = stream_state.clone();
        Box::pin(async move { dispatch_stream(&st, &req, |s: &str| emit(s)).await })
    });
    let input_value = serde_json::to_value(&input).expect("reply input serializes");
    let ctx = RunContext {
        caller: caller.to_string(),
        model: Some(model.to_string()),
        effort: effort.map(str::to_string),
        ..Default::default()
    };
    let st = state.clone();
    let done = tokio::spawn(async move {
        let harness = inbox_reply_harness();
        let run = run_harness_streamed(
            &st,
            &harness,
            &input_value,
            ctx,
            StreamOptions {
                stream,
                on_delta: Some(on_delta),
            },
        );
        let result = tokio::time::timeout(Duration::from_millis(REPLY_DEADLINE_MS), run)
            .await
            .map_err(|_| "Inbox assistant timed out".to_string())?
            .map_err(|HarnessError(m)| m)?;
        Ok(finished_reply(result.value, max))
    });
    StreamedReply { deltas: rx, done }
}

// ── The engine ───────────────────────────────

pub async fn focus_assistant_for(
    pg: &PgPool,
    user_id: &str,
) -> Result<FocusAssistant, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as(
        "select model, display_name as name from agent_defs \
         where owner_user_id = $1::uuid and enabled \
         order by created_at asc limit 1",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(match row {
        Some((model, name)) => FocusAssistant {
            configured: true,
            model: Some(model),
            name: Some(name),
        },
        None => FocusAssistant {
            configured: false,
            model: None,
            name: None,
        },
    })
}

/// One focus-state row, the four fields the queue reads. Timestamps arrive
/// as epoch ms: every use is an ms compare or an ISO render, so ms is the
/// honest shape.
struct FocusStateRow {
    snoozed_until: Option<i64>,
    content_fingerprint: Option<String>,
    brief: Option<Value>,
    brief_generated_at: Option<i64>,
}

async fn focus_states(
    pg: &PgPool,
    user_id: &str,
    keys: &[String],
) -> Result<HashMap<String, FocusStateRow>, sqlx::Error> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    #[allow(clippy::type_complexity)] // the state row's own columns, one each
    let rows: Vec<(
        String,
        Option<i64>,
        Option<String>,
        Option<Value>,
        Option<i64>,
    )> = sqlx::query_as(
        "select source_type || ':' || source_id as key, \
                    (trunc(extract(epoch from snoozed_until) * 1000))::bigint, \
                    content_fingerprint, brief, \
                    (trunc(extract(epoch from brief_generated_at) * 1000))::bigint \
             from inbox_focus_state \
             where user_id = $1::uuid and source_type || ':' || source_id = any($2)",
    )
    .bind(user_id)
    .bind(keys)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(key, snoozed_until, content_fingerprint, brief, brief_generated_at)| {
                (
                    key,
                    FocusStateRow {
                        snoozed_until,
                        content_fingerprint,
                        brief,
                        brief_generated_at,
                    },
                )
            },
        )
        .collect())
}

/// The brief cache's claim half, detached — no caller ever waits on it. The
/// claim row records the attempt even when the model fails, so a bad model
/// costs one retry window rather than a spinning card.
fn spawn_claim_and_generate_brief(
    state: &AppState,
    user_id: &str,
    assistant: &FocusAssistant,
    item: &RawFocusItem,
) {
    if !assistant.configured || assistant.model.is_none() {
        return;
    }
    let state = state.clone();
    let user_id = user_id.to_string();
    let model = assistant
        .model
        .clone()
        .expect("configured assistant carries a model");
    let item = item.clone();
    tokio::spawn(async move {
        let claimed = sqlx::query_scalar::<_, String>(
            "insert into inbox_focus_state \
                (user_id, source_type, source_id, content_fingerprint, brief, brief_generated_at) \
             values ($1::uuid, $2, $3, $4, null, now()) \
             on conflict (user_id, source_type, source_id) do update \
             set content_fingerprint = excluded.content_fingerprint, brief = null, \
                 brief_generated_at = now(), updated_at = now() \
             where inbox_focus_state.content_fingerprint is distinct from excluded.content_fingerprint \
                or ( \
                  inbox_focus_state.brief is null \
                  and ( \
                    inbox_focus_state.brief_generated_at is null \
                    or inbox_focus_state.brief_generated_at < now() - ($5::bigint * interval '1 millisecond') \
                  ) \
                ) \
             returning source_id",
        )
        .bind(&user_id)
        .bind(&item.source_type)
        .bind(&item.source_id)
        .bind(&item.source_fingerprint)
        .bind(BRIEF_RETRY_MS)
        .fetch_optional(&state.pg)
        .await;
        if matches!(claimed, Ok(Some(_))) {
            let _ = generate_brief(&state, &user_id, &model, &item).await;
        }
    });
}

/// The persistence half. A null brief leaves the previous row untouched: the
/// claim row already recorded the attempt, so a bad model costs one retry
/// window rather than an overwritten card. Every failure is swallowed here —
/// nothing waits on this task, so no caller ever sees them.
async fn generate_brief(
    state: &AppState,
    user_id: &str,
    model: &str,
    item: &RawFocusItem,
) -> Result<(), FocusError> {
    let input = brief_input_for(item);
    let Ok(Some(brief)) =
        request_focus_brief(state, model, &input, &format!("user:{user_id}")).await
    else {
        return Ok(());
    };
    let stored = serde_json::to_value(&brief).expect("brief serializes");
    sqlx::query(
        "update inbox_focus_state \
         set brief = $4, brief_generated_at = now(), updated_at = now() \
         where user_id = $1::uuid and source_type = $2 and source_id = $3 \
           and content_fingerprint = $5",
    )
    .bind(user_id)
    .bind(&item.source_type)
    .bind(&item.source_id)
    .bind(&stored)
    .bind(&item.source_fingerprint)
    .execute(&state.pg)
    .await?;
    Ok(())
}

/// The brief input off a raw item — exactly the four fields the def reads.
fn brief_input_for(item: &RawFocusItem) -> FocusBriefInput {
    FocusBriefInput {
        source_type: item.source_type.clone(),
        evidence: item.evidence.clone(),
        metadata: item.metadata.clone(),
        actions: item.actions.clone(),
    }
}

/// The narrow item slice the command harnesses see — `FocusHarnessItem`, not
/// the card: the source fingerprint and the ranking bucket are Talaria's
/// bookkeeping and have no business in a prompt.
fn harness_item_of(item: &RawFocusItem) -> FocusHarnessItem {
    FocusHarnessItem {
        key: item.key.clone(),
        question: item.question.clone(),
        source_href: item.source_href.clone(),
        evidence: item.evidence.clone(),
        metadata: item.metadata.clone(),
        actions: item.actions.clone(),
    }
}

async fn raw_focus_items(
    pg: &PgPool,
    user: &SessionUser,
) -> Result<Vec<RawFocusItem>, sqlx::Error> {
    // The four sources in parallel — sequential would stack four query
    // round-trips the queue need not wait out in series.
    let (approvals, tasks, channels, notifications) = tokio::try_join!(
        approval_items(pg, user, None),
        task_items(pg, &user.id, None),
        channel_items(pg, &user.id, None),
        notification_items(pg, &user.id, None)
    )?;
    let mut all = approvals;
    all.extend(tasks);
    all.extend(channels);
    all.extend(notifications);
    Ok(sort_items(dedupe_items(all), now_ms()))
}

/// Queue listing options. Defaults: enrich true, include_snoozed false; the
/// routes pass both explicitly.
pub struct FocusQueueOptions {
    pub enrich: bool,
    pub include_snoozed: bool,
}

/// The queue a page renders: visible items (snoozed ones unless asked for),
/// the cache overlay, first-item enrichment with its detached claim, the
/// counts, and the assistant fact.
pub async fn list_focus_queue(
    state: &AppState,
    user: &SessionUser,
    options: FocusQueueOptions,
) -> Result<Value, sqlx::Error> {
    let pg = &state.pg;
    let raw = raw_focus_items(pg, user).await?;
    let keys = raw.iter().map(|item| item.key.clone()).collect::<Vec<_>>();
    let (states, assistant) = tokio::try_join!(
        focus_states(pg, &user.id, &keys),
        focus_assistant_for(pg, &user.id)
    )?;
    let now = now_ms();
    let mut visible: Vec<RawFocusItem> = raw
        .into_iter()
        .filter(|item| {
            if options.include_snoozed {
                return true;
            }
            let until = states.get(&item.key).and_then(|s| s.snoozed_until);
            until.is_none_or(|until| until <= now)
        })
        .collect();

    // First-item enrichment happens on the item objects, before the wire
    // mapping — `briefStatus` is a field of the card, and the enrich writes
    // the very object the mapping later serializes. (Enrich-then-overlay and
    // overlay-then-enrich land the same bytes: the overlay touches three
    // other fields.)
    if options.enrich
        && let Some(item) = visible.first_mut()
    {
        let row = states.get(&item.key);
        let claimed_at = row.and_then(|s| s.brief_generated_at).unwrap_or(0);
        let cache_fresh = row.is_some_and(|s| {
            s.content_fingerprint.as_deref() == Some(item.source_fingerprint.as_str())
                && valid_brief(s.brief.as_ref(), &item.actions).is_some()
        });
        if !cache_fresh
            && (claimed_at == 0
                || now - claimed_at > BRIEF_RETRY_MS
                || row.and_then(|s| s.content_fingerprint.as_deref())
                    != Some(item.source_fingerprint.as_str()))
        {
            item.brief_status = if assistant.configured {
                "pending"
            } else {
                "fallback"
            }
            .into();
            spawn_claim_and_generate_brief(state, &user.id, &assistant, item);
        } else if !cache_fresh && assistant.configured {
            item.brief_status = "pending".into();
        }
    }

    // The cache overlay: `{...item, ...cached, briefStatus:'cached'}` — the
    // brief's three fields overwrite the card's in place (JSON.stringify
    // keeps first-seen positions), so the wire key order never moves.
    let items = visible
        .iter()
        .map(|item| {
            let mut wire = wire_item(item);
            let cached = states.get(&item.key).and_then(|s| {
                (s.content_fingerprint.as_deref() == Some(item.source_fingerprint.as_str()))
                    .then(|| valid_brief(s.brief.as_ref(), &item.actions))
                    .flatten()
            });
            if let Some(cached) = cached {
                let object = wire.as_object_mut().expect("wire item is an object");
                object.insert("question".into(), json!(cached.question));
                object.insert("recommendation".into(), json!(cached.recommendation));
                object.insert(
                    "recommendedActionId".into(),
                    json!(cached.recommended_action_id),
                );
                object.insert("briefStatus".into(), json!("cached"));
            }
            wire
        })
        .collect::<Vec<_>>();

    let total = items.len();
    let count = |source_type: &str| {
        items
            .iter()
            .filter(|item| item.get("sourceType") == Some(&json!(source_type)))
            .count()
    };
    Ok(json!({
        "items": items,
        "counts": {
            "total": total,
            "approvals": count("approval"),
            "tasks": count("task"),
            "comms": count("channel"),
            "notifications": count("notification"),
        },
        "assistant": serde_json::to_value(&assistant).expect("assistant serializes"),
    }))
}

/// The badge count: visible items, snoozed ones excluded.
pub async fn focus_summary(pg: &PgPool, user: &SessionUser) -> Result<usize, sqlx::Error> {
    let raw = raw_focus_items(pg, user).await?;
    let keys = raw.iter().map(|item| item.key.clone()).collect::<Vec<_>>();
    let states = focus_states(pg, &user.id, &keys).await?;
    let now = now_ms();
    Ok(raw
        .into_iter()
        .filter(|item| {
            let until = states.get(&item.key).and_then(|s| s.snoozed_until);
            until.is_none_or(|until| until <= now)
        })
        .count())
}

/// The tri-state snooze and the viewed mark. The outer Option is "don't
/// touch the column" vs a value; truthiness means an empty string clears the
/// snooze too, which `filter` reproduces.
pub async fn update_focus_state(
    pg: &PgPool,
    user: &SessionUser,
    source_type: &str,
    source_id: &str,
    snoozed_until: Option<Option<&str>>,
    viewed: bool,
) -> Result<bool, sqlx::Error> {
    let key = key_of(source_type, source_id);
    if find_focus_item_for_user(pg, user, &key).await?.is_none() {
        return Ok(false);
    }
    let update_snooze = snoozed_until.is_some();
    let snoozed = snoozed_until.flatten().filter(|s| !s.is_empty());
    let viewed_at = viewed.then(now_iso);
    sqlx::query(
        "insert into inbox_focus_state (user_id, source_type, source_id, snoozed_until, viewed_at) \
         values ($1::uuid, $2, $3, $4::timestamptz, $5::timestamptz) \
         on conflict (user_id, source_type, source_id) do update \
         set snoozed_until = case when $6 then excluded.snoozed_until else inbox_focus_state.snoozed_until end, \
             viewed_at = case when $7 then excluded.viewed_at else inbox_focus_state.viewed_at end, \
             updated_at = now()",
    )
    .bind(&user.id)
    .bind(source_type)
    .bind(source_id)
    .bind(snoozed)
    .bind(viewed_at)
    .bind(update_snooze)
    .bind(viewed)
    .execute(pg)
    .await?;
    Ok(true)
}

pub async fn find_focus_item_for_user(
    pg: &PgPool,
    user: &SessionUser,
    key: &str,
) -> Result<Option<RawFocusItem>, sqlx::Error> {
    // `indexOf(':') <= 0` covers both the miss (-1) and a leading colon (0).
    let separator = key.find(':').unwrap_or(0);
    if separator == 0 {
        return Ok(None);
    }
    let source_type = &key[..separator];
    let source_id = &key[separator + 1..];
    if source_id.is_empty() {
        return Ok(None);
    }
    let items = match source_type {
        "approval" => approval_items(pg, user, Some(source_id)).await?,
        "task" => task_items(pg, &user.id, Some(source_id)).await?,
        "channel" => channel_items(pg, &user.id, Some(source_id)).await?,
        "notification" => notification_items(pg, &user.id, Some(source_id)).await?,
        _ => return Ok(None),
    };
    Ok(items.into_iter().find(|item| item.key == key))
}

async fn pending_approval_for(
    pg: &PgPool,
    user: &SessionUser,
    id: &str,
) -> Result<Option<PendingAction>, sqlx::Error> {
    let pending = list_pending(pg, &user.id, user.role == "admin").await?;
    Ok(pending.into_iter().find(|action| action.id == id))
}

/// One row per decision, with the focus context written server-side in
/// literal key order. `completed_at` is `now()` for exactly the three
/// terminal statuses, decided in SQL from the status being written.
#[allow(clippy::too_many_arguments)]
async fn create_decision(
    pg: &PgPool,
    user_id: &str,
    item: &RawFocusItem,
    instruction: Option<&str>,
    action_id: Option<&str>,
    agent_model: Option<&str>,
    delegate_model: Option<&str>,
    status: &str,
    proposal: Option<&Value>,
    outcome: Option<&Value>,
    confirmation_token_hash: Option<&str>,
    expires_at: Option<&str>,
) -> Result<String, sqlx::Error> {
    let focus_context = json!({
        "key": format!("{}:{}", item.source_type, item.source_id),
        "question": item.question,
        "sourceHref": item.source_href,
        "sourceType": item.source_type,
        "sourceId": item.source_id,
    });
    let id: (String,) = sqlx::query_as(
        "insert into inbox_decisions ( \
            user_id, source_type, source_id, instruction, action_id, agent_model, delegate_model, \
            status, proposal, outcome, confirmation_token_hash, expires_at, focus_context, \
            completed_at \
         ) values ( \
            $1::uuid, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::timestamptz, $13, \
            case when $8 = any(array['completed','failed','cancelled']) then now() else null end \
         ) returning id::text",
    )
    .bind(user_id)
    .bind(&item.source_type)
    .bind(&item.source_id)
    .bind(instruction)
    .bind(action_id)
    .bind(agent_model)
    .bind(delegate_model)
    .bind(status)
    .bind(proposal)
    .bind(outcome)
    .bind(confirmation_token_hash)
    .bind(expires_at)
    .bind(&focus_context)
    .fetch_one(pg)
    .await?;
    Ok(id.0)
}

/// The confirmation card's {title, details}, built per branch. The approval
/// branch's details are the four audit-shape facts; the
/// channel reply branch is the message; everything else is the payload
/// (`payload ?? {}` — only null/absent collapses to the empty object).
fn exact_preview(
    item: &RawFocusItem,
    action_id: &str,
    payload: &Value,
    approval: Option<&PendingAction>,
) -> Value {
    if let Some(approval) = approval {
        let base = approval
            .summary
            .clone()
            .unwrap_or_else(|| approval.kind.clone());
        let title = if action_id == "approve" {
            base
        } else {
            format!("Reject {base}")
        };
        return json!({
            "title": title,
            "details": {
                "kind": approval.kind,
                "payload": approval.payload,
                "agentModel": approval.agent_model,
                "isOrg": approval.is_org,
            },
        });
    }
    if item.source_type == "channel" && action_id == "reply" {
        return json!({
            "title": "Post this reply",
            "details": {
                "message": payload.get("message")
                    .filter(|m| !m.is_null())
                    .cloned()
                    .unwrap_or(json!("")),
            },
        });
    }
    json!({
        "title": item.question,
        "details": match payload {
            Value::Null => json!({}),
            v => v.clone(),
        },
    })
}

async fn cancel_decision(
    pg: &PgPool,
    user: &SessionUser,
    decision_id: &str,
) -> Result<Value, sqlx::Error> {
    let cancelled = sqlx::query_scalar::<_, String>(
        "update inbox_decisions set status = 'cancelled', completed_at = now(), \
            confirmation_token_hash = null, proposal = proposal - 'confirmationCipher' \
         where id = $1::uuid and user_id = $2::uuid and status = 'proposed' \
         returning id",
    )
    .bind(decision_id)
    .bind(&user.id)
    .fetch_optional(pg)
    .await?;
    Ok(if cancelled.is_some() {
        json!({ "status": "cancelled", "decisionId": decision_id })
    } else {
        json!({ "status": "stale", "message": "That confirmation is no longer pending." })
    })
}

/// `outcome.beforeCursor ?? 0` run through JS `Number(...)`: a number passes
/// through, a numeric string parses, anything else is 0. The outcomes here
/// are only ever written by the mark-read arm as numbers; the coercion is
/// fidelity, not expectation.
fn number_of(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// The only reversible focus action is a mark-read, and only within the undo
/// window, and only when the source still sits exactly where the decision
/// left it (the read-at / cursor guards make a race a no-op rather than a
/// wrong restore).
async fn undo_decision(
    pg: &PgPool,
    user: &SessionUser,
    decision_id: &str,
) -> Result<Value, sqlx::Error> {
    // `outcome` is nullable on disk — every completed row today carries one,
    // but a future writer that completes without deciding must cost this
    // person an empty restore, not the whole undo read.
    let decision: Option<(String, String, Option<String>, Option<Value>)> = sqlx::query_as(
        "select source_type, source_id, action_id, outcome \
         from inbox_decisions \
         where id = $1::uuid and user_id = $2::uuid and status = 'completed' \
           and completed_at > now() - ($3::bigint * interval '1 millisecond')",
    )
    .bind(decision_id)
    .bind(&user.id)
    .bind(UNDO_MS)
    .fetch_optional(pg)
    .await?;
    let Some((source_type, source_id, action_id, outcome_value)) = decision else {
        return Ok(stale_result("Undo is no longer available."));
    };
    if action_id.as_deref() != Some("mark_read") {
        return Ok(stale_result("Undo is no longer available."));
    }
    let outcome = outcome_value
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let restored: bool;
    if source_type == "notification" {
        let after_read_at = outcome
            .get("afterReadAt")
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(after_read_at) = after_read_at else {
            return Ok(stale_result(
                "Undo can no longer verify the original read action.",
            ));
        };
        let updated = sqlx::query_scalar::<_, String>(
            "update notifications set read_at = null \
             where id = $1::uuid and user_id = $2::uuid and read_at = $3::timestamptz \
             returning id::text",
        )
        .bind(&source_id)
        .bind(&user.id)
        .bind(&after_read_at)
        .fetch_optional(pg)
        .await?;
        restored = updated.is_some();
    } else if source_type == "channel" {
        let before = number_of(outcome.get("beforeCursor")).max(0);
        let after = number_of(outcome.get("afterCursor")).max(0);
        let updated = sqlx::query_scalar::<_, String>(
            "update channel_members set last_read_seq = $3 \
             where channel_id = $1::uuid and user_id = $2::uuid and last_read_seq = $4 \
             returning channel_id::text",
        )
        .bind(&source_id)
        .bind(&user.id)
        .bind(before as i32)
        .bind(after as i32)
        .fetch_optional(pg)
        .await?;
        restored = updated.is_some();
    } else {
        return Ok(stale_result("This action cannot be undone."));
    }
    if !restored {
        return Ok(stale_result(
            "New activity changed this source, so the previous read state was not restored.",
        ));
    }
    // `{...outcome, undone: true}` — the stored jsonb order with undone
    // appended (or overwritten in place if a re-undo ever lands).
    let mut undone_outcome = Value::Object(outcome);
    if let Some(object) = undone_outcome.as_object_mut() {
        object.insert("undone".into(), json!(true));
    }
    // `where id = ${decisionId}` — no user_id re-check on the final write;
    // ownership was proven by the read above.
    sqlx::query(
        "update inbox_decisions set status = 'cancelled', outcome = $2, completed_at = now() \
         where id = $1::uuid",
    )
    .bind(decision_id)
    .bind(&undone_outcome)
    .execute(pg)
    .await?;
    log_audit(
        pg,
        AuditEntry {
            actor: &actor_of(user),
            action: "inbox.focus.undo",
            target_type: &source_type,
            target_id: Some(&source_id),
            target_label: None,
            before: None,
            after: Some(json!({ "decisionId": decision_id })),
        },
    )
    .await;
    Ok(json!({ "status": "undone", "decisionId": decision_id }))
}

fn stale_result(message: &str) -> Value {
    json!({ "status": "stale", "message": message })
}

/// The gate's answer: nothing executes, a decision row holds the exact
/// proposal with the token sealed INTO it, and the token is handed back
/// once. `source` is carried onto the row because this write REPLACES the
/// command proposal wholesale, and the reissue path re-derives the same
/// question from what it finds there — dropping it would leave a widened
/// sign-off looking like an action that never needed confirming.
#[allow(clippy::too_many_arguments)]
async fn proposed_confirmation(
    state: &AppState,
    user: &SessionUser,
    item: &RawFocusItem,
    action_id: &str,
    payload: &Value,
    approval: Option<&PendingAction>,
    agent_model: Option<&str>,
    delegate_model: Option<&str>,
    source: FocusProposalSource,
    existing_decision_id: Option<&str>,
) -> Result<Value, FocusError> {
    let token = random_token_24();
    let expires_iso = crate::agent_auth::epoch_ms_to_iso(now_ms() + CONFIRMATION_MS);
    let preview = exact_preview(item, action_id, payload, approval);
    let cipher = state
        .secretbox()
        .await
        .and_then(|sb| sb.seal(&token).map_err(|e| e.to_string()))
        .map_err(FocusError::Throw)?;
    let proposal = json!({
        "preview": preview,
        "payload": payload,
        "source": source.as_str(),
        "sourceFingerprint": item.source_fingerprint,
        "confirmationCipher": cipher,
    });
    let decision_id = match existing_decision_id {
        Some(existing) => {
            let updated = sqlx::query_scalar::<_, String>(
                "update inbox_decisions \
                 set proposal = $3, confirmation_token_hash = $4, expires_at = $5::timestamptz, \
                     agent_model = $6, delegate_model = $7 \
                 where id = $1::uuid and user_id = $2::uuid and source_type = $8 \
                   and source_id = $9 and action_id = $10 and status = 'proposed' \
                   and confirmation_token_hash is null \
                 returning id",
            )
            .bind(existing)
            .bind(&user.id)
            .bind(&proposal)
            .bind(token_hash(&token))
            .bind(&expires_iso)
            .bind(agent_model)
            .bind(delegate_model)
            .bind(&item.source_type)
            .bind(&item.source_id)
            .bind(action_id)
            .fetch_optional(&state.pg)
            .await?;
            if updated.is_none() {
                return Ok(json!({
                    "status": "stale",
                    "message": "That command proposal is no longer available.",
                }));
            }
            existing.to_string()
        }
        None => {
            create_decision(
                &state.pg,
                &user.id,
                item,
                None,
                Some(action_id),
                agent_model,
                delegate_model,
                "proposed",
                Some(&proposal),
                None,
                Some(&token_hash(&token)),
                Some(&expires_iso),
            )
            .await?
        }
    };
    Ok(json!({
        "status": "confirmation_required",
        "decisionId": decision_id,
        "confirmationToken": token,
        "expiresAt": expires_iso,
        "preview": preview,
    }))
}

/// What the confirmation hand-back carries: the proposal's payload plus the
/// two actor facts stored beside it, so the executed decision is attributed
/// to the models that proposed it rather than to the click alone.
struct ConfirmedProposal {
    payload: Option<Value>,
    agent_model: Option<String>,
    delegate_model: Option<String>,
}

/// The one-way door. A token that matches flips the row to confirmed and
/// strips the cipher; anything else is a miss, and a miss that means the
/// source moved FAILS the pending confirmation so the card stops offering
/// it.
async fn consume_confirmation(
    pg: &PgPool,
    user: &SessionUser,
    decision_id: &str,
    token: &str,
    item: &RawFocusItem,
    action_id: &str,
) -> Result<Option<ConfirmedProposal>, sqlx::Error> {
    let confirmed: Option<(Value, Option<String>, Option<String>)> = sqlx::query_as(
        "update inbox_decisions set status = 'confirmed', confirmed_at = now(), \
            confirmation_token_hash = null, proposal = proposal - 'confirmationCipher' \
         where id = $1::uuid and user_id = $2::uuid and source_type = $3 \
           and source_id = $4 and action_id = $5 and status = 'proposed' \
           and expires_at > now() and confirmation_token_hash = $6 \
           and proposal->>'sourceFingerprint' = $7 \
         returning proposal, agent_model, delegate_model",
    )
    .bind(decision_id)
    .bind(&user.id)
    .bind(&item.source_type)
    .bind(&item.source_id)
    .bind(action_id)
    .bind(token_hash(token))
    .bind(&item.source_fingerprint)
    .fetch_optional(pg)
    .await?;
    let Some((proposal, agent_model, delegate_model)) = confirmed.filter(|(p, _, _)| p.is_object())
    else {
        // The miss: read the row back to decide whether it merely answered
        // null (retryable) or means the confirmation is dead.
        let pending: Option<(Option<String>, Option<i64>, Option<String>)> = sqlx::query_as(
            "select status, (trunc(extract(epoch from expires_at) * 1000))::bigint, \
                    proposal->>'sourceFingerprint' \
             from inbox_decisions where id = $1::uuid and user_id = $2::uuid limit 1",
        )
        .bind(decision_id)
        .bind(&user.id)
        .fetch_optional(pg)
        .await?;
        let (status, expires_ms, stored_fingerprint) = match pending {
            Some((status, expires_ms, stored)) => (status, expires_ms, stored),
            None => (None, None, None),
        };
        let expires_iso = expires_ms.map(crate::agent_auth::epoch_ms_to_iso);
        if confirmation_miss_invalidates(
            status.as_deref(),
            expires_iso.as_deref(),
            stored_fingerprint.as_deref(),
            &item.source_fingerprint,
            now_ms(),
        ) {
            sqlx::query(
                "update inbox_decisions set status = 'failed', \
                    outcome = $3::jsonb, confirmation_token_hash = null, \
                    proposal = proposal - 'confirmationCipher', completed_at = now() \
                 where id = $1::uuid and user_id = $2::uuid and status = 'proposed'",
            )
            .bind(decision_id)
            .bind(&user.id)
            .bind(json!({ "error": "Confirmation expired or source changed." }))
            .execute(pg)
            .await?;
        }
        return Ok(None);
    };
    Ok(Some(ConfirmedProposal {
        payload: proposal.get("payload").cloned(),
        agent_model,
        delegate_model,
    }))
}

/// A page reload re-asks for a token it already earned. Validity is
/// re-derived from the stored row; a recoverable cipher hands back the SAME
/// token (so an old tab and a new tab confirm the same capability), anything
/// else issues a fresh one.
pub async fn reissue_focus_confirmation(
    state: &AppState,
    user: &SessionUser,
    decision_id: &str,
) -> Result<Value, FocusError> {
    let pg = &state.pg;
    #[allow(clippy::type_complexity)] // the pending proposal's own columns, one each
    let row: Option<(String, String, Option<String>, Value, Option<String>, Option<i64>)> =
        sqlx::query_as(
            "select source_type, source_id, action_id, proposal, \
                    confirmation_token_hash, (trunc(extract(epoch from expires_at) * 1000))::bigint \
             from inbox_decisions \
             where id = $1::uuid and user_id = $2::uuid and status = 'proposed' \
               and proposal ? 'preview' \
             limit 1",
        )
        .bind(decision_id)
        .bind(&user.id)
        .fetch_optional(pg)
        .await?;
    let Some((source_type, source_id, action_id, proposal, token_hash_row, expires_ms)) =
        row.filter(|(_, _, _, proposal, _, _)| proposal.is_object())
    else {
        return Ok(json!({
            "status": "stale",
            "message": "That confirmation is no longer pending.",
        }));
    };

    let item = find_focus_item_for_user(pg, user, &format!("{source_type}:{source_id}")).await?;
    let action = item.as_ref().and_then(|item| {
        item.actions
            .iter()
            .find(|a| Some(a.id.as_str()) == action_id.as_deref())
            .cloned()
    });
    let approval = match &item {
        Some(item) if item.source_type == "approval" => {
            pending_approval_for(pg, user, &item.source_id).await?
        }
        _ => None,
    };
    // The SAME question the action runner asked when it issued this token,
    // asked again from the stored row: a pending confirmation is valid when
    // the action needs one, or when the source that proposed it does. Reading
    // `action.confirmationRequired` alone here would fail every widened
    // sign-off the moment the owner reloaded the Inbox.
    let valid = item
        .as_ref()
        .zip(action.as_ref())
        .is_some_and(|(item, action)| {
            requires_human_confirmation(action, proposal_source_of(&proposal))
                && proposal.get("sourceFingerprint") == Some(&json!(item.source_fingerprint))
                && (item.source_type != "approval" || approval.is_some())
        });
    if !valid {
        sqlx::query(
            "update inbox_decisions set status = 'failed', \
                outcome = $3::jsonb, confirmation_token_hash = null, \
                proposal = proposal - 'confirmationCipher', completed_at = now() \
             where id = $1::uuid and user_id = $2::uuid and status = 'proposed'",
        )
        .bind(decision_id)
        .bind(&user.id)
        .bind(json!({ "error": "Source state or permission changed before confirmation." }))
        .execute(pg)
        .await?;
        return Ok(json!({
            "status": "stale",
            "decisionId": decision_id,
            "message": "That source changed or is no longer available to you.",
        }));
    }

    let item = item.expect("valid implies the item");
    let payload = proposal.get("payload").unwrap_or(&Value::Null);
    let preview = exact_preview(
        &item,
        action_id.as_deref().unwrap_or(""),
        payload,
        approval.as_ref(),
    );
    let cipher = proposal.get("confirmationCipher").and_then(Value::as_str);
    if let (Some(cipher), Some(hash), Some(expires_ms)) =
        (cipher, token_hash_row.as_deref(), expires_ms)
        && expires_ms > now_ms()
    {
        // The encrypted capability may still be recoverable; a failure here
        // falls through to the fresh token below.
        if let Ok(token) = state
            .secretbox()
            .await
            .and_then(|sb| sb.open(cipher).map_err(|e| e.to_string()))
            && token_hash(&token) == hash
        {
            return Ok(json!({
                "status": "confirmation_required",
                "decisionId": decision_id,
                "confirmationToken": token,
                "expiresAt": crate::agent_auth::epoch_ms_to_iso(expires_ms),
                "preview": preview,
            }));
        }
    }

    let token = random_token_24();
    let expires_iso = crate::agent_auth::epoch_ms_to_iso(now_ms() + CONFIRMATION_MS);
    let cipher = state
        .secretbox()
        .await
        .and_then(|sb| sb.seal(&token).map_err(|e| e.to_string()))
        .map_err(FocusError::Throw)?;
    // `{...proposal, preview, confirmationCipher: seal(token)}` — the stored
    // order with the two keys overwritten in place (or appended the first
    // time).
    let mut updated = proposal.clone();
    if let Some(object) = updated.as_object_mut() {
        object.insert("preview".into(), preview.clone());
        object.insert("confirmationCipher".into(), json!(cipher));
    }
    let updated_row = sqlx::query_scalar::<_, String>(
        "update inbox_decisions \
         set proposal = $3, confirmation_token_hash = $4, expires_at = $5::timestamptz \
         where id = $1::uuid and user_id = $2::uuid and status = 'proposed' \
         returning id",
    )
    .bind(decision_id)
    .bind(&user.id)
    .bind(&updated)
    .bind(token_hash(&token))
    .bind(&expires_iso)
    .fetch_optional(pg)
    .await?;
    if updated_row.is_none() {
        return Ok(json!({
            "status": "stale",
            "decisionId": decision_id,
            "message": "That confirmation is no longer pending.",
        }));
    }
    Ok(json!({
        "status": "confirmation_required",
        "decisionId": decision_id,
        "confirmationToken": token,
        "expiresAt": expires_iso,
        "preview": preview,
    }))
}

/// What the command-proposal read hands back: the proposal the assistant's
/// command left on the row, plus WHO proposed it — read from the row the
/// server wrote, never from the request. A client that could name its own
/// provenance could name itself deterministic and skip the click, which is
/// the whole gate.
struct CommandProposalRow {
    payload: Option<Value>,
    agent_model: Option<String>,
    delegate_model: Option<String>,
    source: FocusProposalSource,
}

async fn command_decision(
    pg: &PgPool,
    user: &SessionUser,
    decision_id: &str,
    item: &RawFocusItem,
    action_id: &str,
) -> Result<Option<CommandProposalRow>, sqlx::Error> {
    let row: Option<(Value, Option<String>, Option<String>)> = sqlx::query_as(
        "select proposal, agent_model, delegate_model \
         from inbox_decisions \
         where id = $1::uuid and user_id = $2::uuid and source_type = $3 \
           and source_id = $4 and action_id = $5 and status = 'proposed' \
           and instruction is not null and confirmation_token_hash is null \
           and proposal->>'sourceFingerprint' = $6 \
         limit 1",
    )
    .bind(decision_id)
    .bind(&user.id)
    .bind(&item.source_type)
    .bind(&item.source_id)
    .bind(action_id)
    .bind(&item.source_fingerprint)
    .fetch_optional(pg)
    .await?;
    let Some((proposal, agent_model, delegate_model)) = row.filter(|(p, _, _)| p.is_object())
    else {
        return Ok(None);
    };
    Ok(Some(CommandProposalRow {
        payload: proposal.get("payload").cloned(),
        agent_model,
        delegate_model,
        source: proposal_source_of(&proposal),
    }))
}

/// An idempotency answer for a decision already made: completed (with its
/// undo window), failed (with its recorded error), or a confirmed row whose
/// outcome never landed (the stale sentence). A decision still in flight
/// answers null — the caller proceeds as a first run.
async fn replay_decision(
    pg: &PgPool,
    user: &SessionUser,
    decision_id: &str,
    key: &str,
    action_id: &str,
) -> Result<Option<Value>, sqlx::Error> {
    let separator = key.find(':').unwrap_or(0);
    if separator == 0 {
        return Ok(None);
    }
    let row: Option<(String, Option<Value>, Option<i64>)> = sqlx::query_as(
        "select status, outcome, (trunc(extract(epoch from completed_at) * 1000))::bigint \
         from inbox_decisions \
         where id = $1::uuid and user_id = $2::uuid \
           and source_type = $3 and source_id = $4 and action_id = $5 \
         limit 1",
    )
    .bind(decision_id)
    .bind(&user.id)
    .bind(&key[..separator])
    .bind(&key[separator + 1..])
    .bind(action_id)
    .fetch_optional(pg)
    .await?;
    let Some((status, outcome, completed_ms)) = row else {
        return Ok(None);
    };
    match status.as_str() {
        "completed" => {
            let mut result = json!({
                "status": "completed",
                "decisionId": decision_id,
                "result": outcome.unwrap_or(Value::Null),
            });
            if action_id == "mark_read" && completed_ms.unwrap_or(0) > now_ms() - UNDO_MS {
                result["undo"] = json!({
                    "decisionId": decision_id,
                    "expiresAt": crate::agent_auth::epoch_ms_to_iso(
                        completed_ms.unwrap_or(0) + UNDO_MS
                    ),
                });
            }
            Ok(Some(result))
        }
        "failed" => {
            let error = outcome
                .as_ref()
                .and_then(|o| o.get("error"))
                .map(value_to_string)
                .unwrap_or_else(|| "That action failed.".to_string());
            Ok(Some(json!({
                "status": "failed",
                "decisionId": decision_id,
                "message": error,
            })))
        }
        "confirmed" => Ok(Some(json!({
            "status": "stale",
            "decisionId": decision_id,
            "message": "The confirmation was accepted, but its final outcome could not be verified. Open the source before trying again.",
        }))),
        _ => Ok(None),
    }
}

/// `String(outcome?.error ?? …)` — a non-string error becomes its JS string
/// coercion; for the JSON types that reach here, numbers and booleans
/// stringify and objects read "[object Object]".
fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => "[object Object]".to_string(),
    }
}

async fn complete_decision(
    pg: &PgPool,
    decision_id: &str,
    status: &str,
    outcome: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update inbox_decisions set status = $2, outcome = $3, completed_at = now(), \
            confirmation_token_hash = null, proposal = proposal - 'confirmationCipher' \
         where id = $1::uuid",
    )
    .bind(decision_id)
    .bind(status)
    .bind(outcome)
    .execute(pg)
    .await?;
    Ok(())
}

/// A command proposal that cannot proceed fails ON ITS OWN ROW (the
/// assistant proposed it; the card should say so), and the decisionId rides
/// along only when a row was actually updated.
async fn fail_command_proposal(
    pg: &PgPool,
    user: &SessionUser,
    decision_id: Option<&str>,
    key: &str,
    action_id: &str,
    status: &str,
    message: &str,
) -> Result<Value, sqlx::Error> {
    let Some(decision_id) = decision_id else {
        return Ok(json!({ "status": status, "message": message }));
    };
    let separator = key.find(':').unwrap_or(0);
    if separator == 0 {
        return Ok(json!({ "status": status, "message": message }));
    }
    let updated = sqlx::query_scalar::<_, String>(
        "update inbox_decisions \
         set status = 'failed', outcome = $6::jsonb, completed_at = now() \
         where id = $1::uuid and user_id = $2::uuid \
           and source_type = $3 and source_id = $4 and action_id = $5 \
           and status = 'proposed' and instruction is not null \
         returning id",
    )
    .bind(decision_id)
    .bind(&user.id)
    .bind(&key[..separator])
    .bind(&key[separator + 1..])
    .bind(action_id)
    .bind(json!({ "error": message, "kind": status }))
    .fetch_optional(pg)
    .await?;
    Ok(if updated.is_some() {
        json!({ "status": status, "message": message, "decisionId": decision_id })
    } else {
        json!({ "status": status, "message": message })
    })
}

/// The five allowlisted mutations. Everything past the gate lands here, and
/// the audit row names the session user as the actor, which is only honest
/// because the user clicked. Whatever failed, the catch arm records it on
/// the decision row and the card reads the message — including a database
/// failure (its message is what the card reads).
#[allow(clippy::too_many_arguments)]
async fn execute_action(
    state: &AppState,
    user: &SessionUser,
    item: &RawFocusItem,
    action_id: &str,
    payload: &Value,
    existing_decision_id: Option<&str>,
    agent_model: Option<&str>,
    delegate_model: Option<&str>,
) -> Result<Value, FocusError> {
    match execute_action_arms(
        state,
        user,
        item,
        action_id,
        payload,
        existing_decision_id,
        agent_model,
        delegate_model,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => {
            let message = match &error {
                FocusError::Db(e) => e.to_string(),
                FocusError::Throw(m) => m.clone(),
            };
            let decision_id = match existing_decision_id {
                Some(existing) => {
                    complete_decision(&state.pg, existing, "failed", &json!({ "error": message }))
                        .await?;
                    existing.to_string()
                }
                None => {
                    create_decision(
                        &state.pg,
                        &user.id,
                        item,
                        None,
                        Some(action_id),
                        agent_model,
                        delegate_model,
                        "failed",
                        None,
                        Some(&json!({ "error": message })),
                        None,
                        None,
                    )
                    .await?
                }
            };
            Ok(json!({ "status": "failed", "decisionId": decision_id, "message": message }))
        }
    }
}

/// The shared early exit: the proposal fails on its own row (when there is
/// one) and the caller reads status + message.
async fn finish_early(
    pg: &PgPool,
    existing_decision_id: Option<&str>,
    status: &str,
    message: &str,
) -> Result<Value, FocusError> {
    if let Some(existing) = existing_decision_id {
        complete_decision(
            pg,
            existing,
            "failed",
            &json!({ "error": message, "kind": status }),
        )
        .await?;
    }
    Ok(match existing_decision_id {
        Some(existing) => json!({ "status": status, "message": message, "decisionId": existing }),
        None => json!({ "status": status, "message": message }),
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_action_arms(
    state: &AppState,
    user: &SessionUser,
    item: &RawFocusItem,
    action_id: &str,
    payload: &Value,
    existing_decision_id: Option<&str>,
    agent_model: Option<&str>,
    delegate_model: Option<&str>,
) -> Result<Value, FocusError> {
    let pg = &state.pg;
    let actor = actor_of(user);
    let outcome: Value;

    if item.source_type == "notification" && action_id == "mark_read" {
        let after: Option<i64> = sqlx::query_scalar(
            "update notifications set read_at = now() \
             where id = $1::uuid and user_id = $2::uuid and read_at is null \
             returning (trunc(extract(epoch from read_at) * 1000))::bigint",
        )
        .bind(&item.source_id)
        .bind(&user.id)
        .fetch_optional(pg)
        .await?;
        let Some(after) = after else {
            return finish_early(
                pg,
                existing_decision_id,
                "stale",
                "That notification is already resolved.",
            )
            .await;
        };
        outcome = json!({
            "beforeReadAt": null,
            "afterReadAt": crate::agent_auth::epoch_ms_to_iso(after),
        });
    } else if item.source_type == "channel" && action_id == "mark_read" {
        let cursors: Option<(i32, i32)> = sqlx::query_as(
            "select member.last_read_seq, c.msg_seq \
             from channel_members member join channels c on c.id = member.channel_id \
             where member.channel_id = $1::uuid and member.user_id = $2::uuid \
               and c.archived_at is null",
        )
        .bind(&item.source_id)
        .bind(&user.id)
        .fetch_optional(pg)
        .await?;
        let Some((before, after)) = cursors else {
            return finish_early(
                pg,
                existing_decision_id,
                "stale",
                "That conversation is no longer available.",
            )
            .await;
        };
        mark_channel_read(pg, &item.source_id, &user.id, after).await?;
        outcome = json!({ "beforeCursor": before, "afterCursor": after });
    } else if item.source_type == "channel" && action_id == "reply" {
        let message = crate::body::truncate_utf16(
            payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim(),
            20_000,
        )
        .to_string();
        if message.is_empty() {
            return finish_early(pg, existing_decision_id, "failed", "The reply is empty.").await;
        }
        if channel_role(pg, &user.id, &item.source_id).await?.is_none() {
            return finish_early(
                pg,
                existing_decision_id,
                "stale",
                "You no longer have access to that conversation.",
            )
            .await;
        }
        let author = user
            .email
            .clone()
            .or_else(|| user.name.clone())
            .unwrap_or_else(|| "user".to_string());
        let deps = NotifyDeps::publishing(pg.clone(), state.redis().await.ok());
        let posted = insert_channel_message(
            &deps,
            &item.source_id,
            "user",
            &author,
            &message,
            "complete",
            &json!([]),
            None,
        )
        .await?;
        mark_channel_read(pg, &item.source_id, &user.id, posted.seq).await?;
        let channel: Option<(String, String)> =
            sqlx::query_as("select name, kind from channels where id = $1::uuid")
                .bind(&item.source_id)
                .fetch_optional(pg)
                .await?;
        // Detached fan-out: the reply is posted; the notifications and agent
        // triggers it causes are never this response's problem.
        if let Some((channel_name, channel_kind)) = channel {
            let label = user.name.clone().unwrap_or_else(|| author.clone());
            if channel_kind == "dm" {
                let deps = deps.clone();
                let (channel_id, user_id) = (item.source_id.clone(), user.id.clone());
                let message = message.clone();
                tokio::spawn(async move {
                    notify_dm_message(&deps, &channel_id, &user_id, &label, &message).await;
                });
            } else {
                let deps = deps.clone();
                let (channel_id, user_id) = (item.source_id.clone(), user.id.clone());
                let channel_name = channel_name.clone();
                let message = message.clone();
                tokio::spawn(async move {
                    notify_user_mentions(
                        &deps,
                        &channel_id,
                        &channel_name,
                        &user_id,
                        &label,
                        &message,
                    )
                    .await;
                });
            }
            let deps = deps.clone();
            let sb = state.secretbox().await.ok();
            let trigger_state = state.clone();
            let trigger_seq = posted.seq;
            let (channel_id, channel_name, message) =
                (item.source_id.clone(), channel_name, message);
            tokio::spawn(async move {
                if let Some(sb) = sb {
                    trigger_agent_replies(
                        &trigger_state,
                        &deps,
                        &sb,
                        &channel_id,
                        &channel_name,
                        &message,
                        0,
                        trigger_seq,
                        None,
                    )
                    .await;
                }
            });
        }
        outcome = json!({
            "messageId": posted.id,
            "seq": posted.seq,
            "href": item.source_href,
        });
    } else if item.source_type == "task"
        && (action_id == "approve_task" || action_id == "request_changes")
    {
        let task = match get_task(pg, &item.source_id).await? {
            Some(task) => task,
            None => {
                return finish_early(
                    pg,
                    existing_decision_id,
                    "stale",
                    "The task is unavailable or your access changed.",
                )
                .await;
            }
        };
        // Short-circuit: `!task || !canEdit(boardRole(...))` — the board
        // lookup only runs when the task exists.
        if !can_edit(board_role(pg, &user.id, &task.board_id).await?.as_deref()) {
            return finish_early(
                pg,
                existing_decision_id,
                "stale",
                "The task is unavailable or your access changed.",
            )
            .await;
        }
        let meta = status_meta(pg, &task.board_id).await?;
        if !meta.review_keys.contains(&task.status) {
            return finish_early(
                pg,
                existing_decision_id,
                "stale",
                "That task is no longer awaiting review.",
            )
            .await;
        }
        let approved = action_id == "approve_task";
        let next_status = if approved {
            meta.done_keys.first().cloned()
        } else if meta.keys.iter().any(|k| k == "in_progress") {
            Some("in_progress".to_string())
        } else {
            meta.assigned_key.clone()
        };
        if !matches!(next_status.as_deref(), Some(s) if meta.keys.iter().any(|k| k == s)) {
            return finish_early(
                pg,
                existing_decision_id,
                "failed",
                "This board does not have a valid destination status for the review.",
            )
            .await;
        }
        let task_deps = TaskDeps::from_route(pg.clone(), state.redis().await.ok());
        let updated = match complete_quality_review(
            &task_deps,
            &task.id,
            &actor,
            if approved { "approved" } else { "rejected" },
            next_status.as_deref().expect("checked above"),
        )
        .await
        {
            Ok(updated) => updated,
            Err(TaskError::Refusal(m)) | Err(TaskError::ApprovalRequired(m)) => {
                return Err(FocusError::Throw(m));
            }
            Err(TaskError::Db(e)) => return Err(FocusError::Db(e)),
        };
        let Some(updated) = updated else {
            return finish_early(
                pg,
                existing_decision_id,
                "stale",
                "That task changed before the review could be completed.",
            )
            .await;
        };
        outcome = json!({ "taskId": task.id, "status": updated.status });
    } else if item.source_type == "approval" && (action_id == "approve" || action_id == "reject") {
        let decision = if action_id == "approve" {
            "approve"
        } else {
            "reject"
        };
        let sb = state.secretbox().await.map_err(FocusError::Throw)?;
        let result = match decide_action(
            pg,
            &sb,
            &item.source_id,
            &user.id,
            user.role == "admin",
            decision,
            now_ms(),
        )
        .await
        {
            Ok(result) => result,
            Err(m) => return Err(FocusError::Throw(m)),
        };
        match result {
            None => {
                return finish_early(
                    pg,
                    existing_decision_id,
                    "stale",
                    "That approval is no longer available.",
                )
                .await;
            }
            Some(r) if matches!(r.status.as_str(), "forbidden" | "pending") => {
                return finish_early(
                    pg,
                    existing_decision_id,
                    "stale",
                    "That approval is no longer available.",
                )
                .await;
            }
            Some(r) if matches!(r.status.as_str(), "not_connected" | "failed") => {
                return Err(FocusError::Throw(r.message.unwrap_or(r.status)));
            }
            // The whole decision result is the outcome — {status:'executed'},
            // {status:'rejected'}, or the already-decided status it answered with.
            Some(r) => outcome = json!({ "status": r.status }),
        }
    } else {
        return finish_early(
            pg,
            existing_decision_id,
            "failed",
            "That action is not available for this item.",
        )
        .await;
    }

    let mut decision_id = existing_decision_id.map(str::to_string);
    if decision_id.is_none() {
        decision_id = Some(
            create_decision(
                pg,
                &user.id,
                item,
                None,
                Some(action_id),
                agent_model,
                delegate_model,
                "completed",
                None,
                Some(&outcome),
                None,
                None,
            )
            .await?,
        );
    }
    if let Some(existing) = existing_decision_id {
        complete_decision(pg, existing, "completed", &outcome).await?;
    }
    log_audit(
        pg,
        AuditEntry {
            actor: &actor,
            action: &format!("inbox.focus.{action_id}"),
            target_type: &item.source_type,
            target_id: Some(&item.source_id),
            target_label: Some(&item.question),
            before: None,
            after: Some(json!({
                "decisionId": decision_id,
                "agentModel": agent_model,
                "delegateModel": delegate_model,
                "outcome": outcome,
            })),
        },
    )
    .await;
    let mut result = json!({
        "status": "completed",
        "decisionId": decision_id,
        "result": outcome,
    });
    if action_id == "mark_read" {
        result["undo"] = json!({
            "decisionId": decision_id,
            "expiresAt": crate::agent_auth::epoch_ms_to_iso(now_ms() + UNDO_MS),
        });
    }
    Ok(result)
}

/// The parsed body of an actions call — one shape for the whole family
/// (run, confirm, replay, cancel, undo).
#[derive(Default)]
pub struct FocusActionInput {
    pub key: Option<String>,
    pub action_id: Option<String>,
    pub payload: Option<Value>,
    pub command_decision_id: Option<String>,
    pub decision_id: Option<String>,
    pub confirmation_token: Option<String>,
    pub cancel_decision_id: Option<String>,
    pub undo_decision_id: Option<String>,
}

/// The one entry the card's every button funnels through. Cancel and undo
/// are their own verbs; everything else resolves the item, checks the action
/// against the allowlist, honors a command proposal or a confirmation token,
/// and then either asks for the click or executes.
pub async fn run_focus_action(
    state: &AppState,
    user: &SessionUser,
    input: &FocusActionInput,
) -> Result<Value, FocusError> {
    let pg = &state.pg;
    if let Some(cancel) = input.cancel_decision_id.as_deref() {
        return Ok(cancel_decision(pg, user, cancel).await?);
    }
    if let Some(undo) = input.undo_decision_id.as_deref() {
        return Ok(undo_decision(pg, user, undo).await?);
    }
    let (Some(key), Some(action_id)) = (input.key.as_deref(), input.action_id.as_deref()) else {
        return Ok(
            json!({ "status": "failed", "message": "A queue item and action are required." }),
        );
    };
    if input.decision_id.is_some()
        && input.confirmation_token.is_some()
        && let Some(replayed) = replay_decision(
            pg,
            user,
            input.decision_id.as_deref().expect("checked above"),
            key,
            action_id,
        )
        .await?
    {
        return Ok(replayed);
    }
    let Some(item) = find_focus_item_for_user(pg, user, key).await? else {
        return Ok(fail_command_proposal(
            pg,
            user,
            input.command_decision_id.as_deref(),
            key,
            action_id,
            "stale",
            "That item was already resolved or is no longer accessible.",
        )
        .await?);
    };
    let Some(action) = item.actions.iter().find(|a| a.id == action_id).cloned() else {
        return Ok(fail_command_proposal(
            pg,
            user,
            input.command_decision_id.as_deref(),
            key,
            action_id,
            "failed",
            "That action is not allowlisted for this item.",
        )
        .await?);
    };

    let approval = if item.source_type == "approval" {
        pending_approval_for(pg, user, &item.source_id).await?
    } else {
        None
    };
    if item.source_type == "approval" && approval.is_none() {
        return Ok(fail_command_proposal(
            pg,
            user,
            input.command_decision_id.as_deref(),
            key,
            action_id,
            "stale",
            "That approval is no longer pending.",
        )
        .await?);
    }
    let command = match input.command_decision_id.as_deref() {
        Some(command_decision_id) => {
            let proposal =
                command_decision(pg, user, command_decision_id, &item, action_id).await?;
            if proposal.is_none() {
                return Ok(fail_command_proposal(
                    pg,
                    user,
                    input.command_decision_id.as_deref(),
                    key,
                    action_id,
                    "stale",
                    "That command proposal changed or was already used.",
                )
                .await?);
            }
            proposal
        }
        None => None,
    };
    let payload = command
        .as_ref()
        .and_then(|c| c.payload.clone())
        .or_else(|| input.payload.clone())
        .unwrap_or(Value::Null);
    let (agent_model, delegate_model, source) = match &command {
        Some(command) => (
            command.agent_model.clone(),
            command.delegate_model.clone(),
            command.source,
        ),
        // No command proposal behind this call means a person clicked a
        // button on the card (or Retry on a past decision), and that click is
        // the confirmation.
        None => (None, None, FocusProposalSource::Human),
    };

    // A TOKEN ANSWERS A CONFIRMATION, whatever the action's own risk says.
    // This used to sit inside `if (action.confirmationRequired)`, which was
    // fine while tokens were only ever issued for actions that carried that
    // flag. They are now issued for a `risk: 'safe'` action a model proposed,
    // so the confirming click on one of those would have fallen through to a
    // second, unconsumed execution — leaving the pending row 'proposed'
    // forever and dropping the token's replay protection.
    if input.decision_id.is_some() && input.confirmation_token.is_some() {
        let confirmed = consume_confirmation(
            pg,
            user,
            input.decision_id.as_deref().expect("checked above"),
            input.confirmation_token.as_deref().expect("checked above"),
            &item,
            action_id,
        )
        .await?;
        return match confirmed {
            Some(confirmed) => {
                execute_action(
                    state,
                    user,
                    &item,
                    action_id,
                    &confirmed.payload.unwrap_or(Value::Null),
                    input.decision_id.as_deref(),
                    confirmed.agent_model.as_deref(),
                    confirmed.delegate_model.as_deref(),
                )
                .await
            }
            None => Ok(json!({
                "status": "stale",
                "message": "That confirmation expired or was already used.",
            })),
        };
    }

    // THE GATE (see `requires_human_confirmation`). The action's own risk, OR
    // the fact that a model rather than a regex chose it. Everything past
    // this line executes, and `execute_action` writes an audit row naming the
    // session user as the actor — which is only honest because the user
    // clicked.
    if requires_human_confirmation(&action, source) {
        return proposed_confirmation(
            state,
            user,
            &item,
            action_id,
            &payload,
            approval.as_ref(),
            agent_model.as_deref(),
            delegate_model.as_deref(),
            source,
            input.command_decision_id.as_deref(),
        )
        .await;
    }

    execute_action(
        state,
        user,
        &item,
        action_id,
        &payload,
        input.command_decision_id.as_deref(),
        agent_model.as_deref(),
        delegate_model.as_deref(),
    )
    .await
}

/// A delegate must be an enabled agent the owner (or the org) actually has,
/// and the tier must be one it declares; the routed id comes back for the
/// ledger.
async fn valid_delegate(
    pg: &PgPool,
    user_id: &str,
    requested: Option<&str>,
    tier: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let exists: Option<(i8,)> = sqlx::query_as(
        "select 1 from agent_defs where model = $1 and enabled \
         and (owner_user_id is null or owner_user_id = $2::uuid) limit 1",
    )
    .bind(requested)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    if exists.is_none() {
        return Ok(None);
    }
    routed_model_for(pg, requested, tier).await
}

/// The owner's picked gateway model, still allowed for their account or not.
/// This one THROWS, and the sentence is the product. Shared with the
/// conversation module (same rule for its response-model pick).
pub(crate) async fn valid_response_model(
    pg: &PgPool,
    user: &SessionUser,
    requested: Option<&str>,
) -> Result<Option<String>, FocusError> {
    // `if (!requested) return null` — the empty string is a no-pick, not a
    // throw: the route's max-only validation lets it through.
    let Some(requested) = requested.filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let allowed = gateway_models_for(pg, &user.role).await?;
    if !allowed.iter().any(|model| model.id == requested) {
        return Err(FocusError::Throw(
            "That response model is no longer available to this account.".into(),
        ));
    }
    Ok(Some(requested.to_string()))
}

/// The parsed body of a command call.
pub struct FocusCommandCall {
    pub key: String,
    pub instruction: String,
    pub delegate_model: Option<String>,
    pub delegate_tier: Option<String>,
    pub response_model: Option<String>,
    pub mode: Option<String>,
    /// The owner's reasoning-effort pick, validated by the caller against the
    /// answering model's supported levels. Rides the harness context to the
    /// transport; absent = the model's default.
    pub effort: Option<String>,
    pub attachment_context: Option<String>,
    pub history: Vec<OwnedTurn>,
}

/// `{kind:'proposal', ...deterministic}` — the deterministic proposal spread,
/// whose own key order is {actionId, message, payload?}.
fn deterministic_turn(d: &CommandProposal) -> CommandTurn {
    CommandTurn {
        kind: "proposal",
        message: d.message.clone(),
        action_id: Some(d.action_id.clone()),
        payload: d.payload.clone(),
    }
}

/// The model-arm wire literal: `{kind, message, actionId?, payload?}` —
/// conditional keys absent when absent.
fn turn_model_json(turn: &CommandTurn) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("kind".into(), json!(turn.kind));
    object.insert("message".into(), json!(turn.message));
    if let Some(action_id) = &turn.action_id {
        object.insert("actionId".into(), json!(action_id));
    }
    if let Some(payload) = &turn.payload {
        object.insert("payload".into(), payload.clone());
    }
    Value::Object(object)
}

/// The deterministic-arm wire literal: `{kind:'proposal', actionId, message,
/// payload?}` — the spread of `CommandProposal` after the kind.
fn turn_proposal_json(turn: &CommandTurn) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("kind".into(), json!("proposal"));
    if let Some(action_id) = &turn.action_id {
        object.insert("actionId".into(), json!(action_id));
    }
    object.insert("message".into(), json!(turn.message));
    if let Some(payload) = &turn.payload {
        object.insert("payload".into(), payload.clone());
    }
    Value::Object(object)
}

/// The two-seat command turn: a bounded specialist consultation on a
/// delegate the owner pointed at this item, then their own assistant
/// orchestrating the answer. Falls through to the specialist, then
/// the deterministic match, then a clarification — each step gated against
/// ITS OWN allowlist, and every union of two seats' judgement still needs the
/// human click that `proposal_source_for` records.
pub async fn run_focus_command(
    state: &AppState,
    user: &SessionUser,
    input: &FocusCommandCall,
) -> Result<Option<Value>, FocusError> {
    let pg = &state.pg;
    let Some(item) = find_focus_item_for_user(pg, user, &input.key).await? else {
        return Ok(None);
    };
    let assistant = focus_assistant_for(pg, &user.id).await?;
    let delegate_model = valid_delegate(
        pg,
        &user.id,
        input.delegate_model.as_deref(),
        input.delegate_tier.as_deref(),
    )
    .await?;
    let response_model = valid_response_model(pg, user, input.response_model.as_deref()).await?;
    let mode = match input.mode.as_deref() {
        Some("fast") => FocusCommandMode::Fast,
        Some("plan") => FocusCommandMode::Plan,
        _ => FocusCommandMode::Normal,
    };
    let deterministic = if mode == FocusCommandMode::Plan {
        None
    } else {
        deterministic_proposal(&item, &input.instruction)
    };
    let caller = format!(
        "user:{}",
        user.email.clone().unwrap_or_else(|| user.id.clone())
    );
    // Everything both seats share. `role` and `specialist` are the only
    // fields that differ between the bounded specialist consultation and the
    // owner's own assistant orchestrating the answer, so the prompt for each
    // is a branch inside `inboxCommandHarness.render` rather than two
    // prompts out here.
    let shared_input = FocusCommandInput {
        item: harness_item_of(&item),
        instruction: format!(
            "{}{}",
            input.instruction,
            input.attachment_context.as_deref().unwrap_or("")
        ),
        history: input.history.clone(),
        mode,
        deterministic_action_id: deterministic.as_ref().map(|d| d.action_id.clone()),
        role: FocusSeatRole::Specialist,
        specialist: Value::Null,
    };

    let mut specialist_turn = None;
    if delegate_model.is_some() && mode != FocusCommandMode::Fast {
        let delegate = delegate_model.clone().expect("checked above");
        specialist_turn = request_focus_command(state, &delegate, &shared_input, &caller, None)
            .await
            .map_err(FocusError::Throw)?;
    }

    let mut turn = None;
    let mut from_deterministic = false;
    if mode == FocusCommandMode::Fast
        && let Some(deterministic) = &deterministic
    {
        turn = Some(deterministic_turn(deterministic));
        from_deterministic = true;
    } else if response_model.is_some() || (assistant.configured && assistant.model.is_some()) {
        // One call whichever model won. Which transport carries it is the
        // runner's decision, and both get the same schema, the same
        // temperature and the same repair turn.
        let model = response_model
            .clone()
            .or_else(|| assistant.model.clone())
            .expect("one of the two is set");
        let orchestrator_input = FocusCommandInput {
            specialist: match &specialist_turn {
                Some(specialist) => turn_model_json(specialist),
                None => Value::Null,
            },
            role: FocusSeatRole::Orchestrator,
            ..shared_input.clone()
        };
        turn = request_focus_command(
            state,
            &model,
            &orchestrator_input,
            &caller,
            input.effort.as_deref(),
        )
        .await
        .map_err(FocusError::Throw)?;
    }

    // The specialist's proposal can still carry the turn when the orchestrator
    // produced nothing. Each seat was gated against ITS OWN allowlist inside
    // `request_focus_command`, and both allowlists are subsets of this item's
    // own actions, so falling through here can never widen authority. WHAT THE
    // AUTHORITY ARGUMENT DOES NOT COVER: a fall-through UNIONS two seats'
    // judgement, and the delegate is a second model the owner pointed at this
    // item rather than their own assistant answering a deterministic
    // instruction. So a delegate proposal is flagged `fromDelegate` and needs
    // the same human click a widened one does.
    if turn.is_none() && specialist_turn.is_some() {
        turn = specialist_turn.clone();
    }
    let turn = turn.unwrap_or_else(|| match &deterministic {
        Some(deterministic) => {
            from_deterministic = true;
            deterministic_turn(deterministic)
        }
        None => CommandTurn {
            kind: "clarification",
            message: if assistant.configured {
                "I could not safely map that instruction to an available action. Please clarify the intended outcome."
            } else {
                "Your personal assistant is not configured. You can still use the safe actions on this card or open the source."
            }
            .to_string(),
            action_id: None,
            payload: None,
        },
    });
    let action_id = turn.action_id.clone();

    let proposal = if action_id.is_some() {
        Some(json!({
            "message": turn.message,
            "payload": turn.payload.clone().unwrap_or(Value::Null),
            // `source` is written HERE, on the server, next to the
            // fingerprint that already pins this proposal to the item it was
            // made about. It is what the action runner reads back to decide
            // whether this proposal may run without a click, and writing it
            // anywhere the client can reach would make the gate advisory.
            "source": proposal_source_for(
                turn_source_is_delegate(&turn, &specialist_turn),
                action_id.as_deref().unwrap_or_default(),
                deterministic.as_ref().map(|d| d.action_id.as_str()),
            )
            .as_str(),
            "sourceFingerprint": item.source_fingerprint,
        }))
    } else {
        None
    };
    let outcome = (action_id.is_none()).then(|| json!({ "kind": "clarification" }));
    let decision_id = create_decision(
        pg,
        &user.id,
        &item,
        // The instruction column stores what the OWNER typed, not the
        // attachment-augmented prompt the models saw.
        Some(&input.instruction),
        action_id.as_deref(),
        response_model.as_deref().or(assistant.model.as_deref()),
        delegate_model.as_deref(),
        if action_id.is_some() {
            "proposed"
        } else {
            "completed"
        },
        proposal.as_ref(),
        outcome.as_ref(),
        None,
        None,
    )
    .await?;

    // `{...response, decisionId, assistant, consultedModel: delegateModel}` —
    // the response's OWN key order leads (the deterministic arm spells
    // actionId before message; every model arm spells kind, message), then
    // the three engine facts.
    let mut result = if from_deterministic {
        turn_proposal_json(&turn)
    } else {
        turn_model_json(&turn)
    };
    let object = result.as_object_mut().expect("turn json is an object");
    object.insert("decisionId".into(), json!(decision_id));
    object.insert(
        "assistant".into(),
        serde_json::to_value(&assistant).expect("assistant serializes"),
    );
    object.insert("consultedModel".into(), json!(delegate_model));
    Ok(Some(result))
}

/// True only when the turn that carried this proposal came off the
/// specialist seat's fall-through (the one seat fact the proposal itself
/// cannot record).
fn turn_source_is_delegate(turn: &CommandTurn, specialist_turn: &Option<CommandTurn>) -> bool {
    specialist_turn.is_some() && Some(turn) == specialist_turn.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_tokens_are_24_bytes_of_urlsafe_base64() {
        let token = random_token_24();
        assert_eq!(token.len(), 32); // 24 bytes → 32 base64url chars, no pad
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(token, random_token_24());
    }

    #[test]
    fn turn_json_orders_are_pinned() {
        let model = CommandTurn {
            kind: "proposal",
            message: "m".into(),
            action_id: Some("approve".into()),
            payload: Some(json!({ "x": 1 })),
        };
        assert_eq!(
            turn_model_json(&model).to_string(),
            r#"{"kind":"proposal","message":"m","actionId":"approve","payload":{"x":1}}"#
        );
        let deterministic = deterministic_turn(&CommandProposal {
            action_id: "approve".into(),
            message: "m".into(),
            payload: None,
        });
        assert_eq!(
            turn_proposal_json(&deterministic).to_string(),
            r#"{"kind":"proposal","actionId":"approve","message":"m"}"#
        );
    }

    #[test]
    fn undo_numbers_coerce_like_js_number() {
        assert_eq!(number_of(Some(&json!(3))), 3);
        assert_eq!(number_of(Some(&json!("3"))), 3);
        assert_eq!(number_of(Some(&json!("x"))), 0);
        assert_eq!(number_of(None), 0);
    }
}
