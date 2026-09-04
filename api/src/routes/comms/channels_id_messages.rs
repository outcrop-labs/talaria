// /api/channels/{id}/messages.
// GET ?since=<seq>&thread=<id> → the channel's messages (members; agents in
// the channel, elevated assistants any non-DM). POST { content } → post a
// message; @mentioned channel agents reply, streamed into the channel.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::body::{
    array_msg, array_too_big_msg, as_object, enum_member, object_msg, optional_uuid_array_member,
    optional_uuid_member, string_member, uuid_member, zod_type_name,
};
use crate::channel_replies::{notify_dm_message, notify_user_mentions, trigger_agent_replies};
use crate::channels::{
    agent_may_access_channel, channel_role, get_channel_message, insert_channel_message,
    inserted_wire, list_channel_messages, list_thread_messages,
};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::describe_agent;
use crate::notify::NotifyDeps;
use crate::refs::{MessageRef, RefUser, resolve_refs};
use crate::retrieval::embed;
use crate::retrieval::index::IndexDoc;
use crate::retrieval::qdrant;
use crate::retrieval::sources::index_activity;
use crate::session::require_user;
use crate::state::AppState;
use crate::uploads::resolve_attachments;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
) -> Response {
    let query = |k: &str| -> Option<String> {
        uri.query().and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
        })
    };
    // `since` coercion: trims, '' → 0, unparseable → NaN — and only finite
    // values survive (NaN falls back to -1).
    let since = match query("since") {
        None => -1.0,
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
    };
    let since = if since.is_finite() { since } else { -1.0 };
    let thread = query("thread");

    // Agents in the channel can read it (elevated assistants: any non-DM).
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return get_as_user(&state, &headers, &id, since, thread).await,
        Err(resp) => return resp,
    };
    let may = match agent_may_access_channel(&state.pg, &id, &AgentSubject::Caller(caller)).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[channels] agent access read failed: {e}");
            return thrown_internal_error();
        }
    };
    if !may {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let messages = match page(&state, &id, since, thread.as_deref()).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[channels] message page read failed: {e}");
            return thrown_internal_error();
        }
    };
    // WITHOUT `guard`. An agent reads a channel through this route (the MCP
    // `read_channel` tool proxies it), and a finding is the guard's verdict on
    // flagged content plus a verbatim excerpt OF that content — the one thing
    // guardrails' cardinal invariant says must never re-enter a model's
    // context. The engine declines to pin findings for exactly this reason
    // and says so; the streamed-reply path pins them anyway, so the
    // projection is where this is closed for good. Humans still get the
    // caveat.
    let stripped: Vec<Value> = messages
        .iter()
        .map(|m| {
            let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
            if let Some(obj) = v.as_object_mut() {
                obj.remove("guard");
            }
            v
        })
        .collect();
    Json(json!({ "messages": stripped })).into_response()
}

async fn get_as_user(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    since: f64,
    thread: Option<String>,
) -> Response {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match channel_role(&state.pg, &user.id, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on GET messages failed: {e}");
            return thrown_internal_error();
        }
    }
    match page(state, id, since, thread.as_deref()).await {
        Ok(messages) => Json(json!({ "messages": messages })).into_response(),
        Err(e) => {
            tracing::error!("[channels] message page read failed: {e}");
            thrown_internal_error()
        }
    }
}

async fn page(
    state: &AppState,
    id: &str,
    since: f64,
    thread: Option<&str>,
) -> Result<Vec<crate::channels::ChannelMessageWire>, String> {
    match thread {
        // The default page: limit 60, main flow only.
        Some(root) => list_thread_messages(&state.pg, id, root)
            .await
            .map_err(|e| e.to_string()),
        None => {
            // A fractional `since` is refused here: sent on to Postgres it
            // would die on the int4 comparison ("invalid input syntax") and
            // 500 the route. The thread arm never passes one, so only this
            // arm checks.
            if since.fract() != 0.0 {
                return Err(format!(
                    "fractional since={since} would die on the int4 comparison downstream"
                ));
            }
            list_channel_messages(&state.pg, id, since as i64, 60, false)
                .await
                .map_err(|e| e.to_string())
        }
    }
}

/// The POST body, in schema order: content carries a default, the arrays
/// are optional and capped, threadRootId is nullish.
struct PostBody {
    content: String,
    attachment_ids: Option<Vec<String>>,
    refs: Option<Vec<MessageRef>>,
    thread_root_id: Option<String>,
}

fn validate_post(obj: &serde_json::Map<String, Value>) -> Result<PostBody, String> {
    // content: max 20_000, default '' — absent takes the default; a present
    // value (null included) must be a string within bounds.
    let content = match obj.get("content") {
        None => String::new(),
        Some(_) => string_member(obj, "content", 0, 20_000)?,
    };
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
    let thread_root_id = optional_uuid_member(obj, "threadRootId")?;
    Ok(PostBody {
        content,
        attachment_ids,
        refs,
        thread_root_id,
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match validate_post(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // A post needs something in it: text, an attachment, or a ref chip.
    if body.content.is_empty()
        && body.attachment_ids.as_ref().is_none_or(|a| a.is_empty())
        && body.refs.as_ref().is_none_or(|r| r.is_empty())
    {
        return house_error(StatusCode::BAD_REQUEST, "bad request");
    }

    // An agent in the channel can post. It doesn't trigger other agents (no
    // reply storms) and can't attach uploads.
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return post_as_user(&state, &headers, &id, body).await,
        Err(resp) => return resp,
    };
    let name = caller.model.clone();
    // The CALLER, not `name`: elevation buys org-wide posting rights.
    let may = match agent_may_access_channel(&state.pg, &id, &AgentSubject::Caller(caller)).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[channels] agent access read on POST failed: {e}");
            return thrown_internal_error();
        }
    };
    if !may {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    if body.content.trim().is_empty() {
        return house_error(StatusCode::BAD_REQUEST, "bad request");
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let msg = match insert_channel_message(
        &notify,
        &id,
        "agent",
        &name,
        &body.content,
        "complete",
        &json!([]),
        None,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[channels] agent post insert failed: {e}");
            return thrown_internal_error();
        }
    };
    let nm = channel_name(&state.pg, &id)
        .await
        .unwrap_or_else(|| "channel".into());
    // `msg.content`, NOT `body.content`: the insert runs an agent's post
    // through the agent-writes door (guard_agent_write), which in strict
    // mode returns the REDACTED body — the row in `channel_messages` is
    // clean, and `body.content` is the raw one.
    //
    // The index is the half that matters. Retrieval is read back INTO model
    // contexts, so an unredacted copy there is not merely a second place the
    // credential is stored: it is the credential re-entering a model's
    // context by the one route guardrails exists to close, arriving as
    // ambient "activity" long after the turn that leaked it. The
    // notification is the same text landing in a human's inbox, unredacted,
    // beside a message that is not.
    //
    // In a task room the post is a COMMENT — the fan-out carries
    // `msg.content` (the redacted body) into the board-scoped comment point
    // and the board's edges, never a channelId-payload channel doc.
    if let Some(meta) = crate::ticket_chat::ticket_for_room(&state.pg, &id).await {
        let pg = state.pg.clone();
        let realtime = notify.realtime.clone();
        let mid = msg.id.clone();
        let author = describe_agent(&name).label;
        let content = msg.content.clone();
        tokio::spawn(async move {
            crate::tasks::room_comment_fanout(&pg, &realtime, &meta, &mid, &author, &content).await;
        });
    } else {
        index_channel_activity(
            state.pg.clone(),
            &id,
            &msg.id,
            &format!("#{nm} · {name}"),
            &msg.content,
        );
    }
    // An agent @mentioning a human notifies exactly like a human would.
    let mention_deps = notify.clone();
    let mention_name = name.clone();
    let mention_content = msg.content.clone();
    let channel_id = id.clone();
    tokio::spawn(async move {
        notify_user_mentions(
            &mention_deps,
            &channel_id,
            &mention_name,
            "",
            &describe_agent(&mention_name).label,
            &mention_content,
        )
        .await;
    });
    // The insert's own RETURNING shape — no guard/editedAt keys (absent, not
    // null; decoration hasn't run).
    Json(json!({ "message": inserted_wire(&msg) })).into_response()
}

async fn post_as_user(state: &AppState, headers: &HeaderMap, id: &str, body: PostBody) -> Response {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match channel_role(&state.pg, &user.id, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on POST messages failed: {e}");
            return thrown_internal_error();
        }
    }
    // A thread reply hangs off a ROOT in this channel; replying to a reply
    // re-roots onto its thread (Slack semantics — threads never nest).
    let mut thread_root_id: Option<String> = None;
    if let Some(root_id) = &body.thread_root_id {
        let root = match get_channel_message(&state.pg, id, root_id).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[channels] thread root read failed: {e}");
                return thrown_internal_error();
            }
        };
        let Some(root) = root else {
            return house_error(StatusCode::BAD_REQUEST, "no such thread");
        };
        thread_root_id = Some(root.thread_root_id.unwrap_or(root.id));
    }
    let author = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    let uploads =
        match resolve_attachments(&state.pg, body.attachment_ids.as_deref().unwrap_or(&[])).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[channels] attachment resolve failed: {e}");
                return thrown_internal_error();
            }
        };
    let ref_chips = match resolve_refs(
        &state.pg,
        &RefUser {
            id: &user.id,
            email: user.email.as_deref(),
            name: user.name.as_deref(),
        },
        body.refs.as_deref().unwrap_or(&[]),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[channels] ref resolve failed: {e}");
            return thrown_internal_error();
        }
    };
    let attachments: Vec<Value> = uploads
        .iter()
        .map(|a| serde_json::to_value(a).unwrap_or(Value::Null))
        .chain(
            ref_chips
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or(Value::Null)),
        )
        .collect();
    // The relevance gate's structural arm needs the turn's shape (an
    // attachments-only message is a handoff) — captured before the insert
    // takes the vec.
    let trigger_attachments = attachments.len();
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let message = match insert_channel_message(
        &notify,
        id,
        "user",
        &author,
        &body.content,
        "complete",
        &Value::Array(attachments),
        thread_root_id.as_deref(),
    )
    .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[channels] post insert failed: {e}");
            return thrown_internal_error();
        }
    };

    // Agent replies + mention notifications run detached; the POST returns
    // at once.
    let channel_name = channel_name(&state.pg, id)
        .await
        .unwrap_or_else(|| "channel".into());
    // A task-room message is a COMMENT, not ambient channel chatter: it
    // indexes board-scoped with the ticket's href (a channel doc's
    // channelId-only payload would escape the board filter and point at a
    // rail the room never lists in), and the board is owed the comment
    // edges — activity line, badge event. Rail channels keep the channel
    // doc.
    if !body.content.trim().is_empty() {
        let room = crate::ticket_chat::ticket_for_room(&state.pg, id).await;
        match room {
            Some(meta) => {
                let pg = state.pg.clone();
                let realtime = notify.realtime.clone();
                let mid = message.id.clone();
                let room_author = author.clone();
                let room_content = body.content.clone();
                tokio::spawn(async move {
                    crate::tasks::room_comment_fanout(
                        &pg,
                        &realtime,
                        &meta,
                        &mid,
                        &room_author,
                        &room_content,
                    )
                    .await;
                });
            }
            None => {
                // Index into the ambient activity brain (retrieval on
                // demand later).
                index_channel_activity(
                    state.pg.clone(),
                    id,
                    &message.id,
                    &format!("#{channel_name} · {author}"),
                    &body.content,
                );
            }
        }
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let trigger_state = state.clone();
    let trigger_deps = notify.clone();
    let trigger_content = body.content.clone();
    let trigger_seq = message.seq;
    let trigger_root = thread_root_id.clone();
    let trigger_name = channel_name.clone();
    let trigger_id = id.to_string();
    tokio::spawn(async move {
        trigger_agent_replies(
            &trigger_state,
            &trigger_deps,
            &sb,
            &trigger_id,
            &trigger_name,
            &trigger_content,
            trigger_attachments,
            trigger_seq,
            trigger_root.as_deref(),
        )
        .await;
    });
    // A DM message notifies the peer outright (deduped while unread);
    // channel/relay messages notify only on @mention.
    let kind: Option<(String,)> =
        match sqlx::query_as("select kind from channels where id = $1::uuid")
            .bind(id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(r) => r,
            // A failure here 500s the route after the insert and the
            // detached triggers have already fired — the write stands.
            Err(e) => {
                tracing::error!("[channels] kind read failed: {e}");
                return thrown_internal_error();
            }
        };
    let sender_label = user.name.clone().unwrap_or_else(|| author.clone());
    if kind.as_ref().map(|(k,)| k.as_str()) == Some("dm") {
        let dm_deps = notify.clone();
        let dm_id = id.to_string();
        let dm_content = body.content.clone();
        let dm_sender = user.id.clone();
        let dm_label = sender_label.clone();
        tokio::spawn(async move {
            notify_dm_message(&dm_deps, &dm_id, &dm_sender, &dm_label, &dm_content).await;
        });
    } else {
        let mention_deps = notify;
        let mention_id = id.to_string();
        let mention_content = body.content.clone();
        let mention_sender = user.id.clone();
        let mention_label = sender_label;
        let mention_channel = channel_name.clone();
        tokio::spawn(async move {
            notify_user_mentions(
                &mention_deps,
                &mention_id,
                &mention_channel,
                &mention_sender,
                &mention_label,
                &mention_content,
            )
            .await;
        });
    }
    Json(json!({ "message": inserted_wire(&message) })).into_response()
}

async fn channel_name(pg: &sqlx::PgPool, id: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("select name from channels where id = $1::uuid")
        .bind(id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten()
}

/// The ambient copy of a channel message, fire-and-forget — indexing
/// errors are swallowed.
fn index_channel_activity(
    pg: sqlx::PgPool,
    channel_id: &str,
    message_id: &str,
    title: &str,
    text: &str,
) {
    let doc = IndexDoc {
        source_type: "channel".into(),
        source_id: message_id.to_string(),
        title: Some(title.to_string()),
        text: text.to_string(),
        payload: Some(
            vec![("channelId".to_string(), json!(channel_id))]
                .into_iter()
                .collect(),
        ),
        href: Some("/channels".into()),
    };
    tokio::spawn(async move {
        let qd = qdrant::real_deps();
        let ed = embed::real_deps();
        let _ = index_activity(&pg, &qd, &ed, &doc).await;
    });
}
