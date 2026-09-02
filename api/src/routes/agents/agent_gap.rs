// /api/agent/gap. POST — an agent reports a capability gap (the honesty loop). Deduped by
// work-shape server-side: repeats bump seen_count, never re-notify. Lands in
// the Studio's Suggested queue; the ticket (if given) gets an audit line.

use crate::agent_auth::{AgentSubject, require_agent};
use crate::boards::board_allows_agent;
use crate::body::{
    as_object, optional_max_string_member, optional_uuid_member, parse, string_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::gaps::{remember_ticket_refusal, report_gap, report_gap::GapInput};
use crate::notify::NotifyDeps;
use crate::state::AppState;
use crate::tasks::{AgentIntent, AgentWriteTarget, agent_ticket_refusal, get_task, log_activity};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let agent = caller.model.clone();
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let kind = match string_member(obj, "kind", 2, 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let missing = match string_member(obj, "missing", 5, 300) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let needs = match optional_max_string_member(obj, "needs", 5000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let task_id = match optional_uuid_member(obj, "taskId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // `taskId` arrives from the agent, so it is AUTHORISED, never taken on
    // faith: the ticket gets an audit line and the gap row is bound to that
    // ticket's board. Without this an agent forges activity on any ticket
    // org-wide and binds a Studio gap to a board it cannot see. Unknown and
    // not-allowed refuse identically — a distinct 404 would be a ticket
    // enumeration oracle.
    //
    // NEVER tell a refused agent to drop `taskId` and retry: `boardId` is
    // what narrows the gap's free text to the admins who can see that board,
    // and that text routinely names the ticket, the customer or the file.
    // "Re-send without taskId" would be an instruction to widen its own
    // disclosure. AND SAYING NOTHING IS NOT ENOUGH — each refusal below is
    // REMEMBERED against the caller, and `reportGap` reads that memo when a
    // report arrives with no ticket on it. An agent that names no ticket and
    // was refused nothing is making a genuinely org-wide claim and is
    // unaffected.
    let task = match task_id.as_deref() {
        Some(id) => match get_task(&state.pg, id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("[agent.gap] ticket read failed: {e}");
                return thrown_internal_error();
            }
        },
        None => None,
    };
    if let Some(task_id) = task_id.as_deref() {
        // Unknown and not-allowed refuse identically.
        let refuse = || {
            house_error(
                StatusCode::FORBIDDEN,
                &format!(
                    "taskId \"{task_id}\" is not a ticket you may write to. Ask for access to its \
                     board, then report the gap against the ticket."
                ),
            )
        };
        // Remembered with NO board in both of these: the agent named a ticket
        // that does not exist, or one on a board it may not work. Binding its
        // next report to a board it cannot see would let it choose which
        // board's admins read its text, so it gets no board and its retry
        // reaches every admin as a fact and none of them as words.
        let Some(task) = task.as_ref() else {
            remember_ticket_refusal(&state.pg, &agent, None).await;
            return refuse();
        };
        // The CALLER, not its model: board policy's elevated bypass is only
        // for an identity that was proven, never merely asserted.
        let allowed = match board_allows_agent(
            &state.pg,
            &task.board_id,
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[agent.gap] board policy read failed: {e}");
                return thrown_internal_error();
            }
        };
        if !allowed {
            remember_ticket_refusal(&state.pg, &agent, None).await;
            return refuse();
        }
        // A person has taken this ticket off the table (signed off, archived,
        // or its board archived). The SAME predicate `agentSafePatch` asks:
        // this route writes an activity line and never reaches `updateTask`.
        // The agent IS allowed on this board, so this is the one refusal whose
        // board we can safely bind a retry to.
        let target = AgentWriteTarget {
            board_id: task.board_id.clone(),
            status: task.status.clone(),
            archived_at: task.archived_at.clone(),
        };
        match agent_ticket_refusal(
            &state.pg,
            &target,
            &AgentSubject::Caller(caller.clone()),
            AgentIntent::Write,
        )
        .await
        {
            Ok(None) => {}
            Ok(Some(shut)) => {
                remember_ticket_refusal(&state.pg, &agent, Some(&task.board_id)).await;
                let mut resp =
                    Json(json!({ "error": "forbidden", "message": shut })).into_response();
                *resp.status_mut() = StatusCode::FORBIDDEN;
                return resp;
            }
            Err(e) => {
                tracing::error!("[agent.gap] ticket refusal read failed: {e}");
                return thrown_internal_error();
            }
        }
    }

    let deps = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let gap = match report_gap(
        &deps,
        GapInput {
            agent_model: &agent,
            kind: &kind,
            missing: &missing,
            needs: needs.as_deref(),
            board_id: task.as_ref().map(|t| t.board_id.as_str()),
            task_id: task.as_ref().map(|t| t.id.as_str()),
        },
    )
    .await
    {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[agent.gap] report failed: {e}");
            return thrown_internal_error();
        }
    };
    if let Some(task) = task.as_ref() {
        // the audit line is best-effort — a failed log is not a failed report.
        let seen = if gap.seen_count > 1 {
            format!(", seen {}×", gap.seen_count)
        } else {
            String::new()
        };
        let clipped: String = missing.chars().take(200).collect();
        let _ = log_activity(
            &state.pg,
            &task.id,
            &agent,
            "gap",
            &format!("reported a capability gap ({kind}{seen}): {clipped}"),
        )
        .await;
    }
    let note = if gap.first {
        "Gap recorded — it will be suggested to the team in the Studio. Continue as best you can or set the ticket blocked with a comment.".to_string()
    } else {
        // × is a literal multiplication sign in the sentence, not a format
        // artifact.
        format!(
            "Known gap (seen {}×) — already suggested to the team. Do not report it again; continue as best you can or set the ticket blocked.",
            gap.seen_count
        )
    };
    Json(json!({ "ok": true, "seenCount": gap.seen_count, "note": note })).into_response()
}
