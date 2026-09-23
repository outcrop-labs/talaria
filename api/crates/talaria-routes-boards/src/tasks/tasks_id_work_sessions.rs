// GET /api/tasks/{id}/work-sessions. THE RECORD, beside the live read's
// NOW. The live work-session read answers "is anyone on it"; this answers
// "what happened" — every session this ticket has had, live ones first by
// creation order, finished ones newest-first, twenty deep. The run-detail
// modal does the rest from the runId (live SSE when live, the retained
// per-turn transcript artifacts when not): reviewing the work AFTER it is
// done is the whole point, and until this read the only way back to a
// finished session's log was already knowing its run id.
//
// GATED LIKE THE TASK READ (board visibility) and shaped like the live
// read's session object (runId/state/phase/agentModel/turn), plus
// finishedAt — null while the session is live, the wall clock it ended on
// otherwise. 404 for unknown task, empty list for a ticket nobody has
// worked.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::Row;

use talaria_boards::board_role;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    if let Some(gate) = talaria_params::uuid_gate("tasks", "GET work-sessions", &id) {
        return Ok(gate);
    }
    let user = require_user(&state, &headers).await?;
    let board: Option<(String,)> =
        sqlx::query_as("select board_id::text from tasks where id = $1::uuid")
            .bind(&id)
            .fetch_optional(&state.pg)
            .await
            .unwrap_or(None);
    let Some((board_id,)) = board else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    match board_role(&state.pg, &user.id, &board_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[work-sessions] role read failed", e)),
    }

    // Live first (a session being worked belongs at the top even if it is
    // younger than a recent finish), then the finished tail newest-first.
    // `subject_id` is TEXT: untyped bind, same as the live read.
    let rows = match sqlx::query(
        "select id::text, state::text, phase, input, checkpoint, \
                coalesce(to_char(finished_at, 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), '') as finished \
         from runs \
         where kind = 'work-session' and subject_id = $1 \
         order by (state in ('queued', 'running', 'awaiting')) desc, created_at desc \
         limit 20",
    )
    .bind(&id)
    .fetch_all(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[work-sessions] read failed", e)),
    };
    let sessions: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let input: serde_json::Value = row.get("input");
            let checkpoint: serde_json::Value = row.get("checkpoint");
            let finished: String = row.get("finished");
            json!({
                "runId": row.get::<String, _>("id"),
                "state": row.get::<String, _>("state"),
                "phase": row.get::<Option<String>, _>("phase"),
                "agentModel": input.get("agentModel").and_then(|v| v.as_str()),
                "turn": checkpoint.get("turn").and_then(|v| v.as_i64()),
                "finishedAt": if finished.is_empty() { None } else { Some(finished) },
            })
        })
        .collect();
    Ok(Json(json!({ "sessions": sessions })).into_response())
}
