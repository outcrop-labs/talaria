// /api/inbox/focus/command — port of ui/src/routes/api/inbox.focus.command.ts.
// POST → run one instruction from the focus inbox panel through the assistant
// (normal / fast / plan mode, optional model overrides), as an SSE stream of
// named events — conversation, status, content, activity, done, error.
//
// This route lives in the STREAMING router: the stream's legitimate lifetime
// is the turn itself, not a handler window. The Inbox lock MOVES into the
// stream task (`let _guard`), so it releases when the stream does — TS's
// `finally { release() }` expressed as an ownership move.
//
// On a client disconnect TS stops consuming, the generator is returned at its
// current yield, and the assistant row is left 'streaming'. The port's run
// keeps going and persists the completed reply (the chat family's tee
// philosophy) — the recorded divergence, documented at the module.

use crate::body::{
    array_msg, array_too_big_msg, as_object, enum_member, nullable_optional_string_member,
    nullish_max_string_member, object_msg, optional_enum_member, optional_uuid_array_member,
    optional_uuid_member, uuid_member, zod_type_name,
};
use crate::error::house_error;
use crate::inbox_focus::FocusError;
use crate::inbox_focus_conversation::{
    InboxCommandInput, acquire_inbox_focus_lock, run_inbox_conversation_command,
};
use crate::inbox_focus_types::InboxCommandEvent;
use crate::refs::MessageRef;
use crate::session::require_user;
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde_json::Value;

fn validate(obj: &serde_json::Map<String, Value>) -> Result<InboxCommandInput, String> {
    let focus_key = nullable_optional_string_member(obj, "key", 600)?;
    let surface = nullish_max_string_member(obj, "surface", 40)?;
    let instruction = crate::body::trimmed_string_member(obj, "instruction", 1, 20_000)?;
    let delegate_model = nullish_max_string_member(obj, "delegateModel", 300)?;
    let response_model = nullish_max_string_member(obj, "responseModel", 300)?;
    let mode = optional_enum_member(obj, "mode", &["normal", "fast", "plan"])?
        .unwrap_or_else(|| "normal".into());
    let conversation_id = optional_uuid_member(obj, "conversationId")?;
    let effort = nullish_max_string_member(obj, "effort", 24)?;
    let attachment_ids = optional_uuid_array_member(obj, "attachmentIds", 12)?.unwrap_or_default();
    let refs = match obj.get("refs") {
        None => Vec::new(),
        Some(v) => {
            let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
            if arr.len() > 6 {
                return Err(array_too_big_msg(6));
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
            out
        }
    };
    Ok(InboxCommandInput {
        instruction,
        focus_key,
        surface,
        delegate_model,
        response_model,
        mode,
        effort,
        attachment_ids,
        refs,
        conversation_id,
    })
}

/// `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n` — the frame
/// shape the panel's SSE reader parses. The event's own serialization
/// carries its `type` tag inside the data payload too, as in TS.
fn frame(event: &InboxCommandEvent) -> Bytes {
    let json = serde_json::to_string(event).expect("event serializes");
    Bytes::from(format!("event: {}\ndata: {json}\n\n", event_type(event)))
}

fn event_type(event: &InboxCommandEvent) -> &'static str {
    match event {
        InboxCommandEvent::Conversation { .. } => "conversation",
        InboxCommandEvent::Status { .. } => "status",
        InboxCommandEvent::Content { .. } => "content",
        InboxCommandEvent::Activity { .. } => "activity",
        InboxCommandEvent::Done { .. } => "done",
        InboxCommandEvent::Error { .. } => "error",
    }
}

fn error_frame(message: &str) -> Bytes {
    let json = serde_json::json!({ "type": "error", "message": message }).to_string();
    Bytes::from(format!("event: error\ndata: {json}\n\n"))
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
    let input = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let Some(guard) = acquire_inbox_focus_lock(&user.id) else {
        return house_error(
            StatusCode::CONFLICT,
            "Your assistant is already handling another Inbox action.",
        );
    };

    // The generator as two channels: the run sends typed events into `etx`
    // (unbounded — the run never blocks on a slow reader), and the stream
    // task forwards each as an SSE frame into the body channel. A prologue
    // failure arrives as an Err from the run and is rendered as the TS
    // catch's error event; a PANIC in the run task surfaces as a JoinError
    // after the event channel closes, and gets the TS catch's fallback
    // sentence. The `etx` clone is the error branch's voice after the
    // original moved into the call.
    let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel::<InboxCommandEvent>();
    let (btx, brx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let err_tx = etx.clone();
    let run_state = state.clone();
    let run_user = user.clone();
    let run = tokio::spawn(async move {
        // The lock lives exactly as long as this stream task.
        let _guard = guard;
        if let Err(e) = run_inbox_conversation_command(&run_state, &run_user, input, etx).await {
            let message = match e {
                FocusError::Db(err) => err.to_string(),
                FocusError::Throw(m) => m,
            };
            let _ = err_tx.send(InboxCommandEvent::Error { message });
        }
    });
    tokio::spawn(async move {
        while let Some(event) = erx.recv().await {
            if btx.send(Ok(frame(&event))).await.is_err() {
                // The client went away; stop relaying. The run itself is
                // unaffected — it finishes and persists the reply (the tee).
                break;
            }
        }
        // The event channel closes when the run task body finishes —
        // normally, with an Err it already reported, or by panic.
        if run.await.is_err() {
            let _ = btx
                .send(Ok(error_frame(
                    "Your assistant could not start that response.",
                )))
                .await;
        }
    });

    let stream =
        futures_util::stream::unfold(
            brx,
            |mut rx| async move { rx.recv().await.map(|i| (i, rx)) },
        );
    // Header ORDER matches the oracle's wire, not its source: TS builds the
    // response through a fetch Headers object, which iterates alphabetically,
    // so the bytes on :5273 read cache-control → content-type even though the
    // route literal spells Content-Type first (chat.rs does the same).
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, "no-cache, no-transform")
        .header(header::CONNECTION, "keep-alive")
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .body(Body::from_stream(stream))
        .expect("static headers build")
}
