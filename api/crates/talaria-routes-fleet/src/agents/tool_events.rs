// POST /api/agents/tool-events. The inbound half of the talaria-events
// Hermes plugin: the agent's plugin reports each tool call's lifecycle
// (name, args, RESULT — the thing no other wire carries) and this route
// lands it on the agent's live run's watch stream, where the run-detail
// modal reads it live and the per-turn transcript persists it.
//
// Agent-authenticated, never session-authenticated (the same door as the
// git-credential routes). The platform does NOT trust the plugin's clamps:
// everything is re-clamped and secret-scrubbed HERE, at the boundary, before
// it touches the tail. Correlation is the plugin's `sessionId`: a work
// session sends every turn on a Hermes session keyed by its RUN id
// (`session_for_turn`), so the id the plugin reports IS the run, and
// `live_work_session_for` lands the frame on exactly that run — never on
// the newest one, which was the wrong one whenever an agent drove two
// tickets (the thrash brake aborted the wrong run; transcripts mixed). A
// frame whose session id names no live run of this agent (a chat
// conversation's id, a finished run) lands nowhere; only a frame that names
// no key at all — an older plugin, or a session the container minted for a
// completions turn — takes the newest live session, the v1 approximation
// kept for the frames that cannot say. The CHIPS follow the same key: they
// land in the conversation the session id names (for a pinned agent the
// Hermes session id is the conversation id), and without one only while
// exactly one turn could be meant. A call the plugin reports FAILED is also
// marked failed on the chat reply that ran it, by its tool-call id: Hermes'
// completions stream calls every finished call `completed`, so this door is
// the only place a chat turn learns otherwise.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::require_agent;
use talaria_body::truncate_utf16;
use talaria_error::house_error;
use talaria_state::AppState;

const MAX_FIELD: usize = 2_000;

fn clamp_field(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|v| v.as_str())
        .map(|s| truncate_utf16(s, MAX_FIELD).to_string())
}

/// The credential scrub this door promises, in the one place both readers
/// of the plugin's payload go through.
fn scrub(text: &str) -> String {
    talaria_api_facades::runs::defs::work_session::scrub_secrets(text)
}

/// The session the plugin named, when it is a key Talaria could have minted:
/// a UUID — a work session's run id, or a pinned chat's conversation id.
/// Anything else (blank, or an id the container made up for a completions
/// turn) is "the frame cannot say", and the caller falls back to its
/// newest-session read for it rather than asking the database to cast it.
fn named_session(body: &serde_json::Value) -> Option<&str> {
    body.get("sessionId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| uuid::Uuid::parse_str(s).is_ok())
}

/// The chips one completed tool call implies — read from SCRUBBED text.
///
/// THE BOUNDARY'S PROMISE COVERS THE CHIP TOO. This file re-clamps and
/// scrubs everything before it joins the run-watch tail, but the chip reader
/// was handed the raw strings: a key echoed into a tool result (an agent
/// printing an env var, a 401 body quoting the token it rejected) rode
/// straight onto a chip's label or detail — and a chip is the one thing here
/// that PERSISTS, on the message row, and renders in the chat for everyone
/// who can read the turn. Scrub first, then recognize.
fn chips_for(tool: &str, args: &str, result: &str) -> Vec<serde_json::Value> {
    talaria_chips::chips_from_tool(tool, &scrub(args), &scrub(result))
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Ok(house_error(StatusCode::BAD_REQUEST, "body must be JSON")),
    };
    let tool = parsed
        .get("toolName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if tool.is_empty() {
        return Ok(house_error(StatusCode::BAD_REQUEST, "toolName required"));
    }
    let status = match parsed.get("status").and_then(|v| v.as_str()) {
        Some("running") | Some("completed") | Some("error") => parsed
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "status must be running|completed|error",
            ));
        }
    };
    // Chips are not the run watch. A chat turn has no work-session run, and
    // dropping the frame there used to drop the link and the approval with
    // it. Surface first, then land the watch frame if a run is live.
    if status == "completed" {
        let args = parsed.get("args").and_then(|v| v.as_str()).unwrap_or("");
        let result = parsed.get("result").and_then(|v| v.as_str()).unwrap_or("");
        let chips = chips_for(&tool, args, result);
        if !chips.is_empty() {
            let redis = state.redis().await.ok();
            // The chip's decide gate is owner-or-admin. Passing no owner made
            // a personal confirm-send invisible to the person it was drafted
            // for, while draft_email had already returned success.
            let owner = talaria_api_facades::google::agent::resolve_agent_principal(
                &state.pg,
                &caller.model,
            )
            .await
            .ok()
            .and_then(|principal| principal.owner_user_id);
            let agent = &caller.model;
            let hint = talaria_chips::conversation_hint(&parsed);
            match talaria_chips::surface_tool_chips_in(
                &state.pg,
                redis,
                &caller.model,
                owner.as_deref(),
                hint.as_deref(),
                chips,
            )
            .await
            {
                Ok(talaria_chips::Target::Found(_)) => {}
                // Several turns and no session named: the chips are dropped
                // rather than guessed onto somebody else's thread. The event
                // itself still lands below — the watch frame is keyed by the
                // run, not the conversation.
                Ok(talaria_chips::Target::Ambiguous { turns }) => tracing::warn!(
                    "[tool-events] chips from {tool} for {agent}: {turns} turns live and no \
                     session named — dropped"
                ),
                // Chips computed but nowhere to land: no streaming turn and
                // nothing recent — a silent drop without this line.
                Ok(talaria_chips::Target::Nowhere) => tracing::warn!(
                    "[tool-events] chips from {tool} for {agent} had no turn to land on — dropped"
                ),
                Err(e) => tracing::warn!("[tool-events] chip surface failed: {e}"),
            }
        }
    }
    let redis = state.redis().await.ok();
    // THE VERDICT THE CHAT STREAM CANNOT CARRY. Hermes' completions stream
    // ends every finished tool call `completed`, a command its approval gate
    // refused included; this post is the only place that says the call
    // failed. Kept for the turn still running it and marked on a reply that
    // already landed (talaria-conversations `record_tool_failure`), keyed by
    // the same tool-call id the stream's frames carry.
    if status == "error"
        && let Some(call_id) = parsed.get("toolCallId").and_then(|v| v.as_str())
    {
        let mut conn = redis.clone();
        if let Err(e) = talaria_conversations::record_tool_failure(
            &state.pg,
            conn.as_mut(),
            &caller.model,
            call_id,
        )
        .await
        {
            tracing::warn!("[tool-events] marking {tool} {call_id} failed: {e}");
        }
    }
    // THE FLEET'S ONLY FAILURE SIGNAL. A persona tool failing is invisible to
    // every other surface — the model stream says "completed", container
    // health stays green, stats look idle (the 2026-09-22 rot: half an hour
    // of every tool failing while every dashboard read green). The plugin
    // now reports status "error" for the persona's own error convention,
    // and this streak — agent-scoped, not run-scoped, so chat and cron
    // turns count too — is what the alerts surface reads. INCR + 6h EXPIRE
    // on a failure, DEL on a success: consecutive by construction, and an
    // idle agent's streak dies with the key instead of alerting forever.
    if let Some(mut conn) = redis {
        let streak_key = format!("agent-tools:{}:fail-streak", caller.model);
        if status == "error" {
            let _: Result<i64, redis::RedisError> = redis::cmd("INCR")
                .arg(&streak_key)
                .query_async(&mut conn)
                .await;
            let _: Result<(), redis::RedisError> = redis::cmd("EXPIRE")
                .arg(&streak_key)
                .arg(21_600)
                .query_async(&mut conn)
                .await;
        } else if status == "completed" {
            let _: Result<(), redis::RedisError> = redis::cmd("DEL")
                .arg(&streak_key)
                .query_async(&mut conn)
                .await;
        }
    }
    // THE RUN WHOSE TAIL THIS FRAME JOINS. By the session the plugin named
    // when it named one — exact, and `None` when that session is not a live
    // work session of this agent (a chat's frames stay out of the run) — and
    // only for a frame that could not say, the agent's newest live session.
    // None either way: the frame has nowhere to land (the session ended);
    // dropped quietly, same as the plugin's own failure contract.
    let run_id: Option<String> = match named_session(&parsed) {
        Some(session_id) => {
            talaria_api_facades::runs::defs::work_session::live_work_session_for(
                &state.pg,
                &caller.model,
                session_id,
            )
            .await
        }
        None => {
            sqlx::query_scalar(
                "select id::text from runs \
                 where kind = 'work-session' and state in ('queued', 'running', 'awaiting') \
                   and input->>'agentModel' = $1 \
                 order by created_at desc limit 1",
            )
            .bind(&caller.model)
            .fetch_optional(&state.pg)
            .await
        }
    }
    .unwrap_or_else(|e| {
        // A lookup that failed is a frame that cannot land, not a 500 at a
        // door whose contract is fire-and-forget; the line says why.
        tracing::warn!("[tool-events] run lookup failed: {e}");
        None
    });
    let Some(run_id) = run_id else {
        return Ok(Json(json!({ "ok": true, "landed": false })).into_response());
    };
    let frame = json!({
        "t": "toolfull",
        "v": truncate_utf16(&tool, 120).to_string(),
        "s": status,
        "id": parsed.get("toolCallId").and_then(|v| v.as_str()).unwrap_or(""),
        "p": clamp_field(parsed.get("args")),
        "r": clamp_field(parsed.get("result")),
        "turn": parsed.get("turnId").and_then(|v| v.as_str()).unwrap_or(""),
    });
    // Scrub at the boundary — the same shapes the transcript capture kills.
    let frame = scrub(&frame.to_string());
    let key = format!("run-watch:{run_id}:tail");
    let chan = format!("run-watch:{run_id}");
    match state.redis().await {
        Ok(mut conn) => {
            let _ = redis::cmd("APPEND")
                .arg(&key)
                .arg(format!("{frame}\n"))
                .query_async::<()>(&mut conn)
                .await;
            let _ = redis::cmd("EXPIRE")
                .arg(&key)
                .arg(7_200)
                .query_async::<()>(&mut conn)
                .await;
            let _ = redis::cmd("PUBLISH")
                .arg(&chan)
                .arg(&frame)
                .query_async::<()>(&mut conn)
                .await;
        }
        Err(e) => {
            tracing::warn!("[tool-events] redis unreachable, frame dropped: {e}");
        }
    }
    Ok(Json(json!({ "ok": true, "landed": true })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE FRAME NAMES ITS RUN. A session id Talaria minted — a work
    /// session's run id — is the key the lookup asks by, exactly as sent;
    /// whatever the container minted for a turn Talaria did not key, or
    /// nothing at all, is "cannot say" and takes the newest-session read.
    #[test]
    fn a_frame_is_keyed_by_the_session_it_names_and_only_a_minted_key_counts() {
        let run = "7b1e4c1a-9f8d-8a1b-9c2d-3e4f5a6b7c8d";
        let named = json!({ "sessionId": format!("  {run} "), "toolName": "terminal" });
        assert_eq!(named_session(&named), Some(run));
        for cannot_say in [
            json!({ "toolName": "terminal" }),
            json!({ "sessionId": "", "toolName": "terminal" }),
            json!({ "sessionId": "api-7f3a", "toolName": "terminal" }),
            json!({ "sessionId": 42, "toolName": "terminal" }),
        ] {
            assert_eq!(named_session(&cannot_say), None, "{cannot_say}");
        }
    }

    /// A KEY THE AGENT ECHOED NEVER REACHES A CHIP. The chip is the
    /// persisting half of this door — it is written onto the message row and
    /// rendered to everyone who can read the turn — so it goes through the
    /// same scrub as the watch frame rather than reading the plugin's raw
    /// strings.
    #[test]
    fn a_secret_in_a_tool_result_is_scrubbed_before_the_chip_reader_sees_it() {
        // One of each shape the boundary kills, in the three fields a chip
        // actually copies text out of: title, snippet, and the prose walk.
        let key = "sk-proj-A1b2C3d4E5f6G7h8J9k0";
        let pat = "ghp_A1b2C3d4E5f6G7h8J9k0L1m2N3o4P5q6";
        let result = format!(
            r#"[{{"title":"token {key} works","url":"https://example.com/a","snippet":"export GITHUB_TOKEN={pat}"}}]"#
        );
        let chips = chips_for("web_search", &format!(r#"{{"query":"{key}"}}"#), &result);
        assert_eq!(chips.len(), 1, "{chips:?}");
        let text = serde_json::to_string(&chips).expect("plain data");
        assert!(!text.contains(key), "{text}");
        assert!(!text.contains(pat), "{text}");
        assert!(text.contains("[redacted:key]"), "{text}");
        // And the chip is still the chip: the scrub replaces the shape, it
        // does not drop the page.
        assert_eq!(chips[0]["href"], "https://example.com/a");
        assert_eq!(chips[0]["label"], "token [redacted:key] works");
    }
}
