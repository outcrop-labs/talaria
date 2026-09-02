// /api/conversations/{id}.
// GET → the conversation + its messages (ownership-checked). PATCH { title }
// → rename (owner, or a plan collaborator). A renamed title no longer matches
// the mechanical first-message truncation, so the Titler and its sweep leave
// it alone from then on.

use crate::body::{as_object, trimmed_string_member};
use crate::conversations::get_conversation;
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct DetailEnvelope {
    conversation: crate::conversations::ConversationDetail,
    messages: Vec<crate::conversations::MessageRow>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match get_conversation(&state.pg, &user.id, &id).await {
        Ok(Some((conversation, messages))) => (
            StatusCode::OK,
            Json(DetailEnvelope {
                conversation,
                messages,
            }),
        )
            .into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[conversations] detail read failed: {e}");
            thrown_internal_error()
        }
    }
}

#[derive(serde::Serialize)]
struct OkTrue {
    ok: bool,
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    // The access gate IS the read: the PATCH runs only on a conversation the
    // GET would show this caller (owner, or a plan collaborator).
    match get_conversation(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[conversations] gate read failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let title = match trimmed_string_member(obj, "title", 1, 120) {
        Ok(t) => t,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let updated = sqlx::query("update conversations set title = $1 where id = $2::uuid")
        .bind(&title)
        .bind(&id)
        .execute(&state.pg)
        .await;
    match updated {
        Ok(_) => (StatusCode::OK, Json(OkTrue { ok: true })).into_response(),
        Err(e) => {
            tracing::error!("[conversations] rename failed: {e}");
            thrown_internal_error()
        }
    }
}
