// POST /api/agent/ask — the agent asks the person a question with real
// options, on the turn it is writing.
//
// AN OPTIONAL `conversationId`, the same shape `/api/agent/todo` takes. The
// agent is mid-turn somewhere and Talaria usually knows where — but one agent
// answers several people at once, and "the newest live turn" is then
// somebody else's. The turn's prompt carries the conversation id; a call that
// passes it lands there and nowhere else, and a call that does not is served
// only while exactly one turn could be meant (a 409 says so otherwise). An id
// naming a thread this agent is not in lands nothing.
//
// THE CHIP IS THE WHOLE MECHANISM. There is no question row and no answer
// endpoint — the chip's buttons send the chosen words through the composer's
// own send path, so the answer lands as an ordinary message and triggers the
// next turn exactly as typing it would. That is why this route stores nothing:
// see `talaria_agent_ask`'s header for why a separate record of the answer
// would be a second source of truth about what was said.
//
// A FAILED CHIP IS A FAILED ASK, unlike a failed plan chip. If the plan's chip
// does not land the plan is still saved and readable; if THIS one does not
// land, nobody was asked anything, and telling the agent it succeeded would
// leave it waiting for an answer to a question nobody ever saw.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_ask::{asked_sentence, chip, question_from};
use talaria_agent_auth::require_agent;
use talaria_body::parse;
use talaria_chips::{Target, conversation_hint, surface_tool_chips_in};
use talaria_error::{house_error, internal, object_or_400};
use talaria_state::AppState;

// doc: Ask the person a question with options, on the conversation this agent is in.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let question = match question_from(obj) {
        Ok(q) => q,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let redis = state.redis().await.ok();
    let hint = conversation_hint(&parsed);
    match surface_tool_chips_in(
        &state.pg,
        redis,
        &caller.model,
        None,
        hint.as_deref(),
        vec![chip(&question)],
    )
    .await
    {
        // Several turns and no conversation named: nobody was asked, and the
        // sentence says what would have resolved it.
        Ok(target @ Target::Ambiguous { .. }) => Ok(house_error(
            StatusCode::CONFLICT,
            &target.ambiguity().unwrap_or_default(),
        )),
        // No landing target means no live turn to hang a question on — a
        // channel reply, or a call from outside a turn. Said plainly: an agent
        // told it has nobody to ask stops reaching for the tool there.
        Ok(Target::Nowhere) => Ok(house_error(
            StatusCode::CONFLICT,
            "nobody to ask — this tool works inside a chat, plan or work session where somebody \
             is listening, not in a channel or outside a turn",
        )),
        Ok(Target::Found(_)) => Ok(Json(json!({
            "ok": true,
            "asked": asked_sentence(&question),
        }))
        .into_response()),
        Err(e) => Ok(internal("[ask] chip surface failed", e)),
    }
}
