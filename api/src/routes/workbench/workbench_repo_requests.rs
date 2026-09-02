// /api/workbench/repo-requests — port of ui/src/routes/api/workbench.repo-
// requests.ts. Agent repo-creation requests. GET → pending queue; PUT →
// approve (creates the repo via the App, auto-grants it to the requester) or
// reject. Admin — approval mints real org resources.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::AssertSqlSafe;

use crate::agent_auth::epoch_ms_to_iso;
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, enum_member, parse, uuid_member};
use crate::error::{house_error, thrown_internal_error};
use crate::github as gh;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use crate::tasks::log_activity;

const ROW: &str = "id::text, agent_id::text, agent_model, org, name, description, why, \
                   task_id::text, status, decided_by, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint";

type ReqRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    i64,
);

fn req_wire(r: &ReqRow) -> serde_json::Value {
    json!({
        "id": r.0,
        "agentId": r.1,
        "agentModel": r.2,
        "org": r.3,
        "name": r.4,
        "description": r.5,
        "why": r.6,
        "taskId": r.7,
        "status": r.8,
        "decidedBy": r.9,
        "createdAt": epoch_ms_to_iso(r.10),
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let rows: Vec<ReqRow> = match sqlx::query_as(AssertSqlSafe(format!(
        "select {ROW} from workbench_repo_requests where status = 'pending' order by created_at"
    )))
    .fetch_all(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[workbench/repo-requests] queue read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "requests": rows.iter().map(req_wire).collect::<Vec<_>>() })).into_response()
}

pub async fn put(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let id = match uuid_member(obj, "id") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let action = match enum_member(obj, "action", &["approve", "reject"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let req = match sqlx::query_as::<_, ReqRow>(AssertSqlSafe(format!(
        "select {ROW} from workbench_repo_requests where id = $1::uuid and status = 'pending'"
    )))
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[workbench/repo-requests] request read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(req) = req else {
        return house_error(StatusCode::NOT_FOUND, "not found or already decided");
    };
    let actor = actor_of(&user);
    if action == "reject" {
        if let Err(e) = sqlx::query(
            "update workbench_repo_requests set status = 'rejected', decided_by = $1, \
             updated_at = now() where id = $2::uuid",
        )
        .bind(&actor)
        .bind(&req.0)
        .execute(&state.pg)
        .await
        {
            tracing::error!("[workbench/repo-requests] reject write failed: {e}");
            return thrown_internal_error();
        }
        return Json(json!({ "ok": true })).into_response();
    }
    // Approve — everything inside the TS `try` folds into one 400 {error} on
    // any failure: repo creation, the grant, the decision write, the (only
    // log) activity line. A failure partway through is a 400 with the
    // whatever-steps-happened state behind it, exactly like the TS.
    let sb = state.secretbox().await.unwrap_or_default();
    let created = match gh::create_repo(&state.pg, &sb, &req.3, &req.4, &req.5).await {
        Ok(c) => c,
        Err(e) => return house_error(StatusCode::BAD_REQUEST, &e),
    };
    // The requester gets the repo granted the moment it exists — appended
    // after their existing grants, deduped.
    let mut grants = gh::granted_repos(&state.pg, &req.1).await;
    if !grants.iter().any(|g| g == &created.full_name) {
        grants.push(created.full_name.clone());
    }
    if let Err(e) = gh::set_granted_repos(&state.pg, &req.1, &grants).await {
        return house_error(StatusCode::BAD_REQUEST, &e);
    }
    if let Err(e) = sqlx::query(
        "update workbench_repo_requests set status = 'approved', decided_by = $1, \
         updated_at = now() where id = $2::uuid",
    )
    .bind(&actor)
    .bind(&req.0)
    .execute(&state.pg)
    .await
    {
        tracing::error!("[workbench/repo-requests] approve write failed: {e}");
        return house_error(StatusCode::BAD_REQUEST, &e.to_string());
    }
    if let Some(task_id) = &req.7 {
        // .catch(() => {}) — the ticket's activity line is best-effort.
        let _ = log_activity(
            &state.pg,
            task_id,
            &actor,
            "workbench",
            &format!(
                "approved new repo {} — created and granted to {}",
                created.full_name, req.2
            ),
        )
        .await;
    }
    let pg = state.pg.clone();
    let target_id = created.full_name.clone();
    let audit_actor = actor.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &audit_actor,
                action: "workbench.repo_create",
                target_type: "workbench",
                target_id: Some(&target_id),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "ok": true, "repo": created.full_name, "url": created.url })).into_response()
}
