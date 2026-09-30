// /api/research/{id}/members.
// Multiplayer research, mirroring plan membership. GET → { members, active,
// teams } (any member); `active` is who is looking right now. POST { email } →
// share (owner only; grants the report, notifies). DELETE { userId } → unshare
// (owner, or a collaborator leaving). PUT → the presence heartbeat.
//
// PRESENCE MIRRORS THE PLAN'S, key namespace included: a short-lived Redis key
// per (run, viewer) that the open view refreshes. Sharing without presence is
// half a multiplayer surface — you can see WHO has access but never who is here,
// which is the half that makes two people working at once feel like two people.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use redis::AsyncCommands;
use serde_json::json;
use talaria_api_facades::kb::perms::{EditorGrant, list_editors, set_editors};
use talaria_body::{email_member, parse, uuid_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{NotificationInput, add_notification};
use talaria_research::{
    add_research_member, list_research_members, list_research_teams, remove_research_member,
    research_artifact_for, research_role,
};
use talaria_session::require_user;
use talaria_state::AppState;

const PRESENCE_TTL_S: u64 = 60;

fn presence_key(run_id: &str, user_id: &str) -> String {
    format!("research:presence:{run_id}:{user_id}")
}

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
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("research", "GET members", &id) {
        return Ok(gate);
    }
    match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[research] role read on members failed", e)),
    }
    let members = match list_research_members(&state.pg, &id).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[research] member list failed", e)),
    };
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[research] presence redis unavailable", e)),
    };
    let mut active = Vec::new();
    for m in &members {
        let flag: i64 = match conn.exists(presence_key(&id, &m.user_id)).await {
            Ok(n) => n,
            Err(e) => return Ok(internal("[research] presence read failed", e)),
        };
        if flag == 1 {
            active.push(m.user_id.clone());
        }
    }
    Ok(match list_research_teams(&state.pg, &id).await {
        Ok(teams) => {
            Json(json!({ "members": members, "active": active, "teams": teams })).into_response()
        }
        Err(e) => internal("[research] team list failed", e),
    })
}

/// The presence heartbeat. The open view pings while it is mounted; the key
/// expires on its own, so a closed tab stops counting without anyone
/// telling us.
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("research", "PUT members", &id) {
        return Ok(gate);
    }
    match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[research] role read on PUT members failed", e)),
    }
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[research] presence redis unavailable", e)),
    };
    if let Err(e) = conn
        .set_ex::<_, _, ()>(presence_key(&id, &user.id), "1", PRESENCE_TTL_S)
        .await
    {
        return Ok(internal("[research] presence write failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("research", "POST members", &id) {
        return Ok(gate);
    }
    match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(Some("owner")) => {}
        Ok(_) => {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "only the research owner can share it",
            ));
        }
        Err(e) => return Ok(internal("[research] role read on share failed", e)),
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let email = match email_member(obj, "email") {
        Ok(e) => e,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let invited: Option<(String,)> =
        match sqlx::query_as("select id::text from users where lower(email) = $1")
            .bind(email.to_lowercase())
            .fetch_optional(&state.pg)
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(internal("[research] invitee lookup failed", e)),
        };
    let Some((invited_id,)) = invited else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "no user with that email",
        ));
    };
    if invited_id == user.id {
        return Ok(house_error(StatusCode::BAD_REQUEST, "that is you"));
    }
    if let Err(e) = add_research_member(&state.pg, &id, &invited_id).await {
        return Ok(internal("[research] member add failed", e));
    }
    if let Err(e) = sync_report_grant(&state, &id, &invited_id, true).await {
        // The share landed; the grant sync is best-effort. Completion
        // re-grants members.
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
    // The notification is fire-and-forget — a failure here is logged and
    // the share still happened.
    let notify = talaria_notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
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
    Ok(match list_research_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members })).into_response(),
        Err(e) => internal("[research] member list failed", e),
    })
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("research", "DELETE members", &id) {
        return Ok(gate);
    }
    // The role read precedes the body parse, so a bad body from a stranger
    // still answers 404 and never reveals the parse error first.
    let role = match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[research] role read on unshare failed", e)),
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let leaving = match uuid_member(obj, "userId") {
        Ok(u) => u,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The owner removes anyone; a collaborator removes only themselves.
    if role != Some("owner") && !(role == Some("member") && leaving == user.id) {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if let Err(e) = remove_research_member(&state.pg, &id, &leaving).await {
        return Ok(internal("[research] member remove failed", e));
    }
    if let Err(e) = sync_report_grant(&state, &id, &leaving, false).await {
        tracing::error!("[research] report grant sync on unshare failed: {e}");
    }
    Ok(match list_research_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members })).into_response(),
        Err(e) => internal("[research] member list failed", e),
    })
}
