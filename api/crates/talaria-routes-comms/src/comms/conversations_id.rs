// /api/conversations/{id}.
// GET → the conversation + its messages (ownership-checked). PATCH { title }
// → rename (owner, or a plan collaborator). A renamed title no longer matches
// the mechanical first-message truncation, so the Titler and its sweep leave
// it alone from then on. PATCH { archived: false } restores a plan the owner
// archived — the one way back, and owner-only, because archive hides the row
// from every member's list. DELETE archives (owner); ?hard=1 hard-deletes
// (owner). Decay never sets this flag on a plan (it selects kind = 'chat'),
// so archived on a plan means a person put it away.
//
// Research runs are not archived here. A run is ephemeral: delete is its stop.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use talaria_body::{optional_boolean_member, trimmed_string_member};
use talaria_conversations::get_conversation;
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{
    NotifyDeps, conversation_audience_ids, fan_conversation_event, fan_known_conversation,
};
use talaria_plan_drafts::drop_draft;
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

/// The access gate IS the read: a mutation runs only on a conversation the
/// GET would show this caller. None is 404 — a stranger probing ids learns
/// nothing. The role comes back with that read, so owner-only arms don't
/// take a second query.
async fn visible_role(
    state: &AppState,
    user_id: &str,
    id: &str,
    where_: &str,
) -> Result<String, Response> {
    match get_conversation(&state.pg, user_id, id).await {
        Ok(Some((detail, _))) => Ok(detail.role),
        Ok(None) => Err(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => Err(internal(where_, e)),
    }
}

async fn fan(state: &AppState, id: &str) {
    fan_conversation_event(
        NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
        id.to_string(),
    );
}

/// ?hard=1 — exact match — hard-deletes. Any other value, including absent,
/// archives. Same spelling as DELETE /api/channels/{id}.
fn hard_delete(uri: &Uri) -> bool {
    uri.query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "hard")
                .map(|(_, v)| v.into_owned())
        })
        .as_deref()
        == Some("1")
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let role = visible_role(&state, &user.id, &id, "[conversations] gate read failed").await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let title = if obj.contains_key("title") {
        match trimmed_string_member(obj, "title", 1, 120) {
            Ok(t) => Some(t),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    } else {
        None
    };
    let archived = match optional_boolean_member(obj, "archived") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if title.is_none() && archived.is_none() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "expected title or archived",
        ));
    }
    // Archive is DELETE. PATCH only restores, so there is one door that
    // hides a plan and one door that brings it back.
    if archived == Some(true) {
        return Ok(house_error(StatusCode::BAD_REQUEST, "archive with DELETE"));
    }
    // Restore hides the row from every member. A collaborator must not be
    // able to put back what the owner put away.
    if archived.is_some() && role != "owner" {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if let Some(title) = &title {
        let updated = sqlx::query("update conversations set title = $1 where id = $2::uuid")
            .bind(title)
            .bind(&id)
            .execute(&state.pg)
            .await;
        if let Err(e) = updated {
            return Ok(internal("[conversations] rename failed", e));
        }
    }
    if archived == Some(false) {
        let updated = sqlx::query(
            "update conversations set archived = false, updated_at = now() where id = $1::uuid",
        )
        .bind(&id)
        .execute(&state.pg)
        .await;
        if let Err(e) = updated {
            return Ok(internal("[conversations] restore failed", e));
        }
    }
    fan(&state, &id).await;
    Ok((StatusCode::OK, Json(OkTrue { ok: true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let role = visible_role(
        &state,
        &user.id,
        &id,
        "[conversations] gate read on delete failed",
    )
    .await?;
    // Both arms are owner-only, the channel pattern: a collaborator renaming
    // a shared plan is fine, hiding it from the owner is not.
    if role != "owner" {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if hard_delete(&uri) {
        // Resolve the audience before the row goes. The usual fan reads that
        // row, and a spawn racing the delete would ring nobody.
        let audience = match conversation_audience_ids(&state.pg, &id).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("[conversations] audience read on delete failed: {e}");
                Vec::new()
            }
        };
        // Cancel a live draft before the conversation disappears, so a job
        // mid-spend stops at its next boundary instead of writing into a
        // deleted plan. Older draft rows have no FK — sweep them too.
        if let Err(e) = drop_draft(&state, &id).await {
            return Ok(internal("[conversations] draft cancel failed", e));
        }
        if let Err(e) = sqlx::query("delete from plan_drafts where conversation_id = $1::uuid")
            .bind(&id)
            .execute(&state.pg)
            .await
        {
            return Ok(internal("[conversations] draft sweep failed", e));
        }
        if let Err(e) = sqlx::query("delete from conversations where id = $1::uuid")
            .bind(&id)
            .execute(&state.pg)
            .await
        {
            return Ok(internal("[conversations] delete failed", e));
        }
        fan_known_conversation(
            NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
            id.clone(),
            audience,
        );
    } else {
        if let Err(e) = sqlx::query(
            "update conversations set archived = true, updated_at = now() where id = $1::uuid",
        )
        .bind(&id)
        .execute(&state.pg)
        .await
        {
            return Ok(internal("[conversations] archive failed", e));
        }
        fan(&state, &id).await;
    }
    Ok((StatusCode::OK, Json(OkTrue { ok: true })).into_response())
}
