// /api/workbench/jobs. Workbench jobs from the human side. GET ?taskId= →
// the ticket's jobs (board members — this is how the plan-approval gate and
// PR links surface on the ticket). PUT → approve / reject an awaiting job
// (board editors; rejection abandons with the reason in the ticket's audit
// trail), or merge a started job into the repo's testing branch.
//
// This is the TICKET strip's job wire, not workbench-mcp's: agentId absent,
// plan and mergedTestingAt present, testingBranch appended by the handler.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::AssertSqlSafe;
use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;
use crate::boards::{board_role, can_edit};
use crate::body::{as_object, enum_member, optional_max_string_member, parse, uuid_member};
use crate::error::{house_error, thrown_internal_error};
use crate::github as gh;
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::tasks::{get_task, log_activity};
use crate::workbench::mcp::{MergeJob, WorkbenchActor, WorkbenchDeps, merge_job_to_testing};

/// The route's JOB_ROW — the full strip: plan included, mergedTestingAt
/// present, agentId absent (workbench-mcp's row is the other shape).
const JOB_ROW: &str = "id::text, agent_model, task_id::text, repo, branch, effort, plan, \
                       status, pr_url, summary, merged_testing_at, \
                       (trunc(extract(epoch from created_at) * 1000))::bigint, \
                       (trunc(extract(epoch from updated_at) * 1000))::bigint";

type Row = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<i64>,
    i64,
    i64,
);

fn row_wire(r: &Row, testing_branch: Option<String>) -> serde_json::Value {
    json!({
        "id": r.0,
        "agentModel": r.1,
        "taskId": r.2,
        "repo": r.3,
        "branch": r.4,
        "effort": r.5,
        "plan": r.6,
        "status": r.7,
        "prUrl": r.8,
        "summary": r.9,
        "mergedTestingAt": r.10.map(epoch_ms_to_iso),
        "createdAt": epoch_ms_to_iso(r.11),
        "updatedAt": epoch_ms_to_iso(r.12),
        "testingBranch": testing_branch,
    })
}

async fn job_by_id(pg: &PgPool, id: &str) -> Result<Option<Row>, sqlx::Error> {
    sqlx::query_as::<_, Row>(AssertSqlSafe(format!(
        "select {JOB_ROW} from workbench_jobs where id = $1::uuid"
    )))
    .bind(id)
    .fetch_optional(pg)
    .await
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // ?taskId= — absent and bare-'?taskId' both null.
    let task_id = uri
        .query()
        .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("taskId=")));
    let Some(task_id) = task_id else {
        return house_error(StatusCode::BAD_REQUEST, "taskId required");
    };
    let task = match get_task(&state.pg, task_id).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[workbench/jobs] task read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(task) = task else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    // Membership, not editorship: the strip is how a MEMBER watches the plan
    // gate and PR links on their board's ticket.
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[workbench/jobs] board role read failed: {e}");
            return thrown_internal_error();
        }
    };
    if role.is_none() {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let rows: Vec<Row> = match sqlx::query_as::<_, Row>(AssertSqlSafe(format!(
        "select {JOB_ROW} from workbench_jobs where task_id = $1::uuid order by created_at desc"
    )))
    .bind(task_id)
    .fetch_all(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[workbench/jobs] jobs read failed: {e}");
            return thrown_internal_error();
        }
    };
    // Per-row read, fanned out: each wire gets its repo's testing branch
    // appended — testingBranch is null only when the flow read SAYS so; an
    // infra failure is the 500. join_all keeps the rows' order.
    let pg = state.pg.clone();
    let flows = futures_util::future::join_all(rows.iter().map(|r| {
        let pg = pg.clone();
        async move { gh::repo_flow(&pg, &r.3).await }
    }))
    .await;
    let mut wires = Vec::with_capacity(rows.len());
    for (r, flow) in rows.iter().zip(flows) {
        match flow {
            Ok(f) => wires.push(row_wire(r, f.testing_branch)),
            Err(e) => {
                tracing::error!("[workbench/jobs] repo flow read failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    Json(json!({ "jobs": wires })).into_response()
}

pub async fn put(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let job_id = match uuid_member(obj, "jobId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let action = match enum_member(obj, "action", &["approve", "reject", "merge_testing"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let note = match optional_max_string_member(obj, "note", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let job = match job_by_id(&state.pg, &job_id).await {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("[workbench/jobs] job read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(job) = job else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    // A job with no ticket has no board to gate on — this route cannot act on
    // it (the agent's own verbs remain the only doors).
    let mut edit_allowed = false;
    if let Some(task_id) = &job.2
        && let Ok(Some(task)) = get_task(&state.pg, task_id).await
        && let Ok(role) = board_role(&state.pg, &user.id, &task.board_id).await
    {
        edit_allowed = can_edit(role.as_deref());
    }
    if !edit_allowed {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let actor = actor_of(&user);
    if action == "merge_testing" {
        let deps = WorkbenchDeps {
            pg: state.pg.clone(),
            sb: state.secretbox().await.unwrap_or_default(),
            redis: None,
        };
        let merge = MergeJob {
            id: job.0.clone(),
            repo: job.3.clone(),
            branch: job.4.clone(),
            status: job.7.clone(),
            task_id: job.2.clone(),
        };
        // BOTH engine flavors — the tool sentence and the thrown infra
        // error — fold into one 400 {error}.
        let r = merge_job_to_testing(&deps, &merge, &WorkbenchActor::Human(actor.clone())).await;
        return match r {
            Ok(_) => Json(json!({ "ok": true })).into_response(),
            Err(e) => {
                let msg = match e {
                    crate::workbench::mcp::MergeJobError::Fail(m) => m,
                    crate::workbench::mcp::MergeJobError::Throw(m) => m,
                };
                house_error(StatusCode::BAD_REQUEST, &msg)
            }
        };
    }
    if job.7 != "awaiting_approval" {
        return house_error(StatusCode::BAD_REQUEST, &format!("job is {}", job.7));
    }
    let (status, description) = if action == "approve" {
        (
            "started",
            format!(
                "approved the workbench plan — {} may build ({} @ {})",
                job.1, job.3, job.4
            ),
        )
    } else {
        (
            "abandoned",
            match &note {
                Some(n) => format!("rejected the workbench plan: {n} — job abandoned"),
                None => "rejected the workbench plan — job abandoned".to_string(),
            },
        )
    };
    if let Err(e) =
        sqlx::query("update workbench_jobs set status = $1, updated_at = now() where id = $2::uuid")
            .bind(status)
            .bind(&job.0)
            .execute(&state.pg)
            .await
    {
        tracing::error!("[workbench/jobs] job write failed: {e}");
        return thrown_internal_error();
    }
    if let Err(e) = log_activity(
        &state.pg,
        job.2.as_deref().unwrap_or_default(),
        &actor,
        "workbench",
        &description,
    )
    .await
    {
        tracing::error!("[workbench/jobs] activity write failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}
