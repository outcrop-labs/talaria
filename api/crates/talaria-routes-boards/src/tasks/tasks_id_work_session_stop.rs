// POST /api/tasks/{id}/work-session/stop. THE HUMAN BRAKE. Once an agent
// has picked a ticket up, nothing else on the board ends that work: the
// session is a detached driver whose send step may legitimately run for
// hours. This route cancels every live work-session on the ticket —
// queued, running, or parked awaiting an answer — and the cancel fires
// the local driver's abort (talaria_runs_drivers::fire, from cancel_run),
// so an in-flight model call is dropped at its next await, not at the
// next step boundary.
//
// THE TICKET IS NOT TOUCHED. Status and assignees stay exactly as they
// are — the same choice a turn-cap or a failed session makes — because a
// stop is a triage event, not an outcome: the ticket stays where the
// person can see it, with one activity line as the record. It never
// looks like work that finished (nobody signs off on a stop), and it
// never vanishes.
//
// GATED LIKE THE WORK-SESSION READ (board membership): stop is a safety
// action, so every member who can see the work may end it — viewer
// included. `{id}` is the row uuid or the ticket ref, same as the read.
// Idempotent: a ticket with no live session stops fine and still says ok.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_boards::board_role;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
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
        Err(e) => return Ok(internal("[work-session/stop] role read failed", e)),
    }

    // Every live session on the ticket — one per agent that picked it up.
    // The board feed is per-task, so "stop the work on this ticket" is the
    // only granularity the person has ever had. `subject_id` is TEXT: the
    // bind stays untyped exactly like the work-session read's, because a
    // $1::uuid against a text column is a 500 (the bind-cast trap).
    let live: Result<Vec<(String,)>, sqlx::Error> = sqlx::query_as(
        "select id::text from runs \
         where kind = 'work-session' and subject_id = $1 \
           and state in ('queued', 'running', 'awaiting') \
         order by created_at",
    )
    .bind(&id)
    .fetch_all(&state.pg)
    .await;
    let live = match live {
        Ok(runs) => runs,
        Err(e) => return Ok(internal("[work-session/stop] live-session read failed", e)),
    };

    let who = user
        .name
        .clone()
        .or(user.email.clone())
        .unwrap_or_else(|| user.id.clone());

    if !live.is_empty() {
        // Same assembly as the other run-writing routes: the cancel needs a
        // live Redis for the publish, and a stop that cannot tell the board
        // it happened is a stop that did not happen.
        let Some(redis) = state.redis().await.ok() else {
            return Ok(internal(
                "[work-session/stop] redis unavailable — refusing to stop blind",
                "redis connection unavailable",
            ));
        };
        let realtime = talaria_realtime_watch::RealtimeDeps::publish_only(Some(redis.clone()));
        let deps = talaria_api_facades::runs::real_run_deps(state.pg.clone(), redis, realtime);
        for (run_id,) in &live {
            if let Err(e) = talaria_api_facades::runs::run::cancel_run(
                run_id,
                Some(format!("stopped by {who}")),
                &deps,
            )
            .await
            {
                // One session's refused cancel must not shield the others;
                // the activity line below says what was attempted.
                tracing::error!("[work-session/stop] cancel of {run_id} failed: {e}");
            }
        }
        if let Err(e) = talaria_tasks::log_activity(
            &state.pg,
            &id,
            &who,
            "dispatch",
            &format!("work session stopped by {who} — the ticket is yours to triage"),
        )
        .await
        {
            tracing::error!("[work-session/stop] activity line failed: {e}");
        }
    }

    Ok(Json(json!({ "ok": true })).into_response())
}
