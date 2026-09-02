// inbox-focus-conversation.ts — the Inbox panel's segmented conversations:
// the chat picker, the paginated message+decision timeline, the snooze
// record, the process-local Inbox lock, and the SSE command run that threads
// one instruction through the assistant with events as they happen.
//
// THE PROCESS-LOCAL LOCK: one Inbox assistant action per user at a time,
// process-wide. The queue's decisions are idempotent where they can be, but
// the command run persists message rows in sequence and streams deltas — a
// second concurrent run interleaving seqs and events is incoherent, not
// merely racy. The guard is held across a whole handler (and, for the SSE
// command, across the stream's lifetime — it MOVES into the stream task, so
// the lock releases when the stream does, the TS `finally { release() }`).
// Process-local is the whole guarantee in TS and stays so here: two API
// processes each allow one. The proxy coexistence keeps one live runtime per
// group, so nothing is weakened by the port.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::agent_auth::epoch_ms_to_iso;
use crate::conversations::{
    insert_streaming_assistant, insert_user_message, next_seq, touch_conversation, update_assistant,
};
use crate::gateway::upstream::js_truthy;
use crate::harness::defs::inbox_focus::{FocusReplyInput, OwnedTurn, limit_inbox_model_history};
use crate::inbox_focus::timeline::{
    TimelineDecisionRecord, TimelineRecord, build_inbox_timeline, decode_inbox_timeline_cursor,
    encode_inbox_timeline_cursor, focus_from_metadata,
};
use crate::inbox_focus::types::{
    FocusAssistant, InboxCommandEvent, InboxConversationPage, InboxTimelineEntry, MessageEntry,
    RawFocusItem,
};
use crate::inbox_focus::{
    FocusCommandCall, FocusError, find_focus_item_for_user, focus_assistant_for, now_iso,
    reissue_focus_confirmation, run_focus_command, stream_reply, valid_response_model,
};
use crate::model::efforts::efforts_for_model;
use crate::persona::persona_configured_effort;
use crate::refs::{MessageRef, RefUser, ref_blocks, resolve_refs};
use crate::session::{SessionUser, actor_of};
use crate::state::AppState;
use crate::uploads::{
    UploadViewer, attachment_text_blocks, can_access_upload, resolve_attachments,
};

const PAGE_SIZE: usize = 30;
const UNDO_MS: i64 = 30_000;
// `left()` takes int4 — the bind is i64 elsewhere in the file, so the cast
// lives in the SQL ($2::int), not the bind site.
const CONVERSATION_PREVIEW_CHARS: i64 = 80;

static INBOX_FOCUS_LOCKS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// The lock's guard — dropping it is the release, so `let _guard = …` at the
/// top of a handler (or a move into a stream task) holds it for exactly the
/// handler's (or stream's) lifetime: the TS `try/finally` without the finally.
pub struct FocusLockGuard {
    user_id: String,
}

impl Drop for FocusLockGuard {
    fn drop(&mut self) {
        INBOX_FOCUS_LOCKS
            .lock()
            .expect("inbox focus lock set")
            .remove(&self.user_id);
    }
}

pub fn acquire_inbox_focus_lock(user_id: &str) -> Option<FocusLockGuard> {
    let mut locks = INBOX_FOCUS_LOCKS.lock().expect("inbox focus lock set");
    if locks.contains(user_id) {
        return None;
    }
    locks.insert(user_id.to_string());
    Some(FocusLockGuard {
        user_id: user_id.to_string(),
    })
}

/// `focusContext` — the five-key anchor every decision and message carries.
fn focus_context(item: &RawFocusItem) -> Value {
    json!({
        "key": item.key,
        "question": item.question,
        "sourceHref": item.source_href,
        "sourceType": item.source_type,
        "sourceId": item.source_id,
    })
}

// ── Conversation instances ───────────────────────────────────────────────────
//
// The panel's conversation is SEGMENTED: many instances per owner, picked from
// a dropdown, rather than one ever-growing thread. Segmentation IS the context
// strategy — a fresh instance is how old context is shed, chosen by the person
// rather than imposed by a budget. Everything here is scoped by ownership and
// `kind = 'inbox'` so an instance id from the dropdown can never address one of
// the owner's ordinary chat conversations.

/// One row of the panel's chat picker. The preview is the FIRST user message —
/// "what did I start this chat about" — not the last, which for a working
/// thread is usually a fragment of the task at hand.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxConversationSummary {
    id: String,
    created_at: String,
    updated_at: String,
    preview: String,
}

pub async fn list_inbox_conversations(
    pg: &PgPool,
    user_id: &str,
) -> Result<Vec<InboxConversationSummary>, sqlx::Error> {
    #[allow(clippy::type_complexity)] // the picker's own columns, one each
    let rows: Vec<(String, Option<String>, i64, i64)> = sqlx::query_as(
        "select c.id::text, \
                (select left(m.content, $2::int) from messages m \
                 where m.conversation_id = c.id and m.role = 'user' \
                   and m.status = 'complete' and m.content <> '' \
                 order by m.seq asc limit 1) as preview, \
                (trunc(extract(epoch from c.created_at) * 1000))::bigint, \
                (trunc(extract(epoch from c.updated_at) * 1000))::bigint \
         from conversations c \
         where c.user_id = $1::uuid and c.kind = 'inbox' and c.archived = false \
         order by c.updated_at desc \
         limit 50",
    )
    .bind(user_id)
    .bind(CONVERSATION_PREVIEW_CHARS)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, preview, created_ms, updated_ms)| InboxConversationSummary {
                id,
                created_at: epoch_ms_to_iso(created_ms),
                updated_at: epoch_ms_to_iso(updated_ms),
                preview: preview.unwrap_or_default(),
            },
        )
        .collect())
}

pub async fn create_inbox_conversation(
    pg: &PgPool,
    user_id: &str,
    agent_model: Option<&str>,
) -> Result<String, sqlx::Error> {
    let (id,): (String,) = sqlx::query_as(
        "insert into conversations (user_id, agent_model, title, kind) \
         values ($1::uuid, $2, 'Inbox', 'inbox') returning id::text",
    )
    .bind(user_id)
    .bind(agent_model.unwrap_or("inbox-assistant"))
    .fetch_one(pg)
    .await?;
    Ok(id)
}

pub async fn archive_inbox_conversation(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<bool, sqlx::Error> {
    let archived = sqlx::query_scalar::<_, String>(
        "update conversations set archived = true, updated_at = now() \
             where id = $1::uuid and user_id = $2::uuid and kind = 'inbox' and archived = false \
             returning id::text",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(archived.is_some())
}

/// The instance a request may use: the requested one when it is the owner's
/// live inbox conversation, else their most recently touched one. None when
/// they have none — the READ path stops here (an empty page, no row created);
/// the WRITE path (`resolve_inbox_conversation_for_write`) starts a fresh one.
async fn owned_inbox_conversation_id(
    pg: &PgPool,
    user_id: &str,
    requested: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    if let Some(requested) = requested.filter(|r| !r.is_empty()) {
        let row: Option<(String,)> = sqlx::query_as(
            "select id::text from conversations \
             where id = $1::uuid and user_id = $2::uuid and kind = 'inbox' and archived = false \
             limit 1",
        )
        .bind(requested)
        .bind(user_id)
        .fetch_optional(pg)
        .await?;
        if let Some((id,)) = row {
            return Ok(Some(id));
        }
    }
    let latest: Option<(String,)> = sqlx::query_as(
        "select id::text from conversations \
         where user_id = $1::uuid and kind = 'inbox' and archived = false \
         order by updated_at desc limit 1",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(latest.map(|(id,)| id))
}

async fn resolve_inbox_conversation_for_write(
    pg: &PgPool,
    user_id: &str,
    agent_model: Option<&str>,
    requested: Option<&str>,
) -> Result<String, sqlx::Error> {
    match owned_inbox_conversation_id(pg, user_id, requested).await? {
        Some(existing) => Ok(existing),
        None => create_inbox_conversation(pg, user_id, agent_model).await,
    }
}

/// The model's history for ONE inbox conversation instance.
///
/// THE CONTEXT STRATEGY, in one place (the bounds live beside
/// `limit_inbox_model_history`, which applies them): segmentation first — the
/// owner picks the instance from the panel's chat picker, and starting a
/// fresh one is how old context is shed. Within an instance: a turn window,
/// and ATTACHMENTS EXPAND ONLY WHILE FRESH — full text for the last two user
/// turns (the "keep asking about the doc I just attached" loop), a bare
/// `[attached: …]` marker after that. Re-expanding every file on every turn
/// forever was the single biggest silent bloat in this prompt: one 6k-char
/// attachment rode along in full until it aged out of the turn window, on
/// every command, answering questions that had nothing to do with it.
///   · QUICK ACTIONS NEVER ENTER — decisions, proposals, confirmations and
///     undos are timeline rows, not messages, so the noise the owner clicks
///     through cannot become the model's context. That is a property of this
///     loader reading `messages` only, and it is deliberate.
async fn recent_inbox_history(
    pg: &PgPool,
    sb: &crate::secretbox::SecretBox,
    conversation_id: &str,
) -> Result<Vec<OwnedTurn>, sqlx::Error> {
    // TS fetches with `limit ${INBOX_HISTORY_MAX_TURNS}`; the same cap is
    // re-applied by `limit_inbox_model_history` below, as in TS.
    let rows: Vec<(String, String, Value)> = sqlx::query_as(
        "select role, content, attachments from messages \
         where conversation_id = $1::uuid and role in ('user', 'assistant') \
           and status = 'complete' and content <> '' \
         order by seq desc limit 12",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    let oldest_first: Vec<(String, String, Value)> = rows.into_iter().rev().collect();
    // Which user turns are "fresh" enough to carry their attachment text: the
    // LAST TWO user rows in the window. Everything older keeps a marker.
    let fresh_user_rows: HashSet<usize> = {
        let mut user_indices: Vec<usize> = oldest_first
            .iter()
            .enumerate()
            .filter(|(_, (role, _, _))| role == "user")
            .map(|(index, _)| index)
            .collect();
        let keep = user_indices.split_off(user_indices.len().saturating_sub(2));
        keep.into_iter().collect()
    };
    let mut mapped: Vec<OwnedTurn> = Vec::with_capacity(oldest_first.len());
    for (index, (role, content, attachments)) in oldest_first.iter().enumerate() {
        let content = if role == "user" {
            if fresh_user_rows.contains(&index) {
                format!(
                    "{content}{}{}",
                    ref_blocks(attachments),
                    attachment_text_blocks(pg, sb, attachments, 3).await
                )
            } else {
                format!("{content}{}", marker_for(attachments))
            }
        } else {
            content.clone()
        };
        mapped.push(OwnedTurn {
            role: role.clone(),
            content,
        });
    }
    Ok(limit_inbox_model_history(&mapped))
}

/// The bare `[attached: …]` marker an aged-out user turn keeps.
fn marker_for(attachments: &Value) -> String {
    let public = public_attachments(attachments);
    let names: Vec<&str> = public
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|a| a.get("filename").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        String::new()
    } else {
        format!("\n[attached: {}]", names.join(", "))
    }
}

/// `publicAttachments` — the wire shape of a message's attachments: only
/// rows with the four typed fields, `refType` kept only for the two ref
/// kinds (content and every other bookkeeping key dropped).
fn public_attachments(value: &Value) -> Value {
    let Some(items) = value.as_array() else {
        return json!([]);
    };
    let out: Vec<Value> = items
        .iter()
        .filter_map(|item| {
            let object = item.as_object()?;
            let id = object.get("id")?.as_str()?;
            let filename = object.get("filename")?.as_str()?;
            let mime = object.get("mime")?.as_str()?;
            // `typeof size === 'number'` — the number rides VERBATIM: going
            // through f64 would render integer sizes as `1.0` where TS's
            // JSON.stringify renders `1`, and this object is wire bytes.
            let size = object.get("size")?.as_number()?.clone();
            let mut entry = json!({
                "id": id,
                "filename": filename,
                "mime": mime,
                "size": size,
            });
            if let Some(ref_type) = object.get("refType").and_then(Value::as_str)
                && matches!(ref_type, "kb-doc" | "artifact")
            {
                entry["refType"] = json!(ref_type);
            }
            Some(entry)
        })
        .collect();
    Value::Array(out)
}

/// The effort the ANSWERING model may be asked for. Same rule as
/// `valid_response_model`: the pick is checked against what the model's own
/// metadata vouches for (`efforts_for_model` resolves the assistant persona
/// to the model actually serving it), and a pick that fails the check is an
/// error rather than a silent drop — the sender has a picker built on the same
/// metadata, so a mismatch means one of the two is stale and both should say
/// so. None (no pick) is always fine and means the model's default.
async fn validated_effort(
    pg: &PgPool,
    model: Option<&str>,
    effort: &str,
) -> Result<Option<String>, String> {
    let Some(model) = model else {
        return Err("Your assistant is not configured, so it cannot honor an effort pick.".into());
    };
    let efforts = efforts_for_model(pg, model).await;
    if !efforts.contains(&effort.to_string()) {
        let offered = if efforts.is_empty() {
            String::new()
        } else {
            format!(" (offered: {})", efforts.join(", "))
        };
        return Err(format!(
            "That effort (\"{effort}\") is not available on {model}{offered}."
        ));
    }
    Ok(Some(effort.to_string()))
}

/// The persona-configured default for the answering model, held against its
/// published levels — the same rule `/api/chat` applies when a chat sender
/// made no pick. None for anything that is not a persona with a configured
/// effort, and for a configured level the model no longer publishes.
async fn default_effort_for(pg: &PgPool, model: Option<&str>) -> Option<String> {
    let model = model?;
    let configured = persona_configured_effort(pg, model).await?;
    let efforts = efforts_for_model(pg, model).await;
    efforts.contains(&configured).then_some(configured)
}

/// `messageEntry` — TS funnels one record through `buildInboxTimeline` and
/// takes the message entry out; constructing the entry directly is the same
/// bytes (the divider the builder would have prefixed is dropped by the
/// `find`, and the field extraction below is the builder's own).
fn message_entry(
    id: &str,
    role: &str,
    content: &str,
    status: &str,
    created_at: &str,
    metadata: &Value,
    attachments: Option<Value>,
) -> InboxTimelineEntry {
    InboxTimelineEntry::Message(MessageEntry {
        id: id.to_string(),
        role: role.to_string(),
        content: content.to_string(),
        status: status.to_string(),
        created_at: created_at.to_string(),
        focus: focus_from_metadata(Some(metadata)),
        actor: metadata.get("actor").cloned().filter(|a| !a.is_null()),
        delegate_model: metadata
            .get("delegateModel")
            .and_then(Value::as_str)
            .map(str::to_string),
        response_model: metadata
            .get("responseModel")
            .and_then(Value::as_str)
            .map(str::to_string),
        mode: metadata
            .get("mode")
            .and_then(Value::as_str)
            .map(str::to_string),
        attachments: attachments.unwrap_or(json!([])),
    })
}

/// `chunkText` — 96-char slices. JS slices at UTF-16 code-unit boundaries and
/// would cut a surrogate pair in half at a boundary; Rust strings cannot hold
/// half a pair, so the port closes each chunk at the char boundary at or
/// before the limit (recorded: astral chars never split mid-pair).
fn chunk_text(value: &str, size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut units = 0usize;
    for ch in value.chars() {
        let ch_units = ch.len_utf16();
        if units + ch_units > size && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            units = 0;
        }
        current.push(ch);
        units += ch_units;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

async fn link_decision(
    pg: &PgPool,
    decision_id: &str,
    conversation_id: &str,
    user_message_id: &str,
    assistant_message_id: &str,
) -> Result<(), sqlx::Error> {
    // `where id = ${decisionId}` with no owner scope — TS's own shape; an
    // empty id is skipped rather than sent (TS's undefined binds NULL and
    // matches nothing; `''::uuid` would be a Postgres syntax error).
    if decision_id.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "update inbox_decisions \
         set conversation_id = $2::uuid, user_message_id = $3::uuid, assistant_message_id = $4::uuid \
         where id = $1::uuid",
    )
    .bind(decision_id)
    .bind(conversation_id)
    .bind(user_message_id)
    .bind(assistant_message_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn set_message_metadata(
    pg: &PgPool,
    message_id: &str,
    metadata: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("update messages set metadata = $2 where id = $1::uuid")
        .bind(message_id)
        .bind(metadata)
        .execute(pg)
        .await?;
    Ok(())
}

/// The parsed command body — everything `runInboxConversationCommand` takes
/// except the abort signal (the port's run future is dropped by the stream
/// task's lifetime instead).
pub struct InboxCommandInput {
    pub instruction: String,
    pub focus_key: Option<String>,
    /// Which view the assistant panel is floating over (see surfaceBrief).
    pub surface: Option<String>,
    pub delegate_model: Option<String>,
    pub response_model: Option<String>,
    /// 'normal' | 'fast' | 'plan'
    pub mode: String,
    /// The owner's reasoning-effort pick, validated below.
    pub effort: Option<String>,
    pub attachment_ids: Vec<String>,
    pub refs: Vec<MessageRef>,
    /// Which conversation instance this command belongs to — the panel's chat
    /// picker. Validated against the owner's live inbox conversations; a null
    /// or stale id falls back to their most recent instance, creating one
    /// only when none exists.
    pub conversation_id: Option<String>,
}

/// `runInboxConversationCommand` — the SSE generator as an event channel. TS
/// yields; the port sends into `tx` and the route streams frames. Ok(()) is
/// the generator completing (including its own caught-error tail, which
/// ARRIVES as an Error event — the run is a success at the transport level);
/// Err is a prologue failure the route renders as the TS catch's error event
/// (its message, or 'Your assistant could not start that response.' for a
/// panicked task).
///
/// On a client disconnect the TS loop stops consuming and the generator is
/// returned at its current yield — leaving the assistant row 'streaming'.
/// The port instead finishes the run and persists the completed reply: the
/// chat family's tee philosophy (a reply in progress survives its reader),
/// and the recorded divergence here.
pub async fn run_inbox_conversation_command(
    state: &AppState,
    user: &SessionUser,
    input: InboxCommandInput,
    tx: mpsc::UnboundedSender<InboxCommandEvent>,
) -> Result<(), FocusError> {
    let send = |event: InboxCommandEvent| {
        let _ = tx.send(event);
    };
    let pg = &state.pg;
    let sb = state.secretbox().await.map_err(FocusError::Throw)?;
    let assistant = focus_assistant_for(pg, &user.id).await?;
    let response_model = valid_response_model(pg, user, input.response_model.as_deref()).await?;
    // The owner's explicit pick, else the assistant persona's CONFIGURED default
    // (the agent editor's effort beside the assistant's model) — never both, and
    // the default only when the answering model IS the persona that carries it
    // (`persona_configured_effort` answers None for any other id, including a
    // picked response model, whose effort is nobody's to default).
    let answering = response_model.as_deref().or(assistant.model.as_deref());
    let effort = match input.effort.as_deref().filter(|e| !e.is_empty()) {
        Some(pick) => validated_effort(pg, answering, pick)
            .await
            .map_err(FocusError::Throw)?,
        None => default_effort_for(pg, answering).await,
    };
    let mode = input.mode.clone();
    let mode_instruction = match mode.as_str() {
        "plan" => {
            "\n\n[Plan mode: answer with a concise plan or clarifying question. Do not propose or imply execution.]"
        }
        "fast" => "\n\n[Fast mode: answer directly and keep the response brief.]",
        _ => "",
    };
    let conversation_id = resolve_inbox_conversation_for_write(
        pg,
        &user.id,
        assistant.model.as_deref(),
        input.conversation_id.as_deref(),
    )
    .await?;
    let item = match input.focus_key.as_deref() {
        Some(focus_key) => find_focus_item_for_user(pg, user, focus_key).await?,
        None => None,
    };
    if input.focus_key.is_some() && item.is_none() {
        send(InboxCommandEvent::Error {
            message: "That focus item changed or was already resolved.".into(),
        });
        return Ok(());
    }

    let focus = item.as_ref().map(focus_context);
    let who = user.email.clone().or_else(|| user.name.clone());
    let attachment_access = futures_util::future::join_all(input.attachment_ids.iter().map(|id| {
        can_access_upload(
            pg,
            id,
            UploadViewer::Human {
                user_id: &user.id,
                who: who.as_deref(),
                is_admin: user.role == "admin",
            },
        )
    }))
    .await;
    let allowed_ids: Vec<String> = input
        .attachment_ids
        .iter()
        .zip(attachment_access)
        .filter(|(_, ok)| *ok)
        .map(|(id, _)| id.clone())
        .collect();
    let uploads = resolve_attachments(pg, &allowed_ids).await?;
    let reference_chips = resolve_refs(
        pg,
        &RefUser {
            id: &user.id,
            email: user.email.as_deref(),
            name: user.name.as_deref(),
        },
        &input.refs,
    )
    .await?;
    let message_attachments = {
        let mut items: Vec<Value> = uploads
            .iter()
            .map(|a| serde_json::to_value(a).expect("attachment serializes"))
            .collect();
        items.extend(
            reference_chips
                .iter()
                .map(|c| serde_json::to_value(c).expect("ref chip serializes")),
        );
        Value::Array(items)
    };
    let attachment_context = format!(
        "{}{}",
        ref_blocks(&message_attachments),
        attachment_text_blocks(pg, &sb, &message_attachments, 3).await
    );
    let visible_attachments = public_attachments(&message_attachments);
    let history = recent_inbox_history(pg, &sb, &conversation_id).await?;
    let created_at = now_iso();
    let user_metadata = json!({
        "focus": focus,
        "actor": { "type": "human", "id": user.id, "label": actor_of(user) },
        "responseModel": response_model,
        "mode": mode,
        "effort": effort,
    });
    let user_seq = next_seq(pg, &conversation_id).await?;
    let user_message_id = insert_user_message(
        pg,
        &conversation_id,
        user_seq,
        &input.instruction,
        &message_attachments,
        Some(&user.id),
        &user_metadata,
    )
    .await?;
    let assistant_metadata = json!({
        "focus": focus,
        "actor": {
            "type": "assistant",
            "id": assistant.model,
            "label": assistant.name.clone().unwrap_or_else(|| "your assistant".into()),
        },
        "delegateModel": input.delegate_model,
        "responseModel": response_model,
        "mode": mode,
        "effort": effort,
    });
    let assistant_message_id =
        insert_streaming_assistant(pg, &conversation_id, user_seq + 1, &assistant_metadata).await?;
    touch_conversation(pg, &conversation_id, None).await?;
    send(InboxCommandEvent::Conversation {
        conversation_id: conversation_id.clone(),
        entry: message_entry(
            &user_message_id,
            "user",
            &input.instruction,
            "complete",
            &created_at,
            &user_metadata,
            Some(visible_attachments),
        ),
    });
    send(InboxCommandEvent::Status {
        label: match mode.as_str() {
            "plan" => "Planning with your assistant".to_string(),
            "fast" => "Answering quickly".to_string(),
            _ if focus.is_some() => "Reviewing the active decision".to_string(),
            _ => "Thinking with your assistant".to_string(),
        },
    });

    // The generator's internal try/catch: a failure from here on is an Error
    // EVENT (the stream continues to its close), never a thrown Err.
    let internal = run_inbox_command_tail(
        state,
        user,
        input.instruction.clone(),
        input.surface.clone(),
        input.delegate_model.clone(),
        response_model.clone(),
        mode.clone(),
        mode_instruction,
        effort.clone(),
        attachment_context,
        history,
        item,
        assistant,
        conversation_id.clone(),
        user_message_id.clone(),
        assistant_message_id.clone(),
        assistant_metadata,
        &tx,
    )
    .await;
    if let Err(message) = internal {
        let _ = update_assistant(pg, &assistant_message_id, "", "", &[], "error").await;
        send(InboxCommandEvent::Error { message });
    }
    Ok(())
}

/// Everything inside the generator's try block — the part whose failures are
/// Error events rather than a thrown Err. Returns Err(message) for the TS
/// `catch` (the caller updates the row and emits the event); Ok(()) when the
/// Done event went out.
#[allow(clippy::too_many_arguments)]
async fn run_inbox_command_tail(
    state: &AppState,
    user: &SessionUser,
    instruction: String,
    surface: Option<String>,
    delegate_model: Option<String>,
    response_model: Option<String>,
    mode: String,
    mode_instruction: &str,
    effort: Option<String>,
    attachment_context: String,
    history: Vec<OwnedTurn>,
    item: Option<RawFocusItem>,
    assistant: FocusAssistant,
    conversation_id: String,
    user_message_id: String,
    assistant_message_id: String,
    mut assistant_metadata: Value,
    tx: &mpsc::UnboundedSender<InboxCommandEvent>,
) -> Result<(), String> {
    let pg = &state.pg;
    let send = |event: InboxCommandEvent| {
        let _ = tx.send(event);
    };
    let mut content: String;
    let mut result: Option<Value> = None;
    // True once a delta has gone out, so the tail does not re-send the reply.
    let mut streamed = false;
    if let Some(item) = &item {
        let command = run_focus_command(
            state,
            user,
            &FocusCommandCall {
                key: item.key.clone(),
                instruction: instruction.clone(),
                delegate_model: delegate_model.clone(),
                delegate_tier: None,
                response_model: response_model.clone(),
                mode: Some(mode.clone()),
                effort: effort.clone(),
                attachment_context: Some(attachment_context.clone()),
                history: history.clone(),
            },
        )
        .await
        .map_err(|e| match e {
            FocusError::Db(err) => err.to_string(),
            FocusError::Throw(m) => m,
        })?;
        match command {
            None => {
                let stale = json!({
                    "status": "stale",
                    "message": "That focus item changed or was already resolved.",
                });
                content = stale
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                result = Some(stale);
            }
            Some(command) => {
                content = command
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // `assistantMetadata.delegateModel = …` overwrites in place;
                // `decisionId` is a new key appended after effort.
                if let Some(object) = assistant_metadata.as_object_mut() {
                    object.insert(
                        "delegateModel".into(),
                        command
                            .get("consultedModel")
                            .cloned()
                            .unwrap_or(Value::Null),
                    );
                    object.insert(
                        "decisionId".into(),
                        command.get("decisionId").cloned().unwrap_or(Value::Null),
                    );
                }
                link_decision(
                    pg,
                    command
                        .get("decisionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    &conversation_id,
                    &user_message_id,
                    &assistant_message_id,
                )
                .await
                .map_err(|e| e.to_string())?;
                if let Some(consulted) = command.get("consultedModel").and_then(Value::as_str) {
                    send(InboxCommandEvent::Status {
                        label: format!("Consulted {consulted}"),
                    });
                }
                result = Some(command);
            }
        }
    } else {
        // STREAMED, AND THE CONTENT EVENTS GO OUT AS THEY ARRIVE. This used to
        // await the whole reply and then hand the finished string to `chunkText`
        // below, which is not streaming: the status line sat there for the entire
        // turn with nothing under it, and then the answer landed all at once. On
        // a model that takes twenty seconds that reads as the assistant not
        // replying — which is exactly how it was reported.
        //
        // `streamed` is set so the tail of this function knows not to chunk a
        // reply the panel has already been given token by token.
        let model = response_model.clone().or_else(|| {
            assistant
                .configured
                .then(|| assistant.model.clone())
                .flatten()
        });
        match model {
            None => {
                content = "Your personal assistant is not configured yet. You can still use the safe actions in the Focus Queue.".into();
            }
            Some(model) => {
                let caller = match &response_model {
                    Some(_) => format!(
                        "user:{}",
                        user.email.clone().unwrap_or_else(|| user.id.clone())
                    ),
                    None => format!("inbox:{model}"),
                };
                let reply_input = FocusReplyInput {
                    instruction: format!("{instruction}{mode_instruction}{attachment_context}"),
                    surface,
                    history,
                };
                let mut reply = stream_reply(
                    state,
                    &model,
                    reply_input,
                    &caller,
                    20_000,
                    effort.as_deref(),
                )
                .await;
                let mut accumulated = String::new();
                while let Some(delta) = reply.deltas.recv().await {
                    accumulated.push_str(&delta);
                    streamed = true;
                    send(InboxCommandEvent::Content { text: delta });
                }
                // The run RETURNS its own guarded text. Preferred over the
                // deltas we relayed: a redacted or repaired reply is what
                // should be persisted, even though the raw one is what was
                // already on screen.
                let finished = match reply.done.await {
                    Ok(Ok(value)) => value,
                    Ok(Err(message)) => return Err(message),
                    Err(_) => return Err("Your assistant could not finish that response.".into()),
                };
                content = finished.unwrap_or(accumulated);
                if content.is_empty() {
                    content = "Your assistant is temporarily unavailable. No tools or mutations were attempted.".into();
                }
            }
        }
    }

    set_message_metadata(pg, &assistant_message_id, &assistant_metadata)
        .await
        .map_err(|e| e.to_string())?;
    update_assistant(pg, &assistant_message_id, &content, "", &[], "complete")
        .await
        .map_err(|e| e.to_string())?;
    // The focus-command branch produces a finished proposal message rather than
    // a stream, so it is still chunked here. A streamed reply is already on
    // screen and re-sending it would print the answer twice.
    if !streamed {
        for text in chunk_text(&content, 96) {
            send(InboxCommandEvent::Content { text });
        }
    }
    if let Some(result) = &result
        && result.get("status").is_none()
        && result.get("kind").and_then(Value::as_str) == Some("proposal")
        && let Some(decision_id) = result.get("decisionId").and_then(Value::as_str)
        && let Ok(Some(entry)) = timeline_entry_for_decision(state, user, decision_id, None).await
    {
        send(InboxCommandEvent::Activity { entry });
    }
    let assistant_entry = message_entry(
        &assistant_message_id,
        "assistant",
        &content,
        "complete",
        &now_iso(),
        &assistant_metadata,
        None,
    );
    send(InboxCommandEvent::Done {
        conversation_id,
        entry: assistant_entry,
        result,
    });
    Ok(())
}

// ── The timeline ─────────────────────────────────────────────────────────────

/// One row of the unioned timeline query — a message or a decision. The
/// timestamps arrive as epoch ms (TS held `Date`s; every use is an ISO render
/// or an ms compare, so ms is the honest shape).
struct TimelineRow {
    record_type: String,
    id: String,
    created_ms: i64,
    role: Option<String>,
    content: Option<String>,
    message_status: Option<String>,
    metadata: Option<Value>,
    attachments: Option<Value>,
    decision_status: Option<String>,
    action_id: Option<String>,
    #[allow(dead_code)] // selected by the union; the entry reads instruction from the record
    instruction: Option<String>,
    proposal: Option<Value>,
    outcome: Option<Value>,
    focus: Option<Value>,
    expires_ms: Option<i64>,
    completed_ms: Option<i64>,
}

/// The union's own columns, one each.
#[allow(clippy::type_complexity)]
type TimelineSqlRow = (
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Value>,
    Option<Value>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Value>,
    Option<Value>,
    Option<Value>,
    Option<i64>,
    Option<i64>,
);

fn timeline_row_of(row: TimelineSqlRow) -> TimelineRow {
    TimelineRow {
        record_type: row.0,
        id: row.1,
        created_ms: row.2,
        role: row.3,
        content: row.4,
        message_status: row.5,
        metadata: row.6,
        attachments: row.7,
        decision_status: row.8,
        action_id: row.9,
        instruction: row.10,
        proposal: row.11,
        outcome: row.12,
        focus: row.13,
        expires_ms: row.14,
        completed_ms: row.15,
    }
}

async fn timeline_rows(
    pg: &PgPool,
    conversation_id: &str,
    cursor: Option<&str>,
) -> Result<(Vec<TimelineRow>, bool), sqlx::Error> {
    let before = cursor.and_then(decode_inbox_timeline_cursor);
    // The union keeps `id` uuid and `created_at` timestamptz so the ORDER BY
    // compares the same types TS's does; the cursor's tie-break compares
    // `id::text`, and the outer select renders epoch ms — both exactly as TS.
    let rows: Vec<TimelineSqlRow> = sqlx::query_as(
        "select record_type, id::text, \
                (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
                role, content, message_status, metadata, attachments, \
                decision_status, action_id, instruction, proposal, outcome, focus, \
                (trunc(extract(epoch from expires_at) * 1000))::bigint as expires_ms, \
                (trunc(extract(epoch from completed_at) * 1000))::bigint as completed_ms \
         from ( \
            select 'message'::text as record_type, m.id, m.created_at, \
                   m.role, m.content, m.status as message_status, m.metadata, m.attachments, \
                   null::text as decision_status, null::text as action_id, null::text as instruction, \
                   null::jsonb as proposal, null::jsonb as outcome, null::jsonb as focus, \
                   null::timestamptz as expires_at, null::timestamptz as completed_at \
            from messages m \
            where m.conversation_id = $1::uuid and m.role in ('user', 'assistant') \
            union all \
            select 'decision'::text as record_type, d.id, d.created_at, \
                   null::text as role, null::text as content, null::text as message_status, \
                   null::jsonb as metadata, null::jsonb as attachments, \
                   d.status as decision_status, d.action_id as action_id, d.instruction, \
                   d.proposal, d.outcome, d.focus_context as focus, \
                   d.expires_at as expires_at, d.completed_at as completed_at \
            from inbox_decisions d \
            where d.conversation_id = $1::uuid and d.focus_context is not null \
         ) timeline \
         where ($2::timestamptz is null \
            or timeline.created_at < $2::timestamptz \
            or (timeline.created_at = $2::timestamptz and timeline.id::text < $3)) \
         order by timeline.created_at desc, timeline.id desc \
         limit $4",
    )
    .bind(conversation_id)
    .bind(before.as_ref().map(|(created_at, _)| created_at.clone()))
    .bind(before.as_ref().map(|(_, id)| id.clone()))
    .bind((PAGE_SIZE + 1) as i64)
    .fetch_all(pg)
    .await?;
    let has_more = rows.len() > PAGE_SIZE;
    let rows: Vec<TimelineRow> = rows
        .into_iter()
        .take(PAGE_SIZE)
        .map(timeline_row_of)
        .collect();
    Ok((rows, has_more))
}

/// `timelineRecordForDecision` — a decision row into its timeline record,
/// reissuing a still-pending confirmation's token so the panel can act on an
/// old proposal after a reload. `current_result` is the action result the
/// route just produced (its token/expires win over the row's).
async fn timeline_record_for_decision(
    state: &AppState,
    user: &SessionUser,
    row: &TimelineRow,
    current_result: Option<&Value>,
) -> Result<Option<TimelineDecisionRecord>, FocusError> {
    if row.record_type != "decision" || row.focus.is_none() || row.decision_status.is_none() {
        return Ok(None);
    }
    let mut status = row.decision_status.clone().expect("checked above");
    let mut confirmation_token: Option<String> = current_result
        .and_then(|r| r.get("confirmationToken"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut expires_at: Option<String> = current_result
        .and_then(|r| r.get("expiresAt"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| row.expires_ms.map(epoch_ms_to_iso));
    let mut proposal = row.proposal.clone().unwrap_or(Value::Null);
    // `proposalObject?.preview && !confirmationToken` — JS truthiness on both
    // sides: a null/false/0/"" preview or token counts as absent.
    let has_preview = proposal
        .as_object()
        .and_then(|p| p.get("preview"))
        .is_some_and(js_truthy);
    let token_falsy = !confirmation_token
        .as_deref()
        .is_some_and(|t| js_truthy(&Value::String(t.to_string())));
    if status == "proposed" && has_preview && token_falsy {
        let reissued = reissue_focus_confirmation(state, user, &row.id).await?;
        match reissued.get("status").and_then(Value::as_str) {
            Some("confirmation_required") => {
                confirmation_token = reissued
                    .get("confirmationToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                expires_at = reissued
                    .get("expiresAt")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // `{...proposalObject, preview: reissued.preview}` — the
                // stored order with preview overwritten in place.
                if let Some(object) = proposal.as_object_mut() {
                    object.insert(
                        "preview".into(),
                        reissued.get("preview").cloned().unwrap_or(Value::Null),
                    );
                }
            }
            Some("stale") => status = "failed".into(),
            _ => {}
        }
    }
    let completed_ms = row.completed_ms.unwrap_or(0);
    let undo_expires_at = if row.action_id.as_deref() == Some("mark_read")
        && status == "completed"
        && completed_ms > now_ms() - UNDO_MS
    {
        Some(epoch_ms_to_iso(completed_ms + UNDO_MS))
    } else {
        current_result
            .and_then(|r| r.get("undo"))
            .and_then(|u| u.get("expiresAt"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    // The TS literal's `...(x ? {x} : {})` spreads — a falsy value leaves the
    // key OUT, not present-and-empty.
    Ok(Some(TimelineDecisionRecord {
        id: row.id.clone(),
        created_at: epoch_ms_to_iso(row.created_ms),
        status,
        action_id: row.action_id.clone(),
        instruction: row.instruction.clone(),
        proposal,
        outcome: row.outcome.clone().unwrap_or(Value::Null),
        focus: row.focus.clone().expect("checked above"),
        confirmation_token: confirmation_token.filter(|t| !t.is_empty()),
        expires_at: expires_at.filter(|t| !t.is_empty()),
        undo_expires_at: undo_expires_at.filter(|t| !t.is_empty()),
    }))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One instance's timeline page. READ-ONLY: an owner with no instances yet
/// gets an empty page (conversationId null) rather than a conversation row
/// created by a GET — instances are created by an explicit action (the
/// picker's "New chat") or by the first thing written.
pub async fn get_inbox_conversation(
    state: &AppState,
    user: &SessionUser,
    cursor: Option<&str>,
    conversation_id: Option<&str>,
) -> Result<InboxConversationPage, FocusError> {
    let pg = &state.pg;
    let Some(resolved) = owned_inbox_conversation_id(pg, &user.id, conversation_id).await? else {
        return Ok(InboxConversationPage {
            conversation_id: None,
            entries: Vec::new(),
            next_cursor: None,
            working: false,
        });
    };
    let (rows, has_more) = timeline_rows(pg, &resolved, cursor).await?;
    let mut records: Vec<TimelineRecord> = Vec::new();
    for row in &rows {
        if row.record_type == "message" && row.role.is_some() && row.message_status.is_some() {
            records.push(TimelineRecord::Message {
                id: row.id.clone(),
                created_at: epoch_ms_to_iso(row.created_ms),
                role: row.role.clone().expect("checked above"),
                content: row.content.clone().unwrap_or_default(),
                status: row.message_status.clone().expect("checked above"),
                metadata: row.metadata.clone(),
                attachments: Some(public_attachments(
                    row.attachments.as_ref().unwrap_or(&Value::Null),
                )),
            });
        } else if let Some(record) = timeline_record_for_decision(state, user, row, None).await? {
            records.push(TimelineRecord::Decision(record));
        }
    }
    let oldest = rows.last();
    let next_cursor = match (has_more, oldest) {
        (true, Some(oldest)) => Some(encode_inbox_timeline_cursor(
            &epoch_ms_to_iso(oldest.created_ms),
            &oldest.id,
        )),
        _ => None,
    };
    let working = rows.iter().any(|row| {
        row.record_type == "message" && row.message_status.as_deref() == Some("streaming")
    });
    Ok(InboxConversationPage {
        conversation_id: Some(resolved),
        entries: build_inbox_timeline(records),
        next_cursor,
        working,
    })
}

pub async fn timeline_entry_for_decision(
    state: &AppState,
    user: &SessionUser,
    decision_id: &str,
    current_result: Option<&Value>,
) -> Result<Option<InboxTimelineEntry>, FocusError> {
    // No conversation resolution here, deliberately: the decision already
    // carries the instance it was made in (`link_decision`/`record_inbox_snooze`
    // write it at insert), and under segmentation there is no "the user's one
    // conversation" to re-link it to. The earlier re-link was legacy from before
    // the column existed.
    let pg = &state.pg;
    let row: Option<TimelineSqlRow> = sqlx::query_as(
        "select 'decision'::text as record_type, d.id::text, \
                (trunc(extract(epoch from d.created_at) * 1000))::bigint as created_ms, \
                null::text as role, null::text as content, null::text as message_status, \
                null::jsonb as metadata, null::jsonb as attachments, \
                d.status as decision_status, d.action_id as action_id, d.instruction, \
                d.proposal, d.outcome, d.focus_context as focus, \
                (trunc(extract(epoch from d.expires_at) * 1000))::bigint as expires_ms, \
                (trunc(extract(epoch from d.completed_at) * 1000))::bigint as completed_ms \
         from inbox_decisions d where d.id = $1::uuid and d.user_id = $2::uuid",
    )
    .bind(decision_id)
    .bind(&user.id)
    .fetch_optional(pg)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let row = timeline_row_of(row);
    let Some(record) = timeline_record_for_decision(state, user, &row, current_result).await?
    else {
        return Ok(None);
    };
    Ok(build_inbox_timeline(vec![TimelineRecord::Decision(record)])
        .into_iter()
        .find(|entry| matches!(entry, InboxTimelineEntry::Activity(_))))
}

pub async fn attach_timeline_to_action_result(
    state: &AppState,
    user: &SessionUser,
    result: Value,
) -> Result<Value, FocusError> {
    let Some(decision_id) = result.get("decisionId").and_then(Value::as_str) else {
        return Ok(result);
    };
    let timeline_entry =
        timeline_entry_for_decision(state, user, decision_id, Some(&result)).await?;
    match timeline_entry {
        Some(entry) => {
            // `{ ...result, timelineEntry }` — the entry is a NEW key, so it
            // appends at the end of the spread result.
            let mut with_entry = result;
            if let Some(object) = with_entry.as_object_mut() {
                object.insert(
                    "timelineEntry".into(),
                    serde_json::to_value(&entry).expect("entry serializes"),
                );
            }
            Ok(with_entry)
        }
        None => Ok(result),
    }
}

pub async fn record_inbox_snooze(
    state: &AppState,
    user: &SessionUser,
    source_type: &str,
    source_id: &str,
    snoozed_until: &str,
) -> Result<Option<InboxTimelineEntry>, FocusError> {
    let pg = &state.pg;
    let Some(item) =
        find_focus_item_for_user(pg, user, &format!("{source_type}:{source_id}")).await?
    else {
        return Ok(None);
    };
    let assistant = focus_assistant_for(pg, &user.id).await?;
    let conversation_id =
        resolve_inbox_conversation_for_write(pg, &user.id, assistant.model.as_deref(), None)
            .await?;
    let outcome = json!({
        "snoozedUntil": snoozed_until,
        "message": format!("Snoozed until {snoozed_until}"),
    });
    let (id,): (String,) = sqlx::query_as(
        "insert into inbox_decisions ( \
            user_id, source_type, source_id, action_id, status, outcome, focus_context, \
            conversation_id, completed_at \
         ) values ($1::uuid, $2, $3, 'snooze', 'completed', $4, $5, $6::uuid, now()) \
         returning id::text",
    )
    .bind(&user.id)
    .bind(&item.source_type)
    .bind(&item.source_id)
    .bind(&outcome)
    .bind(focus_context(&item))
    .bind(&conversation_id)
    .fetch_one(pg)
    .await?;
    timeline_entry_for_decision(state, user, &id, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_close_at_char_boundaries() {
        let ascii = "a".repeat(200);
        assert_eq!(
            chunk_text(&ascii, 96),
            vec!["a".repeat(96), "a".repeat(96), "a".repeat(8)]
        );
        // A 96-unit window that would split an astral char closes one unit
        // early instead — the recorded divergence from JS slicing.
        let astral = "𝄞".repeat(49); // 49 × 2 units = 98
        let chunks = chunk_text(&astral, 96);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chars().count(), 48);
        assert_eq!(chunks[1].chars().count(), 1);
    }

    #[test]
    fn marker_names_join_like_ts() {
        let attachments = json!([
            { "id": "1", "filename": "a.txt", "mime": "text/plain", "size": 3 },
            { "id": "2", "filename": "b.txt", "mime": "text/plain", "size": 4 },
            "not an object",
        ]);
        assert_eq!(marker_for(&attachments), "\n[attached: a.txt, b.txt]");
        assert_eq!(marker_for(&json!(null)), "");
    }

    #[test]
    fn public_attachments_keep_only_ref_kinds() {
        let attachments = json!([
            { "id": "1", "filename": "a", "mime": "text/plain", "size": 1, "refType": "kb-doc", "content": "secret" },
            { "id": "2", "filename": "b", "mime": "text/plain", "size": 2, "refType": "other" },
            { "id": 3 },
        ]);
        assert_eq!(
            public_attachments(&attachments).to_string(),
            r#"[{"id":"1","filename":"a","mime":"text/plain","size":1,"refType":"kb-doc"},{"id":"2","filename":"b","mime":"text/plain","size":2}]"#
        );
    }

    #[test]
    fn message_entry_matches_the_builder_shape() {
        let metadata = json!({
            "focus": { "key": "task:1", "question": "q", "sourceHref": "/boards/b/1" },
            "actor": { "type": "human", "id": "u", "label": "u@example.com" },
            "responseModel": "m",
            "mode": "fast",
        });
        let InboxTimelineEntry::Message(entry) = message_entry(
            "m1",
            "user",
            "hello",
            "complete",
            "2026-08-29T00:00:00.000Z",
            &metadata,
            None,
        ) else {
            panic!("message entry");
        };
        assert_eq!(entry.attachments, json!([]));
        assert_eq!(entry.delegate_model, None);
        assert_eq!(entry.response_model.as_deref(), Some("m"));
        assert_eq!(entry.mode.as_deref(), Some("fast"));
        assert!(entry.focus.is_some());
        assert!(entry.actor.is_some());
    }
}
