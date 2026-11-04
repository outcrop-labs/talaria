// GET /api/tasks/{id}/work-sessions. THE RECORD, beside the live read's
// NOW. The live work-session read answers "is anyone on it"; this answers
// "what happened" — every session this ticket has had, live ones first by
// creation order, finished ones newest-first, twenty deep. The run-detail
// modal does the rest from the runId: live SSE while the session is live,
// GET /api/runs/{id}/transcript for the retained turns. Reviewing the work
// after it is done is the whole point.
//
// `{id}` is the row uuid or the ref the board shows (`PLAT-118`), same as
// the live work-session read. A miss is 404. Two boards sharing the ref
// is 409.
//
// GATED LIKE THE TASK READ (board visibility) and shaped like the live
// read's session object (runId/state/phase/agentModel/turn), plus
// finishedAt — null while the session is live, the wall clock it ended on
// otherwise. 404 for unknown task, empty list for a ticket nobody has
// worked.
//
// Each session also says which attempt it is (a run nobody rewound is attempt
// 1 of its own lineage) and what it did outside its workspace: tokens, pushes
// and pull requests. `lineages` groups the sessions into their attempts.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use sqlx::{FromRow, Row};

use talaria_boards::board_role;
use talaria_error::{house_error, internal};
use talaria_runs_store::{LINEAGE_COLS, RunLineage};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
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
        Err(e) => return Ok(internal("[work-sessions] role read failed", e)),
    }

    // Live first (a session being worked belongs at the top even if it is
    // younger than a recent finish), then the finished tail newest-first.
    // `subject_id` is TEXT: untyped bind, same as the live read.
    let rows = match sqlx::query(sqlx::AssertSqlSafe(format!(
        "select id::text, state::text, phase, input, checkpoint, {LINEAGE_COLS}, \
                coalesce(to_char(finished_at, 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), '') as finished, \
                (select coalesce(sum(u.prompt_tokens::bigint + u.completion_tokens \
                                     + u.cache_write_tokens + u.cache_read_tokens), 0)::bigint \
                   from usage_events u where u.ref_id = runs.id) as tokens, \
                (select coalesce(jsonb_agg(jsonb_build_object('branch', p.branch, 'at', \
                          to_char(p.created_at, 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')) \
                          order by p.created_at), '[]'::jsonb) \
                   from run_pushes p where p.run_id = runs.id) as pushes, \
                (select coalesce(jsonb_agg(j.pr_url order by j.created_at), '[]'::jsonb) \
                   from workbench_jobs j \
                   where j.run_id = runs.id and j.pr_url is not null) as pull_requests \
         from runs \
         where kind = 'work-session' and subject_id = $1 \
         order by (state in ('queued', 'running', 'awaiting')) desc, created_at desc \
         limit 20"
    )))
    .bind(&id)
    .fetch_all(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[work-sessions] read failed", e)),
    };
    let mut attempts: Vec<(RunLineage, String)> = Vec::with_capacity(rows.len());
    let mut sessions: Vec<Value> = Vec::with_capacity(rows.len());
    for row in &rows {
        let input: Value = row.get("input");
        let checkpoint: Option<Value> = row.get("checkpoint");
        let finished: String = row.get("finished");
        let run_id: String = row.get("id");
        let lineage = match RunLineage::from_row(row) {
            Ok(lineage) => lineage,
            Err(e) => return Ok(internal("[work-sessions] lineage read failed", e)),
        };
        sessions.push(json!({
            "runId": run_id,
            "state": row.get::<String, _>("state"),
            "phase": row.get::<Option<String>, _>("phase"),
            "agentModel": input.get("agentModel").and_then(|v| v.as_str()),
            "turn": checkpoint.and_then(|c| c.get("turn")?.as_i64()),
            "finishedAt": if finished.is_empty() { None } else { Some(finished) },
            "lineageId": lineage.lineage_id,
            "attemptNo": lineage.attempt_no,
            "rewoundFromRunId": lineage.rewound_from_run_id,
            "rewoundFromSeq": lineage.rewound_from_seq,
            "outside": {
                "tokens": row.get::<i64, _>("tokens"),
                "pushes": row.get::<Value, _>("pushes"),
                "pullRequests": row.get::<Value, _>("pull_requests"),
            },
        }));
        attempts.push((lineage, run_id));
    }
    let lineages = group_attempts(attempts);
    Ok(Json(json!({ "sessions": sessions, "lineages": lineages })).into_response())
}

fn group_attempts(attempts: Vec<(RunLineage, String)>) -> Vec<Value> {
    let mut groups: Vec<(String, Vec<(i32, String)>)> = Vec::new();
    for (lineage, run_id) in attempts {
        let attempt = (lineage.attempt_no, run_id);
        match groups.iter_mut().find(|(id, _)| *id == lineage.lineage_id) {
            Some((_, runs)) => runs.push(attempt),
            None => groups.push((lineage.lineage_id, vec![attempt])),
        }
    }
    groups
        .into_iter()
        .map(|(lineage_id, mut runs)| {
            runs.sort();
            let runs: Vec<String> = runs.into_iter().map(|(_, run_id)| run_id).collect();
            json!({ "lineageId": lineage_id, "attempts": runs })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(lineage_id: &str, attempt_no: i32, run_id: &str) -> (RunLineage, String) {
        let lineage = RunLineage {
            lineage_id: lineage_id.into(),
            attempt_no,
            rewound_from_run_id: None,
            rewound_from_seq: None,
        };
        (lineage, run_id.into())
    }

    #[test]
    fn attempts_group_by_lineage_in_attempt_order() {
        let grouped = group_attempts(vec![
            attempt("root", 3, "third"),
            attempt("other", 1, "other"),
            attempt("root", 1, "root"),
            attempt("root", 2, "second"),
        ]);
        assert_eq!(
            grouped,
            vec![
                json!({ "lineageId": "root", "attempts": ["root", "second", "third"] }),
                json!({ "lineageId": "other", "attempts": ["other"] }),
            ]
        );
    }
}
