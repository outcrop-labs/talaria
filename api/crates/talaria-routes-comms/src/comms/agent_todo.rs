// PUT /api/agent/todo — the agent writes down what it is going to do.
//
// NO CONVERSATION PARAMETER, deliberately, and this is the same call the
// research tool already makes for the same reason: the agent is speaking in a
// conversation right now, Talaria knows which one, and a model that had to
// pass the id could pass a stale one, pass somebody else's, or forget. The
// best fix for a mistake is to make it unavailable. `live_conversation_id`
// resolves it from the turn that is actually streaming.
//
// WHY A WRITE WITH NO READ BESIDE IT. The surface reads the plan through
// `GET /api/conversations/{id}/todo`, which is gated on the conversation's own
// membership. This door is agent-key only and writes to whichever conversation
// the key is mid-turn in — so it needs no id to check, and cannot be pointed
// at a thread the agent is not in.
//
// THE CHIP IS THE POINT. Storing the plan makes it readable; landing a chip
// makes it VISIBLE, in the stream, at the moment it changed. Both, or the
// feature is a table nobody looks at.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::require_agent;
use talaria_agent_todo::{items_from, items_json, save, summary};
use talaria_body::parse;
use talaria_chips::{live_conversation_id, surface_tool_chips};
use talaria_error::{house_error, internal, object_or_400};
use talaria_state::AppState;

// doc: Write down the steps this agent is working through, on the conversation it is in.

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let items = match items_from(obj) {
        Ok(i) => i,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let conversation_id = match live_conversation_id(&state.pg, &caller.model).await {
        Ok(Some(id)) => id,
        // A CHANNEL TURN, OR NO LIVE TURN AT ALL. Said plainly rather than
        // stored somewhere it cannot be read: a plan belongs to a thread, and
        // a channel reply is one message rather than a session worth
        // narrating. An agent told this once stops reaching for the tool there.
        Ok(None) => {
            return Ok(house_error(
                StatusCode::CONFLICT,
                "no conversation to attach a plan to — this tool works inside a chat, plan or \
                 work session, not in a channel or outside a turn",
            ));
        }
        Err(e) => return Ok(internal("[todo] conversation resolve failed", e)),
    };
    if let Err(e) = save(&state.pg, &conversation_id, &items, &caller.model).await {
        return Ok(internal("[todo] save failed", e));
    }
    // The chip carries the WHOLE list, not a delta: the stream is read
    // top-to-bottom and a reader scrolling back to an older chip should see
    // the plan as it stood then, not a diff they have to replay.
    let wire = items_json(&items);
    let chip = json!({
        "id": "todo",
        "kind": "todo",
        "label": summary(&items),
        "items": wire,
    });
    let redis = state.redis().await.ok();
    if let Err(e) = surface_tool_chips(&state.pg, redis, &caller.model, None, vec![chip]).await {
        // The plan IS saved; only its chip failed to land. Reporting a failure
        // here would tell the agent to write the plan again, which would
        // duplicate nothing and fix nothing.
        tracing::warn!("[todo] chip surface failed: {e}");
    }
    Ok(Json(json!({ "ok": true, "summary": summary(&items), "items": wire })).into_response())
}
