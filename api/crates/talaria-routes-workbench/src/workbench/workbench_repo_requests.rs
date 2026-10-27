// /api/workbench/repo-requests. Agent repo-creation requests. GET → pending
// queue; PUT → approve (creates the repo via the App at the requested or
// overridden visibility, protects its default branch against force-push and
// deletion unless the approver says not to, grants it to the requester) or
// reject (the note goes on the requesting ticket). Admin — approval mints
// real org resources.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::AssertSqlSafe;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    enum_member, optional_boolean_member, optional_enum_member, optional_max_string_member, parse,
    uuid_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_github as gh;
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;
use talaria_tasks::log_activity;

const ROW: &str = "id::text, agent_id::text, agent_model, org, name, description, why, \
                   task_id::text, status, decided_by, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint, \
                   visibility, app_slug, responsible_user_id::text";

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
    String,
    Option<String>,
    Option<String>,
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
        "visibility": r.11,
        "appSlug": r.12,
        "responsibleUserId": r.13,
    })
}

/// The ticket line a decision writes. Pure, so its wording is pinned.
fn decision_line(
    action: &str,
    repo: &str,
    agent_model: &str,
    visibility: &str,
    note: Option<&str>,
) -> String {
    match (action, note) {
        ("reject", Some(n)) => {
            format!("rejected the new repo {repo} that {agent_model} asked for: {n}")
        }
        ("reject", None) => format!("rejected the new repo {repo} that {agent_model} asked for"),
        (_, Some(n)) => format!(
            "approved new repo {repo} ({visibility}) — created and granted to {agent_model}: {n}"
        ),
        (_, None) => format!(
            "approved new repo {repo} ({visibility}) — created and granted to {agent_model}"
        ),
    }
}

/// What the approval line adds about the default branch: protected, left
/// unprotected by choice, or the reason GitHub would not protect it.
fn protection_line(protection: &Option<Result<(), String>>) -> String {
    match protection {
        Some(Ok(())) => " Its default branch is protected: no force-push, no deletion.".into(),
        Some(Err(why)) => format!(" Its default branch is NOT protected: {why}."),
        None => " Its default branch was left unprotected, as the approver chose.".into(),
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let rows: Vec<ReqRow> = match sqlx::query_as(AssertSqlSafe(format!(
        "select {ROW} from workbench_repo_requests where status = 'pending' order by created_at"
    )))
    .fetch_all(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[workbench/repo-requests] queue read failed", e)),
    };
    Ok(Json(json!({ "requests": rows.iter().map(req_wire).collect::<Vec<_>>() })).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let id = match uuid_member(obj, "id") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let action = match enum_member(obj, "action", &["approve", "reject"]) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Why the approver decided. A rejection's note is posted on the
    // requesting ticket and shown to the agent in list_repos.
    let note = match optional_max_string_member(obj, "note", 500) {
        Ok(v) => v.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Overrides the visibility the agent asked for; absent keeps it.
    let visibility_override = match optional_enum_member(obj, "visibility", &["private", "public"])
    {
        Ok(v) => v.as_deref().and_then(gh::RepoVisibility::parse),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Protect the new repo's default branch against force-push and deletion
    // (a ruleset: no required reviews, no required checks). On unless the
    // approver says false.
    let protect = match optional_boolean_member(obj, "protect") {
        Ok(v) => v.unwrap_or(true),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let req = match sqlx::query_as::<_, ReqRow>(AssertSqlSafe(format!(
        "select {ROW} from workbench_repo_requests where id = $1::uuid and status = 'pending'"
    )))
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(internal("[workbench/repo-requests] request read failed", e)),
    };
    let Some(req) = req else {
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            "not found or already decided",
        ));
    };
    let actor = actor_of(&user);
    let repo = format!("{}/{}", req.3, req.4);
    if action == "reject" {
        if let Err(e) = sqlx::query(
            "update workbench_repo_requests set status = 'rejected', decided_by = $1, \
             decision_note = $3, updated_at = now() where id = $2::uuid",
        )
        .bind(&actor)
        .bind(&req.0)
        .bind(&note)
        .execute(&state.pg)
        .await
        {
            return Ok(internal("[workbench/repo-requests] reject write failed", e));
        }
        // On the ticket that asked, where the agent working it reads. The
        // decision is already recorded; a failed line is logged, not a 500.
        if let Some(task_id) = &req.7
            && let Err(e) = log_activity(
                &state.pg,
                task_id,
                &actor,
                "workbench",
                &decision_line("reject", &repo, &req.2, &req.11, note.as_deref()),
            )
            .await
        {
            tracing::warn!("[workbench/repo-requests] reject line on {task_id} failed: {e}");
        }
        return Ok(Json(json!({ "ok": true })).into_response());
    }
    let visibility = visibility_override
        .or_else(|| gh::RepoVisibility::parse(&req.11))
        .unwrap_or_default();
    // Approve — any failure folds into one 400 {error}: repo creation, the
    // grant, the decision write, the (only log) activity line. A failure
    // partway through is a 400 with the whatever-steps-happened state
    // behind it.
    let sb = state.secretbox().await.unwrap_or_default();
    let created =
        match gh::create_repo(&state.pg, &sb, &req.3, &req.4, &req.5, visibility, protect).await {
            Ok(c) => c,
            Err(e) => return Ok(house_error(StatusCode::BAD_REQUEST, &e)),
        };
    let (protected, protection_note) = match &created.protection {
        Some(Ok(())) => (Some(true), None),
        Some(Err(why)) => (Some(false), Some(why.clone())),
        None => (
            Some(false),
            Some("not asked for by the approver".to_string()),
        ),
    };
    // The requester gets the repo granted the moment it exists — appended
    // after their existing grants, deduped.
    let mut grants = gh::granted_repos(&state.pg, &req.1).await;
    if !grants.iter().any(|g| g == &created.full_name) {
        grants.push(created.full_name.clone());
    }
    if let Err(e) = gh::set_granted_repos(&state.pg, &req.1, &grants).await {
        return Ok(house_error(StatusCode::BAD_REQUEST, &e));
    }
    if let Err(e) = sqlx::query(
        "update workbench_repo_requests set status = 'approved', decided_by = $1, \
         visibility = $3, decision_note = $4, protected = $5, protection_note = $6, \
         updated_at = now() where id = $2::uuid",
    )
    .bind(&actor)
    .bind(&req.0)
    .bind(visibility.as_str())
    .bind(&note)
    .bind(protected)
    .bind(&protection_note)
    .execute(&state.pg)
    .await
    {
        tracing::error!("[workbench/repo-requests] approve write failed: {e}");
        return Ok(house_error(StatusCode::BAD_REQUEST, &e.to_string()));
    }
    if let Some(task_id) = &req.7 {
        // Best-effort — the ticket's activity line is not load-bearing.
        let _ = log_activity(
            &state.pg,
            task_id,
            &actor,
            "workbench",
            &format!(
                "{}{}",
                decision_line(
                    "approve",
                    &created.full_name,
                    &req.2,
                    visibility.as_str(),
                    note.as_deref(),
                ),
                protection_line(&created.protection)
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
                after: Some(json!({ "protected": protected })),
            },
        )
        .await;
    });
    Ok(Json(json!({
        "ok": true,
        "repo": created.full_name,
        "url": created.url,
        "protected": protected,
        "protection": protection_note,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::{decision_line, protection_line};

    #[test]
    fn the_approval_line_says_whether_the_default_branch_is_protected() {
        assert_eq!(
            protection_line(&Some(Ok(()))),
            " Its default branch is protected: no force-push, no deletion."
        );
        assert_eq!(
            protection_line(&Some(Err("the plan does not allow it".into()))),
            " Its default branch is NOT protected: the plan does not allow it."
        );
        assert!(protection_line(&None).contains("as the approver chose"));
    }

    #[test]
    fn a_decision_line_names_the_repo_the_agent_and_the_reason() {
        assert_eq!(
            decision_line(
                "reject",
                "acme/notes",
                "dev-engineering",
                "private",
                Some("use acme/wiki")
            ),
            "rejected the new repo acme/notes that dev-engineering asked for: use acme/wiki"
        );
        assert_eq!(
            decision_line("reject", "acme/notes", "dev-engineering", "private", None),
            "rejected the new repo acme/notes that dev-engineering asked for"
        );
        assert_eq!(
            decision_line("approve", "acme/notes", "dev-engineering", "public", None),
            "approved new repo acme/notes (public) — created and granted to dev-engineering"
        );
    }
}
