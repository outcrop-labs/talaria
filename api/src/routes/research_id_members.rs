// /api/research/{id}/members — port of ui/src/routes/api/research.$id.members.ts.
// Multiplayer research, mirroring plan membership. GET → members (any member).
// POST { email } → share (owner only; grants the report, notifies). DELETE
// { userId } → unshare (owner, or a collaborator leaving).

use crate::body::{as_object, email_member, parse, uuid_member};
use crate::error::house_error;
use crate::kb_perms::{EditorGrant, list_editors, set_editors};
use crate::notify::{NotificationInput, add_notification};
use crate::research::{
    add_research_member, list_research_members, remove_research_member, research_artifact_for,
    research_role,
};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Keep the report artifact's grants in step with run membership. The write
/// side of `syncReportGrant`: a member of the run is an editor of the report
/// it produced, from the moment the report exists (completion grants the
/// members a report that lands later).
async fn sync_report_grant(
    state: &AppState,
    run_id: &str,
    user_id: &str,
    present: bool,
) -> Result<(), sqlx::Error> {
    let Some(artifact_id) = research_artifact_for(&state.pg, run_id).await? else {
        return Ok(()); // report not written yet — completion grants members
    };
    let mut grants: Vec<EditorGrant> = list_editors(&state.pg, "artifact", &artifact_id)
        .await?
        .into_iter()
        .filter(|g| !(g.principal_type == "user" && g.principal_id == user_id))
        .collect();
    if present {
        grants.push(EditorGrant {
            principal_type: "user".into(),
            principal_id: user_id.to_string(),
            role: "editor".into(),
        });
    }
    set_editors(&state.pg, "artifact", &artifact_id, &grants).await
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("research", "GET members", &id) {
        return gate;
    }
    match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[research] role read on members failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    }
    match list_research_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members })).into_response(),
        Err(e) => {
            tracing::error!("[research] member list failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("research", "POST members", &id) {
        return gate;
    }
    match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(Some("owner")) => {}
        Ok(_) => {
            return house_error(
                StatusCode::FORBIDDEN,
                "only the research owner can share it",
            );
        }
        Err(e) => {
            tracing::error!("[research] role read on share failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let email = match email_member(obj, "email") {
        Ok(e) => e,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let invited: Option<(String,)> =
        match sqlx::query_as("select id::text from users where lower(email) = $1")
            .bind(email.to_lowercase())
            .fetch_optional(&state.pg)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[research] invitee lookup failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        };
    let Some((invited_id,)) = invited else {
        return house_error(StatusCode::BAD_REQUEST, "no user with that email");
    };
    if invited_id == user.id {
        return house_error(StatusCode::BAD_REQUEST, "that is you");
    }
    if let Err(e) = add_research_member(&state.pg, &id, &invited_id).await {
        tracing::error!("[research] member add failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    if let Err(e) = sync_report_grant(&state, &id, &invited_id, true).await {
        // The share landed; the grant sync is best-effort the way TS's
        // fire-and-forget notification is. Completion re-grants members.
        tracing::error!("[research] report grant sync on share failed: {e}");
    }
    let question: Option<(String,)> =
        match sqlx::query_as("select question from research_runs where id = $1::uuid")
            .bind(&id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[research] question read on share failed: {e}");
                None
            }
        };
    // The notification is fire-and-forget in TS (`void … .catch()`); a
    // failure here is logged and the share still happened.
    let notify = crate::notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = add_notification(
        &notify,
        &invited_id,
        &NotificationInput {
            kind: "research-share",
            title: &format!(
                "{} shared research with you",
                user.name
                    .as_deref()
                    .or(user.email.as_deref())
                    .unwrap_or("Someone")
            ),
            body: Some(question.map(|(q,)| q).as_deref().unwrap_or("")),
            href: Some(&format!("/research/{id}")),
        },
    )
    .await
    {
        tracing::error!("[research] share notification failed: {e}");
    }
    match list_research_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members })).into_response(),
        Err(e) => {
            tracing::error!("[research] member list failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("research", "DELETE members", &id) {
        return gate;
    }
    // TS reads the role BEFORE parsing the body — kept, so a bad body from a
    // stranger still answers 404 and never reveals the parse error first.
    let role = match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[research] role read on unshare failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let leaving = match uuid_member(obj, "userId") {
        Ok(u) => u,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // The owner removes anyone; a collaborator removes only themselves.
    if role != Some("owner") && !(role == Some("member") && leaving == user.id) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    if let Err(e) = remove_research_member(&state.pg, &id, &leaving).await {
        tracing::error!("[research] member remove failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    if let Err(e) = sync_report_grant(&state, &id, &leaving, false).await {
        tracing::error!("[research] report grant sync on unshare failed: {e}");
    }
    match list_research_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members })).into_response(),
        Err(e) => {
            tracing::error!("[research] member list failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}
