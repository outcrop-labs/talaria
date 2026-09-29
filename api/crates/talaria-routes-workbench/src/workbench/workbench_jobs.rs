// /api/workbench/jobs. Workbench jobs from the human side. GET ?taskId= →
// the ticket's jobs (board members — this is how the plan-approval gate and
// PR links surface on the ticket). PUT → approve / reject an awaiting job
// (board editors; rejection abandons with the reason in the ticket's audit
// trail).
//
// This is the TICKET strip's job wire, not workbench-mcp's: agentId absent,
// plan present. Talaria's job ends at the pull request — how a branch is then
// promoted or merged is the consuming repo's own CI and branch policy.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::AssertSqlSafe;
use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_boards::{board_role, can_edit};
use talaria_body::{enum_member, optional_max_string_member, parse, uuid_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;
use talaria_tasks::{get_task, log_activity};

/// The route's JOB_ROW — the full strip: plan included, agentId absent
/// (workbench-mcp's row is the other shape).
const JOB_ROW: &str = "id::text, agent_model, task_id::text, repo, branch, effort, plan, \
                       status, pr_url, summary, \
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
    i64,
    i64,
);

fn row_wire(r: &Row) -> serde_json::Value {
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
        "createdAt": epoch_ms_to_iso(r.10),
        "updatedAt": epoch_ms_to_iso(r.11),
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

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // ?taskId= — absent and bare-'?taskId' both null.
    let task_id = uri
        .query()
        .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("taskId=")));
    let Some(task_id) = task_id else {
        return Ok(house_error(StatusCode::BAD_REQUEST, "taskId required"));
    };
    let task = match get_task(&state.pg, task_id).await {
        Ok(t) => t,
        Err(e) => return Ok(internal("[workbench/jobs] task read failed", e)),
    };
    let Some(task) = task else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    // Membership, not editorship: the strip is how a MEMBER watches the plan
    // gate and PR links on their board's ticket.
    let role = match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[workbench/jobs] board role read failed", e)),
    };
    if role.is_none() {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let rows: Vec<Row> = match sqlx::query_as::<_, Row>(AssertSqlSafe(format!(
        "select {JOB_ROW} from workbench_jobs where task_id = $1::uuid order by created_at desc"
    )))
    .bind(task_id)
    .fetch_all(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[workbench/jobs] jobs read failed", e)),
    };
    let wires: Vec<serde_json::Value> = rows.iter().map(row_wire).collect();
    Ok(Json(json!({ "jobs": wires })).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let job_id = match uuid_member(obj, "jobId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let action = match enum_member(obj, "action", &["approve", "reject"]) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let note = match optional_max_string_member(obj, "note", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let job = match job_by_id(&state.pg, &job_id).await {
        Ok(j) => j,
        Err(e) => return Ok(internal("[workbench/jobs] job read failed", e)),
    };
    let Some(job) = job else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
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
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let actor = actor_of(&user);
    if job.7 != "awaiting_approval" {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &format!("job is {}", job.7),
        ));
    }
    let (status, description) = if action == "approve" {
        // Approving STARTS the job (clone + harness in the agent's container).
        // Pack against host RAM; reject is always allowed.
        if let Err(reason) = talaria_api_facades::fleet::budget::admit_work(
            talaria_api_facades::fleet::budget::effort_reserve(&job.5),
        )
        .await
        {
            if let Some(tid) = &job.2 {
                talaria_work_wait::mark_waiting(&state.pg, tid, &job.1, &reason).await;
            }
            return Ok(house_error(StatusCode::BAD_REQUEST, &reason));
        }
        let started = sqlx::query_scalar::<_, i64>(
            "select count(*) from workbench_jobs \
             where status = 'started' \
               and agent_id = (select agent_id from workbench_jobs where id = $1::uuid)",
        )
        .bind(&job.0)
        .fetch_one(&state.pg)
        .await;
        match started {
            Ok(n)
                if n >= talaria_api_facades::workbench::mcp::MAX_CONCURRENT_JOBS_PER_AGENT
                    as i64 =>
            {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "{} already has {n} live jobs (runaway cap) — finish or abandon one first",
                        job.1
                    ),
                ));
            }
            Ok(_) => {}
            Err(e) => return Ok(internal("[workbench/jobs] job count failed", e)),
        }
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
        return Ok(internal("[workbench/jobs] job write failed", e));
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
        return Ok(internal("[workbench/jobs] activity write failed", e));
    }
    if status == "started"
        && let Ok(Some(department)) =
            sqlx::query_scalar::<_, String>("select department from agent_defs where model = $1")
                .bind(&job.1)
                .fetch_optional(&state.pg)
                .await
    {
        let efforts: Vec<String> = sqlx::query_scalar(
            "select j.effort from workbench_jobs j \
             join agent_defs d on d.id = j.agent_id \
             where j.status = 'started' and d.model = $1",
        )
        .bind(&job.1)
        .fetch_all(&state.pg)
        .await
        .unwrap_or_default();
        talaria_api_facades::fleet::budget::sync_workbench_container(
            &state.pg,
            &department,
            &efforts,
        )
        .await;
    }
    Ok(Json(json!({ "ok": true })).into_response())
}
