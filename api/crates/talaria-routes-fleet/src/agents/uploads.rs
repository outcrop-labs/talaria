// POST /api/agents/uploads.
//
// THE DOOR FOR BYTES THE AGENT MADE ITSELF. A persona that takes a browser
// screenshot holds a PNG inside its own container and no other wire can carry
// it: tool-events takes a clamped STRING, the chat-completions stream carries
// prose, and the container is disposable — the file is gone with the next
// roll. The talaria-events plugin reads what `browser_vision` wrote and posts
// it here as base64; this route stores it, stamps it on the turn the agent is
// answering, and lands one image chip, so the person watching sees the
// screenshot instead of a path inside a box that no longer exists.
//
// Agent-authenticated, never session-authenticated — the same door as
// tool-events, and the same contract: the platform does not trust the
// plugin's clamps, so the mime, the filename and both size bounds are
// re-decided HERE.
//
// THE TWO WRITES ARE ORDERED, and the order is the point: the attachment is
// stamped on the message row BEFORE the chip is surfaced. `can_access_upload`
// answers "may this reader see these bytes" by probing the attachments JSON
// of the rows the reader can reach, so a chip that arrives ahead of its stamp
// is an image pointing at a 404 for everyone but the upload's owner — and the
// chip write is also the realtime nudge that makes a browser go fetch it.
//
// ADDRESSED BY CONVERSATION. The plugin stamps the Hermes session id on the
// POST, and for a pinned agent that id IS the conversation id: the row is
// resolved inside that conversation and nowhere else. A body with no usable
// id is served only while the agent has exactly one turn it could mean —
// several, and the answer is a 409 that says so, never a guess at somebody
// else's thread.
//
// THE TRUST BOUNDARY IS THE AGENT, NOT THE PLUGIN. The key this door takes
// sits in the container's environment where the model's own terminal can
// read it, so a model (or an instruction injected into it) can post bytes
// here as "the page it saw"; the route re-clamps and scrubs, and scopes the
// landing to the agent's own threads, but cannot tell plugin from model.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{Value, json};
use talaria_agent_auth::require_agent;
use talaria_body::{as_object, parse, string_member, trimmed_string_member};
use talaria_conversations::{active_streaming_assistant, append_message_attachment};
use talaria_error::{house_error, internal};
use talaria_state::AppState;

/// The decoded ceiling. A screenshot, not a recording: 5 MiB is a generous
/// full-page PNG, and small enough that the whole image rides one JSON body
/// instead of asking an agent plugin to learn multipart.
pub const MAX_BYTES: usize = 5 * 1024 * 1024;

/// The same ceiling in its wire form. Base64 spends 4 characters per 3 bytes,
/// so `MAX_BYTES` encodes to about 6.7 MiB; 7 MiB is that plus padding and
/// nothing more.
pub const MAX_BASE64: usize = 7 * 1024 * 1024;

/// The route's own body cap, for the layer that registers it. axum ships a
/// hidden 2 MB `DefaultBodyLimit`, which would kill every screenshot worth
/// sending as an unreadable body before this file ever ran. The envelope is
/// the JSON around the payload — three keys and a filename.
pub const ROUTE_BODY_LIMIT: usize = MAX_BASE64 + 4 * 1024;

/// What a screenshot may claim to be. Narrow on purpose: this door exists for
/// images a person is about to look at in a chat turn, and a mime list is the
/// cheapest way to keep it from becoming a general file drop that agents post
/// archives through.
const ALLOWED_MIME: &[&str] = &["image/png", "image/jpeg", "image/webp"];

/// The screenshot tool whose PNG this door was built for — named in the
/// refusal so an agent posting something else learns what the door is.
const SOURCE_TOOL: &str = "browser_vision";

pub(crate) struct Shot {
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Written by hand rather than derived: the derive would put megabytes of
/// image into whatever line printed it, and the only thing worth seeing about
/// a decoded screenshot is what it claims to be and how big it came out.
impl std::fmt::Debug for Shot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shot")
            .field("filename", &self.filename)
            .field("mime", &self.mime)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// The body, or the sentence that names the field that was wrong. Pure, so
/// the bounds are provable without a Postgres: every refusal names its field
/// because the caller is a model reading the answer, and "invalid body" tells
/// it nothing it can act on.
pub(crate) fn read_body(parsed: &Value) -> Result<Shot, String> {
    let obj = as_object(parsed)?;
    let filename =
        trimmed_string_member(obj, "filename", 1, 200).map_err(|m| format!("filename: {m}"))?;
    let mime = string_member(obj, "mime", 1, 100).map_err(|m| format!("mime: {m}"))?;
    if !ALLOWED_MIME.contains(&mime.as_str()) {
        return Err(format!(
            "mime must be one of {} — this door takes {SOURCE_TOOL} screenshots, not files",
            ALLOWED_MIME.join(", ")
        ));
    }
    // The wire length is checked before the decode so an oversized payload is
    // refused by its own size rather than by whatever the decoder makes of
    // its tail.
    let data =
        string_member(obj, "dataBase64", 1, MAX_BASE64).map_err(|m| format!("dataBase64: {m}"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| "dataBase64: not valid base64".to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err(format!(
            "dataBase64: decodes to {} bytes, over the {MAX_BYTES}-byte ceiling",
            bytes.len()
        ));
    }
    Ok(Shot {
        filename,
        mime,
        bytes,
    })
}

/// The one chip a stored screenshot lands on the turn. The id is derived from
/// the upload so the chip merge is idempotent — a plugin that retried the
/// same POST would land a second upload, but a surface that replayed the same
/// chip cannot stack it twice. `href` is the ordinary serve route: the image
/// goes through the same access check as every other attachment.
pub(crate) fn image_chip(upload_id: &str, filename: &str) -> Value {
    json!({
        "id": format!("image:{upload_id}"),
        "kind": "image",
        "label": filename,
        "uploadId": upload_id,
        "href": format!("/api/uploads/{upload_id}"),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    let parsed = parse(&body);
    let shot = match read_body(&parsed) {
        Ok(shot) => shot,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // ASKED BEFORE THE BYTES ARE STORED. A screenshot with nowhere to land is
    // a blob nobody will ever open, and the next tool call is one screenshot
    // away — refusing costs the agent nothing. 409, not 400: the body was
    // fine, the moment was not.
    let hint = talaria_chips::conversation_hint(&parsed);
    let target =
        match talaria_chips::landing_target(&state.pg, &caller.model, hint.as_deref()).await {
            Ok(t) => t,
            Err(e) => return Ok(internal("[agent-uploads] live turn lookup failed", e)),
        };
    if let Some(why) = target.ambiguity() {
        return Ok(house_error(StatusCode::CONFLICT, &why));
    }
    // A channel turn has no conversation row to stamp: `append_message_attachment`
    // writes `messages`, and a channel message is a different table.
    let Some(conversation) = target.landed().and_then(|l| l.conversation_id) else {
        return Ok(house_error(
            StatusCode::CONFLICT,
            "no live turn to attach to",
        ));
    };
    // The row the chip is about to land on, in the ordinary case: the reply
    // the agent is streaming right now. Asked of the conversations crate
    // rather than re-queried here — that function is the house's answer to
    // "which row is this turn", crashed-writer sweep included.
    let streaming = active_streaming_assistant(&state.pg, &conversation)
        .await
        .unwrap_or_default();
    // The upload's owner, for the same reason the chip surface needs one: an
    // attachment with no uploader is readable only through somebody's
    // message, and the person the agent works for should own what it made.
    let owner =
        talaria_api_facades::google::agent::resolve_agent_principal(&state.pg, &caller.model)
            .await
            .ok()
            .and_then(|principal| principal.owner_user_id);
    let sb = state.secretbox().await.unwrap_or_default();
    let att = match talaria_uploads::save_upload(
        &state.pg,
        &sb,
        &shot.filename,
        &shot.mime,
        &shot.bytes,
        owner.as_deref(),
    )
    .await
    {
        Ok(att) => att,
        // A blob that cannot be written or recorded is the server's fault,
        // never a 400 blaming a body that was fine — the split the human
        // upload door draws.
        Err(msg) => {
            tracing::error!("[agent-uploads] save failed: {msg}");
            return Ok(house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upload failed",
            ));
        }
    };
    let attachment = serde_json::to_value(&att).unwrap_or(Value::Null);
    let mut stamped = false;
    if let Some(id) = &streaming {
        match append_message_attachment(&state.pg, id, &attachment).await {
            Ok(()) => stamped = true,
            // The upload survives a failed stamp; only its reach is lost, and
            // saying so is the difference between a missing image and a
            // mystery.
            Err(e) => tracing::warn!("[agent-uploads] attachment stamp failed: {e}"),
        }
    }
    let redis = state.redis().await.ok();
    // Addressed to the conversation resolved above, so the chip and the
    // stamp can never land on opposite sides of two people's turns.
    let landed = match talaria_chips::surface_tool_chips_in(
        &state.pg,
        redis,
        &caller.model,
        owner.as_deref(),
        Some(&conversation),
        vec![image_chip(&att.id, &att.filename)],
    )
    .await
    {
        Ok(target) => target.landed(),
        Err(e) => {
            tracing::warn!("[agent-uploads] chip surface failed: {e}");
            None
        }
    };
    // The streaming row is the usual target and the chip lands on it. When
    // the turn closed between the gate and now, the chip went on the
    // conversation's newest reply instead — stamp THAT row, so the image and
    // the chip are never on opposite sides of one answer.
    if !stamped
        && let Some(landed) = &landed
        && landed.conversation_id.is_some()
    {
        match append_message_attachment(&state.pg, &landed.message_id, &attachment).await {
            Ok(()) => stamped = true,
            Err(e) => tracing::warn!("[agent-uploads] attachment stamp failed: {e}"),
        }
    }
    // `landed` answers what the plugin cannot see: the bytes are stored
    // either way, but only a stamped-and-chipped screenshot is actually in
    // front of the person the agent is talking to.
    let landed = stamped && landed.is_some();
    if !landed {
        let agent = &caller.model;
        tracing::warn!(
            "[agent-uploads] {} from {agent} stored but did not reach a turn",
            att.id
        );
    }
    Ok(Json(json!({ "ok": true, "uploadId": att.id, "landed": landed })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(filename: &str, mime: &str, data: &str) -> Value {
        json!({ "filename": filename, "mime": mime, "dataBase64": data })
    }

    // "iVBORw0KGgo=" is the PNG magic, base64'd — the smallest honest payload.
    const PNG: &str = "iVBORw0KGgo=";

    #[test]
    fn the_mime_list_is_the_whole_list() {
        for mime in ALLOWED_MIME {
            let shot = read_body(&body("shot.png", mime, PNG)).expect("an allowed mime passes");
            assert_eq!(&shot.mime, mime);
            assert_eq!(shot.bytes, b"\x89PNG\r\n\x1a\n");
        }
        // A real image type the door still refuses: narrowness is the rule,
        // not an oversight in the list.
        let err = read_body(&body("shot.gif", "image/gif", PNG)).expect_err("gif is refused");
        assert!(err.starts_with("mime must be one of"), "{err}");
        let err =
            read_body(&body("notes.zip", "application/zip", PNG)).expect_err("archives refused");
        assert!(err.contains("image/png"), "{err}");
        // A missing or non-string mime is named too, never swallowed into the
        // list message.
        let err = read_body(&json!({ "filename": "a.png", "dataBase64": PNG }))
            .expect_err("mime is required");
        assert!(err.starts_with("mime: "), "{err}");
    }

    #[test]
    fn the_filename_is_bounded_and_trimmed() {
        let shot = read_body(&body("  shot.png  ", "image/png", PNG)).expect("trimmed");
        assert_eq!(shot.filename, "shot.png");

        let err = read_body(&body("   ", "image/png", PNG)).expect_err("blank is not a name");
        assert!(err.starts_with("filename: "), "{err}");

        let long = "n".repeat(201);
        let err = read_body(&body(&long, "image/png", PNG)).expect_err("201 is too long");
        assert!(err.starts_with("filename: "), "{err}");
        let ok = "n".repeat(200);
        assert!(read_body(&body(&ok, "image/png", PNG)).is_ok(), "200 fits");
    }

    #[test]
    fn the_size_bounds_hold_on_the_wire_and_after_the_decode() {
        // Over the wire bound: refused by its own length, before a decoder
        // ever sees 7 MiB of input.
        let huge = "A".repeat(MAX_BASE64 + 1);
        let err = read_body(&body("shot.png", "image/png", &huge)).expect_err("over the wire cap");
        assert!(err.starts_with("dataBase64: "), "{err}");

        // Under the wire bound, over the decoded one — the pair of checks the
        // single `MAX_BASE64` cap cannot make on its own.
        let over = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_BYTES + 1]);
        assert!(
            over.len() <= MAX_BASE64,
            "the wire check must not be what fires"
        );
        let err =
            read_body(&body("shot.png", "image/png", &over)).expect_err("over the decoded cap");
        assert!(err.contains("over the"), "{err}");

        // The ceiling itself is a pass: a bound is a limit, not a fence one
        // byte short of it.
        let at = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_BYTES]);
        let shot = read_body(&body("shot.png", "image/png", &at)).expect("the ceiling passes");
        assert_eq!(shot.bytes.len(), MAX_BYTES);

        let err = read_body(&body("shot.png", "image/png", "not base64!!"))
            .expect_err("garbage is refused");
        assert_eq!(err, "dataBase64: not valid base64");
        let err = read_body(&json!({ "filename": "a.png", "mime": "image/png" }))
            .expect_err("the payload is required");
        assert!(err.starts_with("dataBase64: "), "{err}");
    }

    #[test]
    fn a_body_that_is_not_an_object_is_refused() {
        assert!(read_body(&json!([1, 2, 3])).is_err());
        assert!(read_body(&Value::Null).is_err());
    }

    // The chip is a contract with the renderer and with chips.ts — pinned
    // whole, because a renamed key here is an image the UI silently drops.
    #[test]
    fn the_image_chip_shape_is_pinned() {
        assert_eq!(
            image_chip("22f1e3a0-0000-4000-8000-000000000001", "shot.png"),
            json!({
                "id": "image:22f1e3a0-0000-4000-8000-000000000001",
                "kind": "image",
                "label": "shot.png",
                "uploadId": "22f1e3a0-0000-4000-8000-000000000001",
                "href": "/api/uploads/22f1e3a0-0000-4000-8000-000000000001",
            })
        );
    }
}
