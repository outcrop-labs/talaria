// /api/conversations/{id}.
// GET → the conversation + its messages (ownership-checked). PATCH { title }
// → rename (owner, or a plan collaborator). A renamed title no longer matches
// the mechanical first-message truncation, so the Titler and its sweep leave
// it alone from then on.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_body::trimmed_string_member;
use talaria_conversations::get_conversation;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct DetailEnvelope {
    conversation: talaria_conversations::ConversationDetail,
    messages: Vec<talaria_conversations::MessageRow>,
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match get_conversation(&state.pg, &user.id, &id).await {
        Ok(Some((conversation, messages))) => (
            StatusCode::OK,
            Json(DetailEnvelope {
                conversation,
                messages,
            }),
        )
            .into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => internal("[conversations] detail read failed", e),
    })
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
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // The access gate IS the read: the PATCH runs only on a conversation the
    // GET would show this caller (owner, or a plan collaborator).
    match get_conversation(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[conversations] gate read failed", e)),
    }
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let title = match trimmed_string_member(obj, "title", 1, 120) {
        Ok(t) => t,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let updated = sqlx::query("update conversations set title = $1 where id = $2::uuid")
        .bind(&title)
        .bind(&id)
        .execute(&state.pg)
        .await;
    Ok(match updated {
        Ok(_) => (StatusCode::OK, Json(OkTrue { ok: true })).into_response(),
        Err(e) => internal("[conversations] rename failed", e),
    })
}
