// /api/boards/{id}/members. GET → the member list. POST { email, role } →
// share; DELETE { userId |
// email } → unshare. Agents allowed on the board may READ membership (they
// would mutate it blind otherwise); the writes stay identity-proxied — a
// personal assistant acts as its owner alongside signed-in humans.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::agent_caller;
use talaria_audit::{AuditEntry, log_audit};
use talaria_boards::{
    ShareOutcome, board_allows_agent, board_role, can_edit, list_members, share_board,
    unshare_board,
};
use talaria_body::{email_member, enum_member, optional_uuid_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{ActingUser, acting_user, require_user, unauthorized};
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return get_as_user(&state, &headers, &id).await,
        Err(resp) => return Err(resp),
    };
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET members", &id) {
        return Ok(gate);
    }
    // The CALLER, not its model: board policy falls back to the
    // elevated-assistant bypass (org-wide reach), and a bare string reads as
    // proven — passing `caller.model` would discard `legacy` silently.
    let subject = talaria_agent_auth::AgentSubject::Caller(caller);
    let allowed = match board_allows_agent(&state.pg, &id, &subject).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[boards] agent gate on members failed", e)),
    };
    if !allowed {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let members = match list_members(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[boards] member list failed", e)),
    };
    Ok(Json(json!({ "members": members })).into_response())
}

async fn get_as_user(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
) -> Result<Response, Response> {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return Err(gate),
    };
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET members", id) {
        return Ok(gate);
    }
    match board_role(&state.pg, &user.id, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[boards] role read on members failed", e)),
    }
    let members = match list_members(&state.pg, id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[boards] member list failed", e)),
    };
    Ok(Json(json!({ "members": members })).into_response())
}

async fn write_gate(state: &AppState, user: &ActingUser, id: &str) -> Option<Response> {
    match board_role(&state.pg, &user.id, id).await {
        Ok(role) if can_edit(role.as_deref()) || user.elevated => None,
        Ok(_) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => Some(internal("[boards] role read on member write failed", e)),
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return Ok(unauthorized()),
        Err(gate) => return Ok(gate),
    };
    if let Some(gate) = talaria_params::uuid_gate("boards", "POST members", &id) {
        return Ok(gate);
    }
    if let Some(gate) = write_gate(&state, &user, &id).await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let email = match email_member(obj, "email") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // role: absent shares as an editor; present must name one of the two
    // non-owner roles.
    let role = match obj.get("role") {
        None => "editor".to_string(),
        Some(_) => match enum_member(obj, "role", &["editor", "viewer"]) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
    };
    match share_board(&state.pg, &id, &email, &role).await {
        Ok(ShareOutcome::Shared) => {}
        Ok(ShareOutcome::Refused(msg)) => return Ok(house_error(StatusCode::BAD_REQUEST, msg)),
        Err(e) => return Ok(internal("[boards] share failed", e)),
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &user.label,
            action: "board.member_add",
            target_type: "board",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(json!({ "email": email, "role": role })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = match acting_user(&state, &headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return Ok(unauthorized()),
        Err(gate) => return Ok(gate),
    };
    if let Some(gate) = talaria_params::uuid_gate("boards", "DELETE members", &id) {
        return Ok(gate);
    }
    if let Some(gate) = write_gate(&state, &user, &id).await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Both members optional; the 'userId or email required' refine runs
    // AFTER the field checks.
    let mut user_id = match optional_uuid_member(obj, "userId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let email = match obj.get("email") {
        None => None,
        Some(_) => match email_member(obj, "email") {
            Ok(v) => Some(v),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
    };
    if user_id.is_none() && email.is_none() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "userId or email required",
        ));
    }
    // The email path resolves to an id here — no trim on this lookup (the
    // share route trims).
    if user_id.is_none()
        && let Some(email) = &email
    {
        let found = match sqlx::query_as::<_, (String,)>(
            "select id::text from users where lower(email) = $1",
        )
        .bind(email.to_lowercase())
        .fetch_optional(&state.pg)
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(internal("[boards] email lookup failed", e)),
        };
        let Some(row) = found else {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "no user with that email",
            ));
        };
        user_id = Some(row.0);
    }
    let Some(user_id) = user_id else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "userId or email required",
        ));
    };
    if let Err(e) = unshare_board(&state.pg, &id, &user_id).await {
        return Ok(internal("[boards] unshare failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &user.label,
            action: "board.member_remove",
            target_type: "board",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(json!({ "userId": user_id })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}
