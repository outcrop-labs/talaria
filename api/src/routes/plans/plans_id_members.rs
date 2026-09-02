// /api/plans/{id}/members — port of ui/src/routes/api/plans.$id.members.ts.
// Multiplayer plan membership + presence.
//   GET    → { members, active } — any member; active = user ids seen in the
//            last minute (Redis presence keys, 60s TTL).
//   POST   { email }  → share (owner only; grants the doc, notifies them).
//   DELETE { userId } → unshare (owner, or a collaborator leaving).
//   PUT    → presence ping (any member).

use crate::body::{as_object, email_member, parse, uuid_member};
use crate::conversations::{add_plan_member, list_plan_members, plan_role, remove_plan_member};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{EditorGrant, list_editors, set_editors};
use crate::notify::{NotificationInput, NotifyDeps, add_notification};
use crate::params::uuid_gate;
use crate::plan_doc::plan_doc_for;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use redis::AsyncCommands;
use serde_json::{Value, json};

const PRESENCE_TTL_S: u64 = 60;

fn presence_key(plan_id: &str, user_id: &str) -> String {
    format!("plan:presence:{plan_id}:{user_id}")
}

fn members_json(members: &[crate::conversations::PlanMember]) -> Vec<Value> {
    members
        .iter()
        .map(|m| {
            json!({
                "userId": m.user_id,
                "name": m.name,
                "email": m.email,
                "role": m.role,
            })
        })
        .collect()
}

/// Keep the plan doc's editor grants in step with membership (syncDocGrant).
/// A db failure here is a 500 in the TS (the call is awaited bare), not a
/// silent skip — a share that granted the row but not the doc leaves the new
/// collaborator able to see the plan and unable to open its document.
async fn sync_doc_grant(
    pg: &sqlx::PgPool,
    plan_id: &str,
    user_id: &str,
    present: bool,
) -> Result<(), sqlx::Error> {
    let Some(doc) = plan_doc_for(pg, plan_id).await? else {
        return Ok(());
    };
    let mut grants: Vec<EditorGrant> = list_editors(pg, "artifact", &doc.id)
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
    set_editors(pg, "artifact", &doc.id, &grants).await
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
    if let Some(gate) = uuid_gate("plans", "GET members", &id) {
        return gate;
    }
    match plan_role(&state.pg, &user.id, &id).await {
        // Nothing and "not a plan" are the same answer here.
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[plans] plan role read on GET members failed: {e}");
            return thrown_internal_error();
        }
    }
    let members = match list_plan_members(&state.pg, &id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[plans] member read failed: {e}");
            return thrown_internal_error();
        }
    };
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[plans] presence redis unavailable: {e}");
            return thrown_internal_error();
        }
    };
    let mut active = Vec::new();
    for m in &members {
        let flag: i64 = match conn.exists(presence_key(&id, &m.user_id)).await {
            Ok(n) => n,
            Err(e) => {
                tracing::error!("[plans] presence read failed: {e}");
                return thrown_internal_error();
            }
        };
        if flag == 1 {
            active.push(m.user_id.clone());
        }
    }
    Json(json!({ "members": members_json(&members), "active": active })).into_response()
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
    if let Some(gate) = uuid_gate("plans", "POST members", &id) {
        return gate;
    }
    match plan_role(&state.pg, &user.id, &id).await {
        Ok(Some(role)) if role == "owner" => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "only the plan owner can share it"),
        Err(e) => {
            tracing::error!("[plans] plan role read on POST members failed: {e}");
            return thrown_internal_error();
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
                tracing::error!("[plans] invitee lookup failed: {e}");
                return thrown_internal_error();
            }
        };
    let Some((invited_id,)) = invited else {
        return house_error(StatusCode::BAD_REQUEST, "no user with that email");
    };
    if invited_id == user.id {
        return house_error(StatusCode::BAD_REQUEST, "that is you");
    }
    if let Err(e) = add_plan_member(&state.pg, &id, &invited_id).await {
        tracing::error!("[plans] member add failed: {e}");
        return thrown_internal_error();
    }
    if let Err(e) = sync_doc_grant(&state.pg, &id, &invited_id, true).await {
        tracing::error!("[plans] doc grant on share failed: {e}");
        return thrown_internal_error();
    }
    let plan_title: Option<(Option<String>,)> =
        match sqlx::query_as("select title from conversations where id = $1::uuid")
            .bind(&id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[plans] plan title read failed: {e}");
                return thrown_internal_error();
            }
        };
    // `.catch(() => {})` — the notification is best-effort; the share itself
    // already happened.
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let who = user
        .name
        .clone()
        .or(user.email)
        .unwrap_or_else(|| "Someone".into());
    let _ = add_notification(
        &notify,
        &invited_id,
        &NotificationInput {
            kind: "plan-share",
            title: &format!("{who} added you to a plan"),
            body: plan_title
                .and_then(|(title,)| title)
                .as_deref()
                .or(Some("Untitled plan")),
            href: Some(&format!("/plan/{id}")),
        },
    )
    .await;
    match list_plan_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members_json(&members) })).into_response(),
        Err(e) => {
            tracing::error!("[plans] member re-read failed: {e}");
            thrown_internal_error()
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
    if let Some(gate) = uuid_gate("plans", "DELETE members", &id) {
        return gate;
    }
    // TS reads the role BEFORE parsing the body — keep the order: the role
    // read has no failure surface of its own, and the body's 400 comes ahead
    // of the permission check.
    let role = match plan_role(&state.pg, &user.id, &id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[plans] plan role read on DELETE members failed: {e}");
            return thrown_internal_error();
        }
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let target = match uuid_member(obj, "userId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Owner removes anyone; a collaborator may remove only themself (leave).
    if role.as_deref() != Some("owner")
        && !(role.as_deref() == Some("collaborator") && target == user.id)
    {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    if let Err(e) = remove_plan_member(&state.pg, &id, &target).await {
        tracing::error!("[plans] member remove failed: {e}");
        return thrown_internal_error();
    }
    if let Err(e) = sync_doc_grant(&state.pg, &id, &target, false).await {
        tracing::error!("[plans] doc grant on unshare failed: {e}");
        return thrown_internal_error();
    }
    match list_plan_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members_json(&members) })).into_response(),
        Err(e) => {
            tracing::error!("[plans] member re-read failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate("plans", "PUT members", &id) {
        return gate;
    }
    match plan_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[plans] plan role read on PUT members failed: {e}");
            return thrown_internal_error();
        }
    }
    let mut conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[plans] presence redis unavailable: {e}");
            return thrown_internal_error();
        }
    };
    let key = presence_key(&id, &user.id);
    if let Err(e) = conn.set_ex::<_, _, ()>(key, "1", PRESENCE_TTL_S).await {
        tracing::error!("[plans] presence write failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}
