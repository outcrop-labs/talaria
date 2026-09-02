// /api/chat. POST { model,
// conversationId?, content } → durable streaming chat: the turn is persisted
// to Postgres (the server owns history) and the gateway stream is TEED — one
// branch to the client as SSE with X-Conversation-Id / X-Message-Id, one
// drained server-side (chat_persist) so an in-progress reply survives a
// disconnect. Messages sent while a reply streams queue into history (202)
// and the completing turn picks them up.

use crate::body::{
    array_msg, array_too_big_msg, as_object, enum_member, object_msg, optional_boolean_member,
    optional_enum_member, optional_max_string_member, optional_uuid_array_member,
    optional_uuid_member, string_member, uuid_member, zod_type_name,
};
use crate::chat_persist::{
    PersistMeta, PlanMeta, TurnMeta, continue_conversation, persist_assistant_stream,
};
use crate::conversations::{
    accessible_conversation, active_streaming_assistant, create_conversation,
    insert_streaming_assistant, insert_user_message, list_plan_members, next_seq, prior_messages,
    title_from, touch_conversation,
};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::{routed_model_for, usable_agent_gate};
use crate::gateway::fleet_chat::{chat_payload, proxy_chat};
use crate::model::efforts::efforts_for_model;
use crate::permissions::has_perm;
use crate::persona::persona_configured_effort;
use crate::plan_doc::{PLAN_MODE_PROMPT, plan_routing_block};
use crate::refs::{MessageRef, RefUser, resolve_refs};
use crate::research::{RESEARCH_MODE_PROMPT, list_research_members, research_run_for_conversation};
use crate::research_origin::mark_agent_turn;
use crate::retrieval::index::IndexDoc;
use crate::retrieval::sources::index_activity;
use crate::session::require_user;
use crate::state::AppState;
use crate::uploads::{
    attachment_as_data_url, attachment_text_blocks, is_image, resolve_attachments,
};
use crate::workspace_handles::{HANDLE_TURN_NOTE, mentions_handle};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

struct ChatBody {
    model: String,
    conversation_id: Option<String>,
    content: String,
    tier: Option<String>,
    effort: Option<String>,
    attachment_ids: Option<Vec<String>>,
    refs: Option<Vec<MessageRef>>,
    kind: Option<String>, // 'chat' | 'plan' | 'research'
    template_id: Option<String>,
    queue: bool,
}

fn validate(obj: &serde_json::Map<String, Value>) -> Result<ChatBody, String> {
    let model = string_member(obj, "model", 1, usize::MAX)?;
    let conversation_id = optional_uuid_member(obj, "conversationId")?;
    // Content may be empty when the turn is attachments-only.
    let content = match obj.get("content") {
        None => String::new(),
        Some(_) => string_member(obj, "content", 0, 100_000)?,
    };
    let tier = optional_max_string_member(obj, "tier", 60)?;
    let effort = optional_max_string_member(obj, "effort", 24)?;
    let attachment_ids = optional_uuid_array_member(obj, "attachmentIds", 10)?;
    let refs = match obj.get("refs") {
        None => None,
        Some(v) => {
            let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
            if arr.len() > 3 {
                return Err(array_too_big_msg(3));
            }
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let e = item
                    .as_object()
                    .ok_or_else(|| object_msg(zod_type_name(item)))?;
                let ref_type = enum_member(e, "type", &["kb-doc", "artifact"])?;
                let id = uuid_member(e, "id")?;
                out.push(MessageRef { ref_type, id });
            }
            Some(out)
        }
    };
    let kind = optional_enum_member(obj, "kind", &["chat", "plan", "research"])?;
    let template_id = optional_uuid_member(obj, "templateId")?;
    let queue = optional_boolean_member(obj, "queue")?.unwrap_or(false);
    Ok(ChatBody {
        model,
        conversation_id,
        content,
        tier,
        effort,
        attachment_ids,
        refs,
        kind,
        template_id,
        queue,
    })
}

/// The queued-turn ack — wire key order: `queued`, then `conversationId`.
#[derive(serde::Serialize)]
struct QueuedAck {
    queued: bool,
    #[serde(rename = "conversationId")]
    conversation_id: String,
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // Resolve the conversation (access-checked: yours, or a plan you're a
    // member of — the WHERE clause is the whole rule) or start a new one.
    let mut conv_id = body.conversation_id.clone();
    let mut agent_model = body.model.clone();
    let mut kind = body.kind.clone().unwrap_or_else(|| "chat".into());
    let mut plan_title: Option<String> = None;
    let mut plan_owner_id = user.id.clone();
    let mut multi_voice = false;
    if let Some(cid) = conv_id.as_deref() {
        let conv = match accessible_conversation(&state.pg, &user.id, cid).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("[chat] conversation read failed: {e}");
                return thrown_internal_error();
            }
        };
        let Some(conv) = conv else {
            return house_error(StatusCode::NOT_FOUND, "conversation not found");
        };
        agent_model = conv.agent_model;
        kind = conv.kind;
        plan_title = conv.title;
        plan_owner_id = conv.owner_user_id;
        // WHO ELSE IS IN THE ROOM. Both shared surfaces prefix a turn with
        // the speaker's name when there is more than one human, so the agent
        // can tell voices apart — research reads its roster off the RUN's
        // members rather than `conversation_members`, the same rule its
        // access follows.
        if kind == "plan" {
            multi_voice = list_plan_members(&state.pg, cid)
                .await
                .map(|m| m.len() > 1)
                .unwrap_or(false);
        } else if kind == "research" {
            let run = research_run_for_conversation(&state.pg, cid)
                .await
                .ok()
                .flatten();
            multi_voice = match run {
                Some(run_id) => !list_research_members(&state.pg, &run_id)
                    .await
                    .unwrap_or_default()
                    .is_empty(),
                None => false,
            };
        }
    }

    // Owner-aware gate: blocks another user from driving someone's personal
    // assistant (which would act as that owner — Google, memory, private soul).
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[chat] agent access read failed: {e}");
            return thrown_internal_error();
        }
    };
    if !gate(&agent_model) {
        return house_error(StatusCode::FORBIDDEN, "forbidden: no access to this agent");
    }

    // Tier routing: validate against the agent's defined aliases, then
    // request `<base>-<tier>` — the agent's own gateway resolves the alias.
    let routed_model = match routed_model_for(&state.pg, &agent_model, body.tier.as_deref()).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[chat] tier routing read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(routed_model) = routed_model else {
        return house_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "unknown tier \"{}\" for {agent_model}",
                body.tier.as_deref().unwrap_or("undefined")
            ),
        );
    };

    // Effort: the same rule the tier above follows, against the per-model
    // metadata the catalog refresh extracted. A model that publishes no
    // levels supports no pick, so ANY effort on it is the error — "not
    // supported" and "not one of yours" read identically to the sender.
    //
    // No pick on a persona with a CONFIGURED default (the agent editor's pick
    // beside the model) runs at that default — re-validated here, so a level
    // that went stale between the admin saving it and the turn using it is
    // inert rather than a 400. An explicit pick always wins.
    let efforts = efforts_for_model(&state.pg, &routed_model).await;
    let mut effort = body.effort.clone();
    if let Some(e) = effort
        .as_deref()
        .filter(|e| !efforts.iter().any(|x| x == e))
    {
        let offered = if efforts.is_empty() {
            String::new()
        } else {
            format!(" (offered: {})", efforts.join(", "))
        };
        return house_error(
            StatusCode::BAD_REQUEST,
            &format!("unsupported effort \"{e}\" for {routed_model}{offered}"),
        );
    }
    if effort.is_none()
        && let Some(configured) = persona_configured_effort(&state.pg, &routed_model)
            .await
            .filter(|c| efforts.iter().any(|x| x == c))
    {
        effort = Some(configured);
    }

    // Validate attachments belong to real uploads before stamping them;
    // knowledge/artifact refs resolve to content-carrying chips.
    let uploads = resolve_attachments(&state.pg, body.attachment_ids.as_deref().unwrap_or(&[]))
        .await
        .unwrap_or_default();
    let ref_user = RefUser {
        id: &user.id,
        email: user.email.as_deref(),
        name: user.name.as_deref(),
    };
    let ref_chips = resolve_refs(&state.pg, &ref_user, body.refs.as_deref().unwrap_or(&[]))
        .await
        .unwrap_or_default();
    let mut attachments: Vec<Value> = uploads
        .iter()
        .map(|a| serde_json::to_value(a).unwrap_or(Value::Null))
        .collect();
    attachments.extend(
        ref_chips
            .iter()
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null)),
    );
    // the title falls back content → first attachment's filename → 'chat'
    let title = if !body.content.is_empty() {
        title_from(&body.content)
    } else {
        attachments
            .first()
            .and_then(|a| a.get("filename"))
            .and_then(|f| f.as_str())
            .map(title_from)
            .unwrap_or_else(|| "chat".into())
    };
    if conv_id.is_none() {
        if kind == "plan" {
            let allowed = has_perm(&state.pg, &user.id, &user.role, "plans.create")
                .await
                .unwrap_or(false);
            if !allowed {
                return house_error(StatusCode::FORBIDDEN, "no permission to create plans");
            }
        }
        let created = create_conversation(
            &state.pg,
            &user.id,
            &agent_model,
            &title,
            &kind,
            body.template_id.as_deref(),
        )
        .await;
        match created {
            Ok(id) => {
                conv_id = Some(id);
                plan_title = Some(title.clone());
            }
            Err(e) => {
                tracing::error!("[chat] conversation create failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    let conv_id = conv_id.expect("created or resolved above");

    // Claude-style flow: while a reply is streaming, new messages QUEUE into
    // history instead of interrupting — the completing turn (or the
    // continuation below, if the stream just ended) picks them up.
    let in_flight = active_streaming_assistant(&state.pg, &conv_id)
        .await
        .unwrap_or(None);
    let queued = in_flight.is_some() || body.queue;

    // Record this turn (history is built AFTER for normal turns, so the new
    // message isn't duplicated into the prior list).
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[chat] secretbox unusable: {e}");
            return thrown_internal_error();
        }
    };
    let prior = if queued {
        Vec::new()
    } else {
        match prior_messages(&state.pg, &sb, &conv_id).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("[chat] history read failed: {e}");
                return thrown_internal_error();
            }
        }
    };
    let user_seq = match next_seq(&state.pg, &conv_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[chat] seq read failed: {e}");
            return thrown_internal_error();
        }
    };
    // The effort pick rides the user's row: the queued-message contract. A
    // reply that is already streaming means this turn is covered later by
    // `continue_conversation`, which re-reads exactly this stamp (and
    // re-validates it against the routed model) when it builds the next turn.
    let metadata = match effort.as_deref() {
        Some(e) => json!({ "effort": e }),
        None => json!({}),
    };
    let user_msg_id = match insert_user_message(
        &state.pg,
        &conv_id,
        user_seq,
        &body.content,
        &Value::Array(attachments.clone()),
        Some(&user.id),
        &metadata,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("[chat] user turn persist failed: {e}");
            return thrown_internal_error();
        }
    };
    let _ = touch_conversation(&state.pg, &conv_id, Some(&title)).await;

    // Plan turns feed the ambient activity brain (plan-owner-scoped) and
    // notify @mentioned teammates who can read the plan's document.
    let sender_label = user
        .name
        .clone()
        .or_else(|| user.email.clone())
        .unwrap_or_else(|| "someone".into());
    let plan_meta = if kind == "plan" {
        Some(PlanMeta {
            owner_user_id: plan_owner_id.clone(),
            title: plan_title.clone(),
        })
    } else {
        None
    };
    if kind == "plan" && !body.content.trim().is_empty() {
        index_user_plan_turn(
            &state,
            &conv_id,
            &user_msg_id,
            &body.content,
            plan_title.as_deref(),
            &sender_label,
            &plan_owner_id,
        );
        notify_user_plan_turn(
            &state,
            &conv_id,
            &user.id,
            &sender_label,
            &body.content,
            plan_title.as_deref(),
        );
    }

    if queued {
        // If the stream ended between the client's view and this landing,
        // nothing would pick the message up — chain the next turn ourselves.
        if in_flight.is_none() {
            let state = state.clone();
            let conv_id = conv_id.clone();
            let meta = TurnMeta {
                agent_model: agent_model.clone(),
                tier: body.tier.clone(),
                plan: plan_meta.clone(),
            };
            tokio::spawn(async move {
                continue_conversation(&state, &conv_id, &meta).await;
            });
        }
        return (
            StatusCode::ACCEPTED,
            axum::Json(QueuedAck {
                queued: true,
                conversation_id: conv_id,
            }),
        )
            .into_response();
    }

    // Give a vision-capable model the actual images (data URLs work even when
    // the agent can't reach Talaria over the network). Text-only history
    // stays plain strings.
    let mut image_urls: Vec<String> = Vec::new();
    for a in &uploads {
        if is_image(&a.mime)
            && let Some(url) = attachment_as_data_url(&state.pg, &sb, &a.id).await
        {
            image_urls.push(url);
        }
    }
    // Multiplayer plans: prefix this turn with the sender's name (history is
    // already prefixed by prior_messages) so the agent tells voices apart.
    // Attached refs contribute their content blocks.
    let spoken_content = format!(
        "{}{}{}",
        if multi_voice && !body.content.is_empty() {
            format!("{sender_label}: {}", body.content)
        } else {
            body.content.clone()
        },
        crate::refs::ref_blocks(&Value::Array(
            ref_chips
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
                .collect()
        )),
        attachment_text_blocks(
            &state.pg,
            &sb,
            &Value::Array(
                uploads
                    .iter()
                    .map(|a| serde_json::to_value(a).unwrap_or(Value::Null))
                    .collect()
            ),
            3,
        )
        .await,
    );
    let user_content: Value = if !image_urls.is_empty() {
        let mut parts: Vec<Value> = Vec::new();
        if !spoken_content.trim().is_empty() {
            parts.push(json!({ "type": "text", "text": spoken_content }));
        }
        parts.extend(
            image_urls
                .iter()
                .map(|url| json!({ "type": "image_url", "image_url": { "url": url } })),
        );
        Value::Array(parts)
    } else {
        Value::String(spoken_content.clone())
    };
    // Plan turns carry the plan-mode harness: think and decide, read freely,
    // create NOTHING — tickets come from Draft tickets later. Research turns
    // carry their own.
    //
    // A HANDLE IN THE MESSAGE NEEDS EXPLAINING, and only when one is there.
    // Standing grants ride in the agent's soul; a relay is minted
    // mid-conversation for one errand, so the soul written this morning has
    // never heard of it. Unexplained, it reads «secret:relay-…» as a typo and
    // asks the human to send the real value — exactly the paste the relay
    // existed to prevent. Costs a paragraph, on the turns that mention one.
    let mut messages: Vec<Value> = Vec::new();
    if kind == "plan" {
        let block = plan_routing_block(&state.pg).await;
        messages.push(json!({ "role": "system", "content": format!("{PLAN_MODE_PROMPT}{block}") }));
    }
    if kind == "research" {
        messages.push(json!({ "role": "system", "content": RESEARCH_MODE_PROMPT }));
    }
    if mentions_handle(&spoken_content) {
        messages.push(json!({ "role": "system", "content": HANDLE_TURN_NOTE }));
    }
    messages.extend(
        prior
            .iter()
            .map(|m| json!({ "role": m.role, "content": m.content })),
    );
    messages.push(json!({ "role": "user", "content": user_content }));
    let assistant_id =
        match insert_streaming_assistant(&state.pg, &conv_id, user_seq + 1, &json!({})).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("[chat] assistant row create failed: {e}");
                return thrown_internal_error();
            }
        };

    // WHERE THIS AGENT IS ANSWERING, recorded before the turn leaves for the
    // container and not after — work the agent starts from inside this turn
    // reaches Talaria as its own authenticated request carrying nothing but
    // an agent key, so this is the only place the two can be tied together.
    {
        let state = state.clone();
        let agent_model = agent_model.clone();
        let conv_id = conv_id.clone();
        tokio::spawn(async move {
            mark_agent_turn(&state, &agent_model, &conv_id).await;
        });
    }

    let upstream = proxy_chat(
        &chat_payload(
            &routed_model,
            &Value::Array(messages.clone()),
            effort.as_deref(),
        ),
        None,
    )
    .await;

    // Tee: one branch relays to the client (fed by the persist loop's forward
    // channel — a client hang-up is a send error it ignores, exactly what a
    // tee does to its other branch), one persists server-side, detached, so
    // it completes even if the client disconnects.
    let prompt_chars: usize = messages
        .iter()
        .map(|m| crate::conversations::content_js_length(&m["content"]))
        .sum();
    let persist_meta = PersistMeta {
        agent_model: agent_model.clone(),
        prompt_chars,
        tier: body.tier.clone(),
        plan: plan_meta.clone(),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let assistant_id_header = assistant_id.clone();
    let persist_conv_id = conv_id.clone();
    tokio::spawn(async move {
        persist_assistant_stream(
            state,
            upstream.body,
            assistant_id,
            persist_conv_id,
            Some(persist_meta),
            Some(tx),
        )
        .await;
    });
    let client_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Response::builder()
        .status(StatusCode::from_u16(upstream.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        // cache-control first — it sits above content-type on the wire, and
        // header order is part of the contract.
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONTENT_TYPE, upstream.content_type.clone())
        .header("x-conversation-id", conv_id.as_str())
        .header("x-message-id", assistant_id_header.as_str())
        .body(Body::from_stream(client_stream))
        .expect("static headers build")
}

/// The user's plan turn, indexed (plan-owner-scoped) — detached.
#[allow(clippy::too_many_arguments)]
fn index_user_plan_turn(
    state: &AppState,
    conv_id: &str,
    msg_id: &str,
    content: &str,
    plan_title: Option<&str>,
    sender_label: &str,
    plan_owner_id: &str,
) {
    let pg = state.pg.clone();
    let doc = IndexDoc {
        source_type: "plan".into(),
        source_id: msg_id.to_string(),
        title: Some(format!(
            "Plan ({}) · {}",
            plan_title.unwrap_or("Untitled"),
            sender_label
        )),
        text: content.to_string(),
        payload: Some(
            vec![
                ("planId".to_string(), json!(conv_id)),
                ("planOwnerId".to_string(), json!(plan_owner_id)),
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

/// The user's plan turn's @mentions, notified — detached.
fn notify_user_plan_turn(
    state: &AppState,
    conv_id: &str,
    user_id: &str,
    sender_label: &str,
    content: &str,
    plan_title: Option<&str>,
) {
    let state = state.clone();
    let conv_id = conv_id.to_string();
    let user_id = user_id.to_string();
    let sender_label = sender_label.to_string();
    let content = content.to_string();
    let plan_title = plan_title.map(str::to_string);
    tokio::spawn(async move {
        let notify =
            crate::notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
        crate::plan_doc::notify_plan_mentions(
            &notify,
            &state.pg,
            &conv_id,
            &user_id,
            &sender_label,
            &content,
            plan_title.as_deref(),
        )
        .await;
    });
}
