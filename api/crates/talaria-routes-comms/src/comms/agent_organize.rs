// POST /api/agent/organize — the agent tidies the conversation it is in:
// renames it, pins it, files it under a group, or puts it away.
//
// NO CONVERSATION PARAMETER, the same call `/api/agent/todo` and
// `/api/agent/ask` make, and for the same reason: the agent is mid-turn
// somewhere, Talaria knows where, and an id a model had to pass is an id it
// could pass stale, pass somebody else's, or forget. `live_conversation_id`
// resolves it from the turn that is actually streaming, so this door cannot
// be pointed at a thread the agent is not in.
//
// WHOSE LIST IT MOVES. The layout is per viewer (see
// `talaria_conversation_layout`'s header), and the agent is not a viewer — it
// has no list of its own and nobody would ever read one. So the row it writes
// is the conversation OWNER's: the person whose sidebar asked for the pin.
//
// WHY ARCHIVE LIVES HERE AT ALL, when `PATCH /api/conversations/{id}` refuses
// it and sends a person to DELETE. Because "we're done, put this away" is a
// thing people say to the agent in the session they are finishing, and the
// alternative is telling them to go and find the row themselves. The guard is
// in the tool's own description rather than in the status code: never unasked.
//
// THE FAN IS NOT OPTIONAL. A rename or a pin that nobody's open list hears
// about is a change the person has to reload to see, so this answers through
// the same conversation event the PATCH route sends.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::require_agent;
use talaria_body::{optional_boolean_member, parse, trimmed_string_member};
use talaria_chips::live_conversation_id;
use talaria_conversation_layout::{patch_from, write_layout};
use talaria_conversations::conversation_owner;
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{NotifyDeps, fan_conversation_event};
use talaria_state::AppState;

// doc: Rename, pin, group or put away the conversation this agent is in.

/// The three fields the layout row owns. Read only when one of them is
/// actually present: `patch_from` answers "nothing to change" for an empty
/// body, which is the wrong sentence for a call that carried a title.
const LAYOUT_KEYS: [&str; 3] = ["pinned", "group", "position"];

/// The one line the model relays. It is addressed to the PERSON whose list
/// just moved — their words for it, not the field names — and it names only
/// what actually changed, so an agent cannot claim a pin it did not make.
fn said(phrases: &[String]) -> String {
    let body = match phrases {
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        // Unreachable: a call with nothing in it never gets this far.
        [] => return String::new(),
    };
    let mut chars = body.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let title = if obj.contains_key("title") {
        match trimmed_string_member(obj, "title", 1, 120) {
            Ok(t) => Some(t),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    } else {
        None
    };
    let archived = match optional_boolean_member(obj, "archived") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let layout = if LAYOUT_KEYS.iter().any(|k| obj.contains_key(*k)) {
        match patch_from(obj) {
            Ok(p) => Some(p),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    } else {
        None
    };
    if title.is_none() && archived.is_none() && layout.is_none() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "nothing to change — give a title, pinned, group, position or archived",
        ));
    }
    let conversation_id = match live_conversation_id(&state.pg, &caller.model).await {
        Ok(Some(id)) => id,
        // The todo route's sentence, said the same way for the same reason: a
        // channel reply is one message, not a session to organise, and an
        // agent told so plainly once stops reaching for the tool there.
        Ok(None) => {
            return Ok(house_error(
                StatusCode::CONFLICT,
                "no conversation to organise — this tool works inside a chat, plan or work \
                 session, not in a channel or outside a turn",
            ));
        }
        Err(e) => return Ok(internal("[organize] conversation resolve failed", e)),
    };
    // The owner by id alone, no access check: the gate already ran — this key
    // is mid-turn in this conversation — and what is needed here is WHOSE list
    // the layout row belongs to.
    let owner = match conversation_owner(&state.pg, &conversation_id).await {
        Ok(Some(owner)) => owner,
        Ok(None) => {
            return Ok(house_error(
                StatusCode::NOT_FOUND,
                "that conversation is gone",
            ));
        }
        Err(e) => return Ok(internal("[organize] owner read failed", e)),
    };

    let mut changed: Vec<&str> = Vec::new();
    let mut phrases: Vec<String> = Vec::new();
    if let Some(title) = &title {
        if let Err(e) = sqlx::query("update conversations set title = $1 where id = $2::uuid")
            .bind(title)
            .bind(&conversation_id)
            .execute(&state.pg)
            .await
        {
            return Ok(internal("[organize] rename failed", e));
        }
        changed.push("title");
        phrases.push(format!("renamed it to {title}"));
    }
    if let Some(layout) = &layout {
        if let Err(e) = write_layout(&state.pg, &owner, &conversation_id, layout).await {
            return Ok(internal("[organize] layout write failed", e));
        }
        if let Some(pinned) = layout.pinned {
            changed.push("pinned");
            phrases.push(
                if pinned {
                    "pinned it to the top of the list"
                } else {
                    "unpinned it"
                }
                .to_string(),
            );
        }
        if let Some(group) = &layout.group {
            changed.push("group");
            phrases.push(match group {
                Some(name) => format!("filed it under {name}"),
                None => "took it out of its group".to_string(),
            });
        }
        if let Some(position) = layout.position {
            changed.push("position");
            phrases.push(match position {
                Some(_) => "moved it in your list".to_string(),
                None => "let it fall back into date order".to_string(),
            });
        }
    }
    if let Some(archived) = archived {
        if let Err(e) = sqlx::query(
            "update conversations set archived = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(archived)
        .bind(&conversation_id)
        .execute(&state.pg)
        .await
        {
            return Ok(internal("[organize] archive failed", e));
        }
        changed.push("archived");
        phrases.push(
            if archived {
                "put it away"
            } else {
                "brought it back"
            }
            .to_string(),
        );
    }
    fan_conversation_event(
        NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
        conversation_id,
    );
    Ok(Json(json!({ "ok": true, "changed": changed, "said": said(&phrases) })).into_response())
}
