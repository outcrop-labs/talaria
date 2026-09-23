// POST /api/agents/tool-events. The inbound half of the talaria-events
// Hermes plugin: the agent's plugin reports each tool call's lifecycle
// (name, args, RESULT — the thing no other wire carries) and this route
// lands it on the agent's live run's watch stream, where the run-detail
// modal reads it live and the per-turn transcript persists it.
//
// Agent-authenticated, never session-authenticated (the same door as the
// git-credential routes). The platform does NOT trust the plugin's clamps:
// everything is re-clamped and secret-scrubbed HERE, at the boundary, before
// it touches the tail. Correlation is the plugin's session_id where the
// platform can use it and the agent's newest live work session otherwise —
// a documented v1 approximation (turns serialize through one persona, so
// the newest live run is almost always the right one; exact session pinning
// is the follow-up).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::require_agent;
use talaria_body::truncate_utf16;
use talaria_error::{house_error, internal};
use talaria_state::AppState;

const MAX_FIELD: usize = 2_000;

fn clamp_field(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|v| v.as_str())
        .map(|s| truncate_utf16(s, MAX_FIELD).to_string())
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
        let chips = talaria_chips::chips_from_tool(&tool, args, result);
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
            if let Err(e) = talaria_chips::surface_tool_chips(
                &state.pg,
                redis,
                &caller.model,
                owner.as_deref(),
                chips,
            )
            .await
            {
                tracing::warn!("[tool-events] chip surface failed: {e}");
            }
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
    if let Ok(mut conn) = state.redis().await {
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
    // The newest LIVE work session for this agent — the run whose tail this
    // frame joins. None live: the frame has nowhere to land (the session
    // ended); dropped quietly, same as the plugin's own failure contract.
    let run_id: Option<String> = match sqlx::query_scalar(
        "select id::text from runs \
         where kind = 'work-session' and state in ('queued', 'running', 'awaiting') \
           and input->>'agentModel' = $1 \
         order by created_at desc limit 1",
    )
    .bind(&caller.model)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[tool-events] run lookup failed", e)),
    };
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
    let frame = talaria_api_facades::runs::defs::work_session::scrub_secrets(&frame.to_string());
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
