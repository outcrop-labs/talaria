// /api/chat. POST { model,
// conversationId?, content } → durable streaming chat: the turn is persisted
// to Postgres (the server owns history) and the gateway stream is TEED — one
// branch to the client as SSE with X-Conversation-Id / X-Message-Id, one
// drained server-side (chat_persist) so an in-progress reply survives a
// disconnect. Messages sent while a reply streams queue into history (202)
// and the completing turn picks them up.

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::fleet::{routed_model_for, usable_agent_gate};
use talaria_api_facades::model::efforts::efforts_for_model;
use talaria_api_facades::retrieval::index::IndexDoc;
use talaria_api_facades::retrieval::sources::index_activity;
use talaria_api_facades::runs::decide::{DecideArgs, DecideResult, decide};
use talaria_api_facades::runs::real_decide_deps;
use talaria_body::{
    array_msg, array_too_big_msg, enum_member, object_msg, optional_boolean_member,
    optional_enum_member, optional_max_string_member, optional_uuid_array_member,
    optional_uuid_member, string_member, string_msg, uuid_member, zod_type_name,
};
use talaria_chat_persist::{
    OpeningTurn, PersistMeta, PlanMeta, TurnMeta, continue_conversation, persist_assistant_stream,
    pinned_turn, send_turn, turn_sent,
};
use talaria_conversations::{
    accessible_conversation, active_streaming_assistant, create_conversation,
    insert_streaming_assistant, insert_user_message, is_seq_collision, list_plan_members, next_seq,
    prior_messages, title_from, touch_conversation,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{NotifyDeps, fan_conversation_event};
use talaria_permissions::has_perm;
use talaria_persona::persona_configured_effort;
use talaria_plan_doc::{PLAN_MODE_PROMPT, plan_routing_block};
use talaria_realtime_watch::RealtimeDeps;
use talaria_refs::{MessageRef, RefUser, resolve_refs};
use talaria_research::{
    RESEARCH_MODE_PROMPT, RESEARCH_WORKING_PROMPT, awaiting_scope_answer, list_research_members,
    research_run_for_conversation, run_state_for_conversation,
};
use talaria_research_origin::mark_agent_turn;
use talaria_sender_identity::{SenderIdentity, sender_identity_message};
use talaria_session::secretbox_or_500;
use talaria_session::{require_user, who_of};
use talaria_state::AppState;
use talaria_uploads::{
    UploadViewer, attachment_as_data_url, attachment_text_blocks, can_access_upload, is_image,
    resolve_attachments,
};
use talaria_work_mode::{
    FocusKind, PaneFocus, WORK_MODE_PROMPT, clean_focus_title, focus_id_ok, focus_line,
};
use talaria_workspace_handles::{HANDLE_TURN_NOTE, mentions_handle};

struct ChatBody {
    model: String,
    conversation_id: Option<String>,
    content: String,
    tier: Option<String>,
    effort: Option<String>,
    attachment_ids: Option<Vec<String>>,
    refs: Option<Vec<MessageRef>>,
    kind: Option<String>, // 'chat' | 'plan' | 'research' | 'ticket' | 'work'
    template_id: Option<String>,
    queue: bool,
    /// The file open in the teammate's pane (work turns only; validated for
    /// every kind, read only for `work`). Null and absent both mean none.
    focus: Option<PaneFocus>,
    /// The page's own name for this turn, a uuid it drew: a stop that names
    /// it reaches this turn before the api has read this send, and on a new
    /// thread before the page knows the thread (talaria-chat-persist
    /// `turn_stop`).
    turn_id: Option<String>,
}

/// `focus: { kind: 'artifact' | 'google', id, title? } | null`. The kind is
/// an enum, the id a file id (`focus_id_ok`); the title is cleaned and capped
/// rather than refused (`clean_focus_title`) — a long or odd file name must
/// never be what stops a message sending, and it reaches the prompt only in
/// its cleaned form.
fn focus_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<PaneFocus>, String> {
    let f = match obj.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?,
    };
    let kind = enum_member(f, "kind", &["artifact", "google"])?;
    let kind = FocusKind::parse(&kind).expect("enum_member admitted only these two");
    let id = string_member(f, "id", 1, usize::MAX)?;
    if !focus_id_ok(&id) {
        return Err("Invalid input: expected focus.id to be a file id".into());
    }
    let title = match f.get("title") {
        None | Some(Value::Null) => None,
        Some(v) => clean_focus_title(v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?),
    };
    Ok(Some(PaneFocus { kind, id, title }))
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
    let kind = optional_enum_member(obj, "kind", &["chat", "plan", "research", "ticket", "work"])?;
    let template_id = optional_uuid_member(obj, "templateId")?;
    let queue = optional_boolean_member(obj, "queue")?.unwrap_or(false);
    // { kind: artifact|google, id, title? } or null — the file open in the
    // teammate's pane; read on work turns only
    let focus = focus_member(obj, "focus")?;
    let turn_id = optional_uuid_member(obj, "turnId")?;
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
        focus,
        turn_id,
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
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let body = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // Resolve the conversation (access-checked: yours, or a plan you're a
    // member of — the WHERE clause is the whole rule) or start a new one.
    let mut conv_id = body.conversation_id.clone();
    let mut agent_model = body.model.clone();
    let mut kind = body.kind.clone().unwrap_or_else(|| "chat".into());
    let mut plan_title: Option<String> = None;
    let mut plan_owner_id = user.id.clone();
    let mut multi_voice = false;
    // Which research prompt this conversation's agent speaks with, set while
    // resolving the conversation below — None everywhere but research.
    let mut research_prompt: Option<&'static str> = None;
    let mut opening: Option<OpeningTurn> = None;
    if let Some(cid) = conv_id.as_deref() {
        let conv = match accessible_conversation(&state.pg, &user.id, cid).await {
            Ok(c) => c,
            Err(e) => return Ok(internal("[chat] conversation read failed", e)),
        };
        let Some(conv) = conv else {
            return Ok(house_error(StatusCode::NOT_FOUND, "conversation not found"));
        };
        // A stop pressed the moment this was sent can arrive before the
        // reply row exists: from here it waits for this turn to name itself
        // instead of missing it (talaria-chat-persist `OpeningTurn`).
        opening = Some(OpeningTurn::take(cid));
        agent_model = conv.agent_model;
        kind = conv.kind;
        plan_title = conv.title;
        plan_owner_id = conv.owner_user_id;
        // WHO ELSE IS IN THE ROOM. Both shared surfaces prefix a turn with
        // the speaker's name when there is more than one human, so the agent
        // can tell voices apart — research reads its roster off the RUN's
        // members rather than `conversation_members`, the same rule its
        // access follows.
        // A work session carries the plan's membership, so it reads its
        // roster the same way — the shared idiom, not a second one.
        if kind == "plan" || kind == "work" {
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
            // THE PROMPT FOLLOWS THE RUN. While the run is working (or parked
            // on a question), the report does not exist yet and the mode
            // prompt's "answer from the report" would be an instruction to
            // invent one. A lookup error picks the working variant: wrongly
            // telling the agent a report sits beside it is the expensive
            // direction.
            research_prompt = match run_state_for_conversation(&state.pg, cid).await {
                Ok(Some(s)) if s == "queued" || s == "running" || s == "awaiting" => {
                    Some(RESEARCH_WORKING_PROMPT)
                }
                Ok(_) => Some(RESEARCH_MODE_PROMPT),
                Err(_) => Some(RESEARCH_WORKING_PROMPT),
            };
        }
    }

    // Owner-aware gate: blocks another user from driving someone's personal
    // assistant (which would act as that owner — Google, memory, private soul).
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => return Ok(internal("[chat] agent access read failed", e)),
    };
    if !gate(&agent_model) {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "forbidden: no access to this agent",
        ));
    }

    // Tier routing: validate against the agent's defined aliases, then
    // request `<base>-<tier>` — the agent's own gateway resolves the alias.
    let routed_model = match routed_model_for(&state.pg, &agent_model, body.tier.as_deref()).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[chat] tier routing read failed", e)),
    };
    let Some(routed_model) = routed_model else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "unknown tier \"{}\" for {agent_model}",
                body.tier.as_deref().unwrap_or("undefined")
            ),
        ));
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
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &format!("unsupported effort \"{e}\" for {routed_model}{offered}"),
        ));
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
    //
    // THE SENDER MUST BE ABLE TO SEE WHAT THEY SEND. An id is not a grant:
    // an upload seen once in a channel the person has since left, or in a
    // ticket, would otherwise be read to the agent here and then stamped on
    // this person's own row — after which `can_access_upload`'s containment
    // probe answers yes for them forever. So every id passes the same reach
    // check the serve route asks before any of it is read, and a refusal
    // names the id so a client can drop the one attachment that is not
    // theirs.
    let who = who_of(&user);
    for id in body.attachment_ids.as_deref().unwrap_or(&[]) {
        let viewer = UploadViewer::Human {
            user_id: &user.id,
            who: who.as_deref(),
            is_admin: user.role == "admin",
        };
        if !can_access_upload(&state.pg, id, viewer).await {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                &format!("attachment {id} is not one you can read"),
            ));
        }
    }
    let uploads = resolve_attachments(&state.pg, body.attachment_ids.as_deref().unwrap_or(&[]))
        .await
        .unwrap_or_default();
    let ref_user = RefUser {
        id: &user.id,
        email: user.email.as_deref(),
        name: user.name.as_deref(),
    };
    // The same row ref_user reads, as the turn's speaker: the TALA-80 stamp
    // must name who the authenticated row says is typing, not who the client
    // or the model claims.
    let identity = SenderIdentity {
        id: user.id.clone(),
        name: user.name.clone(),
        email: user.email.clone(),
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
        if kind == "ticket" {
            // The ensure route is the only creator: a ticket thread needs its
            // task, the owner ladder and the binder — that is the ensure
            // route's job, not the sender's.
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "ticket threads are opened on the ticket",
            ));
        }
        // Creating one of the shared surfaces is a permission; talking in one
        // you were added to is membership, checked above.
        //
        // EITHER PERMISSION OPENS A WORK SESSION, and the arm lists both.
        // The merged surface starts a work session where the Plan surface
        // used to start a plan, so a person holding plans.create and not
        // work.sessions could begin a plan yesterday and can begin nothing
        // today. The plan kind keeps its single permission: plans.create is
        // what it always was.
        if let Some((perms, refusal)) = match kind.as_str() {
            "plan" => Some((&["plans.create"][..], "no permission to create plans")),
            "work" => Some((
                &["work.sessions", "plans.create"][..],
                "no permission to start a session — needs work.sessions or plans.create",
            )),
            _ => None,
        } {
            // `has_perm` is a read per permission, so this is a loop and not
            // an `any` — it stops at the first one that holds.
            let mut allowed = false;
            for perm in perms {
                if has_perm(&state.pg, &user.id, &user.role, perm)
                    .await
                    .unwrap_or(false)
                {
                    allowed = true;
                    break;
                }
            }
            if !allowed {
                return Ok(house_error(StatusCode::FORBIDDEN, refusal));
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
            Err(e) => return Ok(internal("[chat] conversation create failed", e)),
        }
    }
    let conv_id = conv_id.expect("created or resolved above");

    // A new thread's opening is claimed as soon as its id exists. And from
    // here a stop that names this send's turn (`turnId`) finds it: the
    // caller's access is checked and the thread's opening claimed. A stop
    // that came first and outlived its wait is handed to the opening, whose
    // drain sends it the moment the turn names itself.
    let mut opening = opening.unwrap_or_else(|| OpeningTurn::take(&conv_id));
    if let Some(turn_id) = body.turn_id.as_deref() {
        let sent = turn_sent(turn_id, &user.id, &conv_id);
        if sent.stop_held() {
            if opening.hold_stop_here() {
                tracing::info!(
                    "[chat] {conv_id}: a person's stop for this turn came before the send did; it is held until the agent opens the turn"
                );
            } else {
                tracing::info!(
                    "[chat] {conv_id}: a person's stop for this turn came before the send did, and another turn holds the thread: nothing of this send's to stop"
                );
            }
        }
        opening.keep_turn(sent);
    }

    // Claude-style flow: while a reply is streaming, new messages QUEUE into
    // history instead of interrupting — the completing turn (or the
    // continuation below, if the stream just ended) picks them up.
    let in_flight = active_streaming_assistant(&state.pg, &conv_id)
        .await
        .unwrap_or(None);
    let queued = in_flight.is_some() || body.queue;

    // Record this turn (history is built AFTER for normal turns, so the new
    // message isn't duplicated into the prior list).
    let sb = secretbox_or_500(&state, "[chat] secretbox unusable").await?;
    let prior = if queued {
        Vec::new()
    } else {
        match prior_messages(&state.pg, &sb, &conv_id, None).await {
            Ok(p) => p,
            Err(e) => return Ok(internal("[chat] history read failed", e)),
        }
    };
    let user_seq = match next_seq(&state.pg, &conv_id).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[chat] seq read failed", e)),
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
        // Two tabs sent at the same instant and this one lost the seq: the
        // message did not land, which is a thing the sender can act on (send
        // it again), not a fault of the server's.
        Err(e) if is_seq_collision(&e) => {
            return Ok(house_error(
                StatusCode::CONFLICT,
                "another message landed at the same moment — send this one again",
            ));
        }
        Err(e) => return Ok(internal("[chat] user turn persist failed", e)),
    };
    let _ = touch_conversation(&state.pg, &conv_id, Some(&title)).await;

    // Plan turns feed the ambient activity brain (plan-owner-scoped) and
    // notify @mentioned teammates who can read the plan's document.
    let sender_label = user
        .name
        .clone()
        .or_else(|| user.email.clone())
        .unwrap_or_else(|| "someone".into());
    // ONLY A PLAN HAS A LIVING DOCUMENT. A landed plan turn rewrites the
    // plan's document from the conversation so far (and its reply feeds the
    // plan's activity index and @mention notices). A work session has no
    // such document: its files are the ones the agent creates and edits with
    // its own tools, and rewriting one of them from the transcript after
    // every turn would overwrite the agent's work with a summary of the chat.
    let plan_meta = if kind == "plan" {
        Some(PlanMeta {
            owner_user_id: plan_owner_id.clone(),
            title: plan_title.clone(),
            surface: kind.clone(),
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
        // And the rail: a teammate's turn in a shared plan is unread to every
        // OTHER reader — the sender is here, looking at it, but a collaborator
        // elsewhere sees their pill move now, not on the next poll. Chat turns
        // skip this: the only reader is the sender, who is present by
        // definition. (The agent's reply fans from chat_persist on complete.)
        fan_conversation_event(
            NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
            conv_id.clone(),
        );
    }

    // A REPLY IN A PARKED RESEARCH RUN IS THE ANSWER IT WAITS ON. The scope
    // step parks a vague run on a free-text question that lives in THIS
    // conversation, so the owner's next message here is not a persona turn —
    // it is the decision's note. Spending it on decide() re-queues the run
    // (decide enforces the audience itself: a member's message does not
    // answer, and falls through to a normal turn). The run posts its own
    // ack turn into this conversation moments later, which is what the
    // surface's resuming poll picks up — so the 202 below, the queued shape
    // ChatView already knows, is the whole response.
    if kind == "research"
        && !body.content.trim().is_empty()
        && let Some(run_id) = awaiting_scope_answer(&state.pg, &conv_id)
            .await
            .ok()
            .flatten()
    {
        let spent = match state.redis().await {
            Ok(redis) => {
                let deps = real_decide_deps(
                    state.pg.clone(),
                    redis.clone(),
                    RealtimeDeps::publish_only(Some(redis)),
                );
                match decide(
                    DecideArgs {
                        run_id: run_id.clone(),
                        // The free-text park's single placeholder option —
                        // the sentence is the answer, riding the note.
                        option_id: "answered".into(),
                        note: Some(body.content.clone()),
                        by: user.id.clone(),
                        start: None,
                    },
                    &deps,
                )
                .await
                {
                    Ok(DecideResult::Decided { .. }) => true,
                    Ok(DecideResult::Refused { reason, .. }) => {
                        tracing::info!(
                            "[chat] {conv_id}: a message did not answer run {run_id} ({reason:?})"
                        );
                        false
                    }
                    Err(e) => {
                        tracing::error!("[chat] {conv_id}: answering run {run_id} failed: {e}");
                        false
                    }
                }
            }
            Err(e) => {
                tracing::error!("[chat] {conv_id}: decide deps unavailable: {e}");
                false
            }
        };
        if spent {
            return Ok((
                StatusCode::ACCEPTED,
                axum::Json(QueuedAck {
                    queued: true,
                    conversation_id: conv_id.clone(),
                }),
            )
                .into_response());
        }
        // Not spent — the message still lands as a persona turn below, it
        // just didn't resume anything.
    }

    // A work turn's system lines, computed once: the live turn below pushes
    // them, and a queued or resumed turn carries them on its metadata so it
    // speaks to the file that was open when this message was sent.
    let work_prompts: Vec<String> = if kind == "work" {
        vec![
            WORK_MODE_PROMPT.to_string(),
            focus_line(body.focus.as_ref()),
        ]
    } else {
        Vec::new()
    };
    if queued {
        // Not this send's turn to open: let go first, so the chained turn
        // below can claim the thread for its own.
        drop(opening);
        // If the stream ended between the client's view and this landing,
        // nothing would pick the message up — chain the next turn ourselves.
        if in_flight.is_none() {
            let state = state.clone();
            let conv_id = conv_id.clone();
            let meta = TurnMeta {
                agent_model: agent_model.clone(),
                tier: body.tier.clone(),
                plan: plan_meta.clone(),
                research_prompt,
                work_prompts: work_prompts.clone(),
                sender: Some(identity.clone()),
            };
            tokio::spawn(async move {
                continue_conversation(&state, &conv_id, &meta).await;
            });
        }
        return Ok((
            StatusCode::ACCEPTED,
            axum::Json(QueuedAck {
                queued: true,
                conversation_id: conv_id,
            }),
        )
            .into_response());
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
        talaria_refs::ref_blocks(&Value::Array(
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
    // A work turn carries the work-mode harness: the surface, the files pane
    // beside the chat, and — the part that matters — which of its writes
    // happen now and which sit in a queue until a person approves them. The
    // prompt is not the gate (decide_action is); it is what stops the agent
    // telling someone their spreadsheet was updated when it was not.
    //
    // Then ONE line naming the file open in the teammate's pane (or that none
    // is), so "this doc" resolves to the tab they are looking at rather than
    // to the agent's guess.
    for prompt in &work_prompts {
        messages.push(json!({ "role": "system", "content": prompt }));
    }
    if kind == "research" {
        messages.push(json!({
            "role": "system",
            // State-chosen above: working while the run works, report-mode
            // once it has finished. A run with no state row is an old
            // conversation about a finished report — the mode prompt.
            "content": research_prompt.unwrap_or(RESEARCH_MODE_PROMPT),
        }));
    }
    if mentions_handle(&spoken_content) {
        messages.push(json!({ "role": "system", "content": HANDLE_TURN_NOTE }));
    }
    // TALA-80: an unattributed user turn invites the agent to fill the
    // speaker from remembered memory — the remembered user, a person from a
    // different conversation — and answer them instead of whoever is typing.
    // The identity rides WITH the turn, built above from the authenticated
    // row, ahead of the history so it lands among the mode prompts and close
    // to the turn. History rows keep their own speaker prefixes; the block
    // rides only this turn's system row.
    messages.push(sender_identity_message(&identity));
    messages.extend(
        prior
            .iter()
            .map(|m| json!({ "role": m.role, "content": m.content })),
    );
    messages.push(json!({ "role": "user", "content": user_content }));
    // Pinned or not is decided BEFORE the assistant row below exists: on a
    // fresh conversation that row would otherwise be the one assistant row
    // with no run id, and the conversation would read as legacy forever.
    let pinned = pinned_turn(&state.pg, &agent_model, &conv_id, None).await;
    let assistant_id =
        match insert_streaming_assistant(&state.pg, &conv_id, user_seq + 1, &json!({})).await {
            Ok(id) => id,
            Err(e) => return Ok(internal("[chat] assistant row create failed", e)),
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

    let sent_at_ms = talaria_agent_auth::now_ms();
    let upstream = send_turn(
        &state.pg,
        pinned,
        &agent_model,
        &routed_model,
        &conv_id,
        &messages,
        effort.as_deref(),
    )
    .await;

    // Tee: one branch relays to the client (fed by the persist loop's forward
    // channel — a client hang-up is a send error it ignores, exactly what a
    // tee does to its other branch), one persists server-side, detached, so
    // it completes even if the client disconnects.
    let persist_meta = PersistMeta {
        agent_model: agent_model.clone(),
        tier: body.tier.clone(),
        plan: plan_meta.clone(),
        research_prompt,
        work_prompts,
        sender: Some(identity),
        // A live turn is a first attempt — eligible for the one auto-resume.
        resumed: false,
        sent_at_ms: Some(sent_at_ms),
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
            opening,
        )
        .await;
    });
    let client_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Ok(Response::builder()
        .status(StatusCode::from_u16(upstream.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        // cache-control first — it sits above content-type on the wire, and
        // header order is part of the contract.
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONTENT_TYPE, upstream.content_type.clone())
        .header("x-conversation-id", conv_id.as_str())
        .header("x-message-id", assistant_id_header.as_str())
        .body(Body::from_stream(client_stream))
        .expect("static headers build"))
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
        let qd = talaria_api_facades::retrieval::qdrant::real_deps();
        let ed = talaria_api_facades::retrieval::embed::real_deps();
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
            talaria_notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
        talaria_plan_doc::notify_plan_mentions(
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
