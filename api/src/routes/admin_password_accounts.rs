// /api/admin/password-accounts — port of
// ui/src/routes/api/admin.password-accounts.ts. The admin console's API for
// DB-backed password accounts (Admin → People).
//   GET    → the account list.   POST → create an account.
//   PUT    → set/reset a password. DELETE → remove the account (the person
//   stays). Audit entries carry the email, never the password or its hash.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, optional_string_member, parse, preprocessed_email_member, string_member, uuid_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::password_accounts::{
    WriteRefusal, create_password_account, list_password_accounts, remove_password_account,
    set_password_account_password,
};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    match list_password_accounts(&state.pg).await {
        Ok(accounts) => Json(json!({ "accounts": accounts })).into_response(),
        Err(e) => {
            tracing::error!("[admin/password-accounts] list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let email = match preprocessed_email_member(obj, "email", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let password = match string_member(obj, "password", 8, 1000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match optional_string_member(obj, "name", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let result = match create_password_account(&state.pg, &email, &password, name.as_deref()).await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[admin/password-accounts] create failed: {e}");
            return thrown_internal_error();
        }
    };
    let user_id = match result {
        Ok(id) => id,
        Err(WriteRefusal::EmailTaken) => {
            return house_error(
                StatusCode::CONFLICT,
                "An account with that email already exists",
            );
        }
        Err(_) => return house_error(StatusCode::BAD_REQUEST, "Could not create the account"),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "user.password_add",
            target_type: "user",
            target_id: Some(&user_id),
            target_label: Some(&email),
            before: None,
            after: Some(json!({ "email": email })),
        },
    )
    .await;
    Json(json!({ "ok": true, "userId": user_id })).into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let user_id = match uuid_member(obj, "userId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let password = match string_member(obj, "password", 8, 1000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let result = match set_password_account_password(&state.pg, &user_id, &password).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[admin/password-accounts] set failed: {e}");
            return thrown_internal_error();
        }
    };
    let email = match result {
        Ok(email) => email,
        Err(reason) => {
            let (error, status) = match reason {
                WriteRefusal::EmailTaken => (
                    "That email already belongs to another password account",
                    StatusCode::CONFLICT,
                ),
                WriteRefusal::NoEmail => (
                    "That user has no email to hang a password account from",
                    StatusCode::BAD_REQUEST,
                ),
                WriteRefusal::NotFound => ("No such user", StatusCode::NOT_FOUND),
            };
            return house_error(status, error);
        }
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "user.password_set",
            target_type: "user",
            target_id: Some(&user_id),
            target_label: Some(&email),
            before: None,
            after: Some(json!({ "email": email })),
        },
    )
    .await;
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let user_id = match uuid_member(obj, "userId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let email = match remove_password_account(&state.pg, &user_id).await {
        Ok(Some(email)) => email,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "No password account for that user"),
        Err(e) => {
            tracing::error!("[admin/password-accounts] remove failed: {e}");
            return thrown_internal_error();
        }
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "user.password_remove",
            target_type: "user",
            target_id: Some(&user_id),
            target_label: Some(&email),
            before: None,
            after: Some(json!({ "email": email })),
        },
    )
    .await;
    Json(json!({ "ok": true })).into_response()
}
