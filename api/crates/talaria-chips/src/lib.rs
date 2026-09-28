// Chat chips — the record a conversation or channel message carries when an
// agent shares a platform link, exposes a tool, or attempts something that
// leaves the chat. Recognition and extraction are pure; landing a chip is
// the one write every caller shares, so a tool result and a confirm-send
// cannot invent two different shapes.

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_notify::{NotifyDeps, fan_channel_event, fan_conversation_event};
use talaria_realtime::{ChannelEvent, RealtimeDeps, publish_channel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformLink {
    pub href: String,
    pub entity: &'static str,
    pub id: String,
    pub board_id: Option<String>,
}

/// Tools an approval unlocks. The UI copy of this map lives in
/// `ui/src/lib/chips.ts` (`toolsUnlockedBy`) — the two lists are the contract.
pub fn tools_unlocked(kind: &str) -> &'static [&'static str] {
    match kind {
        "gmail_send" => &["draft_email"],
        "calendar_create" => &["draft_calendar_event"],
        "ticket_move" => &["triage_ticket"],
        "board_access" => &["join_board"],
        _ => &[],
    }
}

fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| matches!(i, 8 | 13 | 18 | 23) || c.is_ascii_hexdigit())
}

/// Path-shaped recognition. An absolute URL keeps only its path and query.
/// Unrecognized URLs return None — the caller leaves them as links.
pub fn classify_href(raw: &str) -> Option<PlatformLink> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("mention:")
        || trimmed.to_ascii_lowercase().starts_with("javascript:")
    {
        return None;
    }
    let path = if let Some(rest) = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
    {
        let after_host = rest.find('/')?;
        &rest[after_host..]
    } else if trimmed.starts_with('/') {
        trimmed
    } else {
        return None;
    };
    let (pathname, query) = path.split_once('?').unwrap_or((path, ""));
    let pathname = pathname.trim_end_matches('/');
    let seg: Vec<&str> = pathname.split('/').filter(|s| !s.is_empty()).collect();
    let param = |key: &str| {
        query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key && is_id(v)).then(|| v.to_string())
        })
    };

    if seg.first() == Some(&"boards") && seg.len() == 3 && is_id(seg[1]) && is_id(seg[2]) {
        return Some(PlatformLink {
            href: format!("/boards/{}/{}", seg[1], seg[2]),
            entity: "ticket",
            id: seg[2].to_string(),
            board_id: Some(seg[1].to_string()),
        });
    }
    if seg.first() == Some(&"boards") && seg.len() == 2 && is_id(seg[1]) {
        return Some(PlatformLink {
            href: format!("/boards/{}", seg[1]),
            entity: "board",
            id: seg[1].to_string(),
            board_id: None,
        });
    }
    if seg.first() == Some(&"knowledge") {
        if let Some(doc) = param("doc").or_else(|| param("d")) {
            return Some(PlatformLink {
                href: format!("/knowledge?doc={doc}"),
                entity: "kb",
                id: doc,
                board_id: None,
            });
        }
        if seg.len() == 3 && is_id(seg[1]) && is_id(seg[2]) {
            return Some(PlatformLink {
                href: format!("/knowledge/{}/{}", seg[1], seg[2]),
                entity: "kb",
                id: seg[2].to_string(),
                board_id: None,
            });
        }
    }
    if seg.first() == Some(&"artifacts")
        && let Some(id) = param("a")
    {
        return Some(PlatformLink {
            href: format!("/artifacts?a={id}"),
            entity: "document",
            id,
            board_id: None,
        });
    }
    if seg.first() == Some(&"comms")
        && seg.get(1) == Some(&"channel")
        && seg.len() == 3
        && is_id(seg[2])
    {
        return Some(PlatformLink {
            href: format!("/comms/channel/{}", seg[2]),
            entity: "channel",
            id: seg[2].to_string(),
            board_id: None,
        });
    }
    if seg.first() == Some(&"plan") {
        let id = if seg.len() == 2 && is_id(seg[1]) {
            Some(seg[1].to_string())
        } else {
            param("p")
        };
        if let Some(id) = id {
            return Some(PlatformLink {
                href: format!("/plan/{id}"),
                entity: "plan",
                id,
                board_id: None,
            });
        }
    }
    if seg.first() == Some(&"research") {
        let id = if seg.len() == 2 && is_id(seg[1]) {
            Some(seg[1].to_string())
        } else {
            param("r")
        };
        if let Some(id) = id {
            return Some(PlatformLink {
                href: format!("/research/{id}"),
                entity: "research",
                id,
                board_id: None,
            });
        }
    }
    None
}

fn link_chip(link: &PlatformLink, title: Option<&str>) -> Value {
    json!({
        "id": format!("link:{}", link.href),
        "kind": "link",
        "href": link.href,
        "entity": link.entity,
        "title": title.filter(|t| !t.is_empty()),
    })
}

fn approval_chip(action_id: &str, kind: &str, summary: &str) -> Value {
    json!({
        "id": format!("approval:{action_id}"),
        "kind": "approval",
        "actionId": action_id,
        "actionKind": kind,
        "summary": summary,
        "status": "pending",
    })
}

pub fn unlock_chip(approval_id: &str, kind: &str) -> Value {
    json!({
        "id": format!("unlock:{approval_id}"),
        "kind": "unlock",
        "approvalId": approval_id,
        "actionKind": kind,
        "tools": tools_unlocked(kind),
        "status": "approved",
    })
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn walk_links(v: &Value, title: Option<&str>, out: &mut Vec<Value>) {
    match v {
        Value::String(s) => {
            if let Some(link) = classify_href(s) {
                out.push(link_chip(&link, title));
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_links(item, title, out);
            }
        }
        Value::Object(map) => {
            let sibling = str_field(v, "title").or(str_field(v, "name")).or(title);
            if let (Some(board), Some(id)) = (str_field(v, "boardId"), str_field(v, "id"))
                && is_id(board)
                && is_id(id)
            {
                let link = PlatformLink {
                    href: format!("/boards/{board}/{id}"),
                    entity: "ticket",
                    id: id.to_string(),
                    board_id: Some(board.to_string()),
                };
                out.push(link_chip(&link, sibling.or(str_field(v, "ticketRef"))));
            }
            for (k, child) in map {
                if k == "boardId" || k == "id" {
                    continue;
                }
                walk_links(child, sibling, out);
            }
        }
        _ => {}
    }
}

fn first_id(v: &Value) -> Option<&str> {
    str_field(v, "id")
        .or_else(|| v.get("artifact").and_then(|a| str_field(a, "id")))
        .or_else(|| v.get("doc").and_then(|a| str_field(a, "id")))
        .or_else(|| v.get("task").and_then(|a| str_field(a, "id")))
        .or_else(|| v.get("document").and_then(|a| str_field(a, "id")))
}

fn first_title(v: &Value) -> Option<&str> {
    str_field(v, "title")
        .or_else(|| v.get("artifact").and_then(|a| str_field(a, "title")))
        .or_else(|| v.get("doc").and_then(|a| str_field(a, "title")))
        .or_else(|| v.get("task").and_then(|a| str_field(a, "title")))
        .or_else(|| str_field(v, "name"))
}

/// Chips implied by one completed tool call. Link-producing tools always
/// emit a link chip, even when the result text never pasted the URL.
/// Protected tools emit an approval chip instead of pretending the action
/// already happened.
pub fn chips_from_tool(tool: &str, args: &str, result: &str) -> Vec<Value> {
    let args_v = serde_json::from_str::<Value>(args).unwrap_or(Value::Null);
    let result_v = serde_json::from_str::<Value>(result).unwrap_or(Value::Null);
    let mut out = Vec::new();
    walk_links(&result_v, None, &mut out);
    if let Some(link) = classify_href(result) {
        out.push(link_chip(&link, None));
    }

    let title = first_title(&result_v).or_else(|| str_field(&args_v, "title"));
    let id = first_id(&result_v);
    match tool {
        "create_document"
        | "create_sheet"
        | "create_page"
        | "save_image_artifact"
        | "update_document" => {
            if let Some(id) = id.filter(|s| is_id(s)) {
                out.push(link_chip(
                    &PlatformLink {
                        href: format!("/artifacts?a={id}"),
                        entity: "document",
                        id: id.to_string(),
                        board_id: None,
                    },
                    title,
                ));
            }
        }
        "create_kb_doc" | "edit_kb_doc" => {
            if let Some(id) = id.filter(|s| is_id(s)) {
                out.push(link_chip(
                    &PlatformLink {
                        href: format!("/knowledge?doc={id}"),
                        entity: "kb",
                        id: id.to_string(),
                        board_id: None,
                    },
                    title,
                ));
            }
        }
        "post_to_channel" => {
            if let Some(id) = str_field(&args_v, "channelId").filter(|s| is_id(s)) {
                out.push(link_chip(
                    &PlatformLink {
                        href: format!("/comms/channel/{id}"),
                        entity: "channel",
                        id: id.to_string(),
                        board_id: None,
                    },
                    str_field(&args_v, "name"),
                ));
            }
        }
        "research" => {
            if let Some(id) = id.filter(|s| is_id(s)) {
                out.push(link_chip(
                    &PlatformLink {
                        href: format!("/research/{id}"),
                        entity: "research",
                        id: id.to_string(),
                        board_id: None,
                    },
                    title.or(str_field(&args_v, "question")),
                ));
            }
        }
        "draft_email" | "draft_calendar_event" => {
            if let Some(pending) = result_v.get("pending").and_then(|p| str_field(p, "id")) {
                let kind = if tool == "draft_email" {
                    "gmail_send"
                } else {
                    "calendar_create"
                };
                let summary = str_field(&result_v, "message").unwrap_or(if tool == "draft_email" {
                    "Send email"
                } else {
                    "Create event"
                });
                out.push(approval_chip(pending, kind, summary));
            }
        }
        "triage_ticket" => {
            let refused = result.contains("cannot") || result_v.get("error").is_some();
            if refused {
                let task = str_field(&args_v, "taskId").unwrap_or("");
                let status = str_field(&args_v, "status").unwrap_or("move");
                if is_id(task) {
                    let external = format!("ticket:{task}:{status}");
                    let mut chip = approval_chip(
                        &external,
                        "ticket_move",
                        &format!("Move ticket to {status}"),
                    );
                    if let Some(obj) = chip.as_object_mut() {
                        obj.insert("payload".into(), args_v.clone());
                    }
                    out.push(chip);
                }
            }
        }
        "request_board_access" => {
            if let Some(board) = str_field(&args_v, "boardId").filter(|s| is_id(s)) {
                let mut chip = approval_chip(
                    &format!("board:{board}"),
                    "board_access",
                    "Request access to a board",
                );
                if let Some(obj) = chip.as_object_mut() {
                    obj.insert("payload".into(), args_v.clone());
                }
                out.push(chip);
            }
        }
        _ => {}
    }
    dedupe(out)
}

fn dedupe(chips: Vec<Value>) -> Vec<Value> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for chip in chips {
        let id = chip
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if id.is_empty() || seen.iter().any(|s| s == &id) {
            continue;
        }
        seen.push(id);
        out.push(chip);
    }
    out
}

/// Append chips that are not already on the row. Identity is the chip id, so
/// a retried tool call does not stack a second copy of the same link.
pub fn merge_chips(existing: &Value, incoming: &[Value]) -> Value {
    let mut out = match existing {
        Value::Array(items) => items.clone(),
        _ => Vec::new(),
    };
    for chip in incoming {
        let id = chip.get("id").and_then(Value::as_str).unwrap_or("");
        if id.is_empty()
            || out
                .iter()
                .any(|c| c.get("id").and_then(Value::as_str) == Some(id))
        {
            continue;
        }
        out.push(chip.clone());
    }
    Value::Array(out)
}

#[derive(Debug, Clone)]
pub struct Landed {
    pub conversation_id: Option<String>,
    pub channel_id: Option<String>,
    pub message_id: String,
}

#[derive(Debug, Clone)]
pub struct ApprovalRow {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub payload: Value,
    pub external_id: Option<String>,
    pub status: String,
    pub owner_user_id: Option<String>,
    pub conversation_id: Option<String>,
    pub channel_id: Option<String>,
    pub message_id: Option<String>,
    pub agent_model: Option<String>,
}

/// Where an agent's chips should land, in tier order: the conversation turn
/// it is streaming, else the channel turn it is streaming, else its newest
/// channel message of the last 15 minutes regardless of status, else its
/// newest assistant conversation message of the last 15 minutes regardless
/// of status. None when no tier hits — the agent has no live turn, and
/// chips with nowhere correct to land are dropped.
async fn speaking_target(pg: &PgPool, agent: &str) -> Result<Option<Landed>, sqlx::Error> {
    let conv: Option<(String, String)> = sqlx::query_as(
        "select m.id::text, m.conversation_id::text \
         from messages m join conversations c on c.id = m.conversation_id \
         where c.agent_model = $1 and m.role = 'assistant' and m.status = 'streaming' \
           and m.streamed_at > now() - interval '15 minutes' \
         order by m.streamed_at desc limit 1",
    )
    .bind(agent)
    .fetch_optional(pg)
    .await?;
    if let Some((message_id, conversation_id)) = conv {
        return Ok(Some(Landed {
            conversation_id: Some(conversation_id),
            channel_id: None,
            message_id,
        }));
    }
    let channel: Option<(String, String)> = sqlx::query_as(
        "select id::text, channel_id::text from channel_messages \
         where author_type = 'agent' and author = $1 and status = 'streaming' \
           and created_at > now() - interval '15 minutes' \
         order by created_at desc limit 1",
    )
    .bind(agent)
    .fetch_optional(pg)
    .await?;
    if let Some((message_id, channel_id)) = channel {
        return Ok(Some(Landed {
            conversation_id: None,
            channel_id: Some(channel_id),
            message_id,
        }));
    }
    let channel_recent: Option<(String, String)> = sqlx::query_as(
        "select id::text, channel_id::text from channel_messages \
         where author_type = 'agent' and author = $1 \
           and created_at > now() - interval '15 minutes' \
         order by created_at desc limit 1",
    )
    .bind(agent)
    .fetch_optional(pg)
    .await?;
    if let Some((message_id, channel_id)) = channel_recent {
        return Ok(Some(Landed {
            conversation_id: None,
            channel_id: Some(channel_id),
            message_id,
        }));
    }
    let latest: Option<(String, String)> = sqlx::query_as(
        "select m.id::text, m.conversation_id::text \
         from messages m join conversations c on c.id = m.conversation_id \
         where c.agent_model = $1 and m.role = 'assistant' \
           and m.created_at > now() - interval '15 minutes' \
         order by m.created_at desc limit 1",
    )
    .bind(agent)
    .fetch_optional(pg)
    .await?;
    Ok(latest.map(|(message_id, conversation_id)| Landed {
        conversation_id: Some(conversation_id),
        channel_id: None,
        message_id,
    }))
}

async fn write_chips(pg: &PgPool, landed: &Landed, chips: &[Value]) -> Result<(), sqlx::Error> {
    if chips.is_empty() {
        return Ok(());
    }
    let incoming = Value::Array(chips.to_vec());
    if landed.channel_id.is_some() {
        sqlx::query(
            "update channel_messages set chips = ( \
               select coalesce(jsonb_agg(c), '[]'::jsonb) from ( \
                 select c from jsonb_array_elements(chips) c \
                 union all \
                 select c from jsonb_array_elements($2::jsonb) c \
                 where not exists ( \
                   select 1 from jsonb_array_elements(chips) e \
                   where e->>'id' = c->>'id' \
                 ) \
               ) s \
             ) where id = $1::uuid",
        )
        .bind(&landed.message_id)
        .bind(&incoming)
        .execute(pg)
        .await?;
    } else {
        sqlx::query(
            "update messages set chips = ( \
               select coalesce(jsonb_agg(c), '[]'::jsonb) from ( \
                 select c from jsonb_array_elements(chips) c \
                 union all \
                 select c from jsonb_array_elements($2::jsonb) c \
                 where not exists ( \
                   select 1 from jsonb_array_elements(chips) e \
                   where e->>'id' = c->>'id' \
                 ) \
               ) s \
             ) where id = $1::uuid",
        )
        .bind(&landed.message_id)
        .bind(&incoming)
        .execute(pg)
        .await?;
    }
    Ok(())
}

fn fan(pg: &PgPool, redis: Option<redis::aio::ConnectionManager>, landed: &Landed) {
    let deps = NotifyDeps::publishing(pg.clone(), redis.clone());
    if let Some(id) = &landed.conversation_id {
        fan_conversation_event(deps, id.clone());
    }
    if let Some(id) = &landed.channel_id {
        let realtime = RealtimeDeps::publish_only(redis);
        publish_channel(
            &realtime,
            id,
            &ChannelEvent {
                kind_tag: "message",
                message_id: Some(landed.message_id.clone()),
                seq: None,
                deleted: None,
            },
        );
        fan_channel_event(NotifyDeps::publishing(pg.clone(), None), id.clone());
    }
}

/// Attach chips to the agent's live turn and tell the open thread to refetch.
pub async fn surface_for_agent(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    agent: &str,
    chips: Vec<Value>,
) -> Result<Option<Landed>, sqlx::Error> {
    if chips.is_empty() || agent.is_empty() {
        return Ok(None);
    }
    let Some(landed) = speaking_target(pg, agent).await? else {
        return Ok(None);
    };
    write_chips(pg, &landed, &chips).await?;
    fan(pg, redis, &landed);
    Ok(Some(landed))
}

/// Idempotent pending row. `external_id` is the google pending id, or a
/// stable key for a ticket move (`ticket:{id}:{status}`). A retry returns
/// the row that is already waiting. The arguments are the row's columns.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_approval(
    pg: &PgPool,
    kind: &str,
    summary: &str,
    payload: &Value,
    external_id: &str,
    agent: &str,
    owner_user_id: Option<&str>,
    landed: Option<&Landed>,
) -> Result<String, sqlx::Error> {
    let existing: Option<String> = sqlx::query_scalar(
        "select id::text from chat_approvals where external_id = $1 and status = 'pending' limit 1",
    )
    .bind(external_id)
    .fetch_optional(pg)
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let (conversation_id, channel_id, message_id) = match landed {
        Some(l) => (
            l.conversation_id.as_deref(),
            l.channel_id.as_deref(),
            Some(l.message_id.as_str()),
        ),
        None => (None, None, None),
    };
    let (id,): (String,) = sqlx::query_as(
        "insert into chat_approvals \
           (kind, summary, payload, external_id, agent_model, owner_user_id, conversation_id, channel_id, message_id) \
         values ($1, $2, $3, $4, $5, $6::uuid, $7::uuid, $8::uuid, $9::uuid) \
         returning id::text",
    )
    .bind(kind)
    .bind(summary)
    .bind(payload)
    .bind(external_id)
    .bind(agent)
    .bind(owner_user_id)
    .bind(conversation_id)
    .bind(channel_id)
    .bind(message_id)
    .fetch_one(pg)
    .await?;
    Ok(id)
}

pub async fn load_approval(pg: &PgPool, id: &str) -> Result<Option<ApprovalRow>, sqlx::Error> {
    let row: Option<(
        String,
        String,
        String,
        Value,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "select id::text, kind, summary, payload, external_id, status, owner_user_id::text, \
                conversation_id::text, channel_id::text, message_id::text, agent_model \
         from chat_approvals where id = $1::uuid or external_id = $1 limit 1",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|r| ApprovalRow {
        id: r.0,
        kind: r.1,
        summary: r.2,
        payload: r.3,
        external_id: r.4,
        status: r.5,
        owner_user_id: r.6,
        conversation_id: r.7,
        channel_id: r.8,
        message_id: r.9,
        agent_model: r.10,
    }))
}

pub async fn mark_approval(
    pg: &PgPool,
    id: &str,
    status: &str,
    decided_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update chat_approvals set status = $2, decided_by = $3::uuid, decided_at = now() where id = $1::uuid",
    )
    .bind(id)
    .bind(status)
    .bind(decided_by)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn merge_unlocked(
    pg: &PgPool,
    conversation_id: &str,
    tools: &[&str],
) -> Result<(), sqlx::Error> {
    if tools.is_empty() {
        return Ok(());
    }
    let incoming = json!(tools);
    sqlx::query(
        "update conversations set unlocked_tools = ( \
           select coalesce(jsonb_agg(distinct t), '[]'::jsonb) from ( \
             select t from jsonb_array_elements_text(unlocked_tools) t \
             union all \
             select t from jsonb_array_elements_text($2::jsonb) t \
           ) s \
         ) where id = $1::uuid",
    )
    .bind(conversation_id)
    .bind(&incoming)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn tool_unlocked(
    pg: &PgPool,
    conversation_id: &str,
    tool: &str,
) -> Result<bool, sqlx::Error> {
    let hit: Option<bool> =
        sqlx::query_scalar("select unlocked_tools ? $2 from conversations where id = $1::uuid")
            .bind(conversation_id)
            .bind(tool)
            .fetch_optional(pg)
            .await?;
    Ok(hit.unwrap_or(false))
}

pub async fn live_conversation_id(pg: &PgPool, agent: &str) -> Result<Option<String>, sqlx::Error> {
    Ok(speaking_target(pg, agent)
        .await?
        .and_then(|l| l.conversation_id))
}

/// Rewrite approval chips so their actionId is the chat_approvals row, then
/// land them. Callers that already queued a google action pass that id as
/// the chip's actionId; this function makes the decide endpoint one door.
pub async fn surface_tool_chips(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    agent: &str,
    owner_user_id: Option<&str>,
    mut chips: Vec<Value>,
) -> Result<Option<Landed>, sqlx::Error> {
    if chips.is_empty() {
        return Ok(None);
    }
    let landed = speaking_target(pg, agent).await?;
    for chip in &mut chips {
        if chip.get("kind").and_then(Value::as_str) != Some("approval") {
            continue;
        }
        let kind = chip
            .get("actionKind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let summary = chip
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let external = chip
            .get("actionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if external.is_empty() {
            continue;
        }
        let payload = chip
            .get("payload")
            .cloned()
            .unwrap_or_else(|| json!({ "externalId": external }));
        let id = ensure_approval(
            pg,
            &kind,
            &summary,
            &payload,
            &external,
            agent,
            owner_user_id,
            landed.as_ref(),
        )
        .await?;
        if let Some(obj) = chip.as_object_mut() {
            obj.insert("actionId".into(), json!(id));
            obj.insert("id".into(), json!(format!("approval:{id}")));
        }
    }
    let Some(landed) = landed else {
        return Ok(None);
    };
    write_chips(pg, &landed, &chips).await?;
    fan(pg, redis, &landed);
    Ok(Some(landed))
}

#[derive(Debug, Clone)]
pub struct ResolvedTitle {
    pub href: String,
    pub entity: &'static str,
    pub title: Option<String>,
}

/// Titles the caller may see. A miss is an absent title, never another
/// person's private name — the chip falls back to the entity label.
pub async fn resolve_titles(
    pg: &PgPool,
    user_id: &str,
    hrefs: &[String],
) -> Result<Vec<ResolvedTitle>, sqlx::Error> {
    let mut out = Vec::new();
    for href in hrefs {
        let Some(link) = classify_href(href) else {
            continue;
        };
        let title = lookup_title(pg, user_id, &link).await?;
        out.push(ResolvedTitle {
            href: link.href,
            entity: link.entity,
            title,
        });
    }
    Ok(out)
}

async fn lookup_title(
    pg: &PgPool,
    user_id: &str,
    link: &PlatformLink,
) -> Result<Option<String>, sqlx::Error> {
    // Fixture-shaped ids (b-1) are not rows. Only a uuid is worth a lookup.
    if !is_uuid(&link.id) {
        return Ok(None);
    }
    let title: Option<String> = match link.entity {
        "board" => {
            sqlx::query_scalar(
                "select b.name from boards b \
                 where b.id = $1::uuid and (b.owner_id = $2::uuid or exists( \
                   select 1 from board_members m where m.board_id = b.id and m.user_id = $2::uuid))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "ticket" => {
            sqlx::query_scalar(
                "select t.title from tasks t join boards b on b.id = t.board_id \
                 where t.id = $1::uuid and (b.owner_id = $2::uuid or exists( \
                   select 1 from board_members m where m.board_id = b.id and m.user_id = $2::uuid))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "kb" => {
            sqlx::query_scalar(
                "select title from kb_docs where id = $1::uuid and visibility in ('org', 'public')",
            )
            .bind(&link.id)
            .fetch_optional(pg)
            .await?
        }
        "document" => {
            sqlx::query_scalar(
                "select title from artifacts \
                 where id = $1::uuid and (owner_user_id = $2::uuid or visibility in ('org', 'public', 'workspace'))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "channel" => {
            sqlx::query_scalar(
                "select c.name from channels c \
                 where c.id = $1::uuid and exists( \
                   select 1 from channel_members m where m.channel_id = c.id and m.user_id = $2::uuid)",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "plan" => {
            sqlx::query_scalar(
                "select coalesce(title, 'Plan') from conversations \
                 where id = $1::uuid and kind = 'plan' and (user_id = $2::uuid or exists( \
                   select 1 from conversation_members cm where cm.conversation_id = conversations.id and cm.user_id = $2::uuid))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "research" => {
            sqlx::query_scalar(
                "select question from research_runs \
                 where id = $1::uuid and (owner_user_id = $2::uuid or exists( \
                   select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = $2::uuid))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        _ => None,
    };
    Ok(title.filter(|t| !t.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_entities_agents_share_and_rejects_the_rest() {
        assert_eq!(classify_href("/boards/b-1").unwrap().entity, "board");
        let ticket = classify_href("https://talaria.example/boards/b-1/t-2").unwrap();
        assert_eq!(ticket.entity, "ticket");
        assert_eq!(ticket.href, "/boards/b-1/t-2");
        assert_eq!(classify_href("/knowledge?doc=doc-9").unwrap().entity, "kb");
        assert_eq!(
            classify_href("/artifacts?a=file-3").unwrap().entity,
            "document"
        );
        assert_eq!(
            classify_href("/comms/channel/chan-1").unwrap().entity,
            "channel"
        );
        assert!(classify_href("https://example.com/pricing").is_none());
        assert!(classify_href("/admin").is_none());
        assert!(classify_href("/boards").is_none());
        assert!(classify_href("javascript:alert(1)").is_none());
    }

    #[test]
    fn a_created_ticket_becomes_a_link_chip_without_a_pasted_url() {
        let chips = chips_from_tool(
            "create_ticket",
            r#"{"boardId":"b-1","title":"Fix the gate"}"#,
            r#"{"task":{"id":"t-2","boardId":"b-1","title":"Fix the gate"}}"#,
        );
        assert!(
            chips
                .iter()
                .any(|c| c["kind"] == "link" && c["href"] == "/boards/b-1/t-2")
        );
        assert_eq!(
            chips
                .iter()
                .filter(|c| c["href"] == "/boards/b-1/t-2")
                .count(),
            1
        );
    }

    #[test]
    fn a_drafted_email_becomes_an_approval_chip() {
        let chips = chips_from_tool(
            "draft_email",
            r#"{"to":"a@b.c"}"#,
            r#"{"pending":{"id":"p-1","status":"pending"},"message":"Drafted — waiting"}"#,
        );
        assert_eq!(chips[0]["kind"], "approval");
        assert_eq!(chips[0]["actionKind"], "gmail_send");
        assert_eq!(chips[0]["actionId"], "p-1");
    }

    #[test]
    fn a_refused_move_becomes_an_approval_and_a_success_does_not() {
        let refused = chips_from_tool(
            "triage_ticket",
            r#"{"taskId":"t-2","status":"done"}"#,
            r#"{"error":"agents cannot take a ticket out of review"}"#,
        );
        assert_eq!(refused[0]["actionKind"], "ticket_move");
        let ok = chips_from_tool(
            "triage_ticket",
            r#"{"taskId":"t-2","status":"in_progress"}"#,
            r#"{"task":{"id":"t-2","boardId":"b-1","title":"Gate"}}"#,
        );
        assert!(ok.iter().all(|c| c["kind"] != "approval"));
    }

    #[test]
    fn merge_does_not_stack_the_same_chip() {
        let existing = json!([{ "id": "link:/boards/b-1", "kind": "link" }]);
        let incoming = vec![
            json!({ "id": "link:/boards/b-1", "kind": "link" }),
            json!({ "id": "approval:p-1", "kind": "approval" }),
        ];
        let merged = merge_chips(&existing, &incoming);
        assert_eq!(merged.as_array().unwrap().len(), 2);
    }

    #[test]
    fn unlock_names_the_tools_the_approval_granted() {
        assert_eq!(tools_unlocked("gmail_send"), ["draft_email"]);
        assert_eq!(tools_unlocked("ticket_move"), ["triage_ticket"]);
        assert!(tools_unlocked("nope").is_empty());
        assert_eq!(unlock_chip("a-1", "gmail_send")["tools"][0], "draft_email");
    }

    #[test]
    fn a_created_sheet_becomes_one_document_link_chip() {
        let chips = chips_from_tool(
            "create_sheet",
            r#"{"title":"Q3 numbers"}"#,
            r#"{"id":"9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2","title":"Q3 numbers"}"#,
        );
        assert_eq!(chips.len(), 1, "no stray chips: {chips:?}");
        assert_eq!(chips[0]["kind"], "link");
        assert_eq!(chips[0]["entity"], "document");
        assert_eq!(
            chips[0]["href"],
            "/artifacts?a=9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2"
        );
    }

    #[test]
    fn a_board_access_request_carries_its_payload() {
        let chips = chips_from_tool("request_board_access", r#"{"boardId":"b-9"}"#, "{}");
        assert_eq!(chips.len(), 1, "no stray chips: {chips:?}");
        assert_eq!(chips[0]["kind"], "approval");
        assert_eq!(chips[0]["actionKind"], "board_access");
        assert_eq!(chips[0]["actionId"], "board:b-9");
        assert_eq!(chips[0]["payload"], json!({ "boardId": "b-9" }));
    }
}
