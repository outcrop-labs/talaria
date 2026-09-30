// /api/artifacts/{id}/comments. Comment threads on a Talaria document. GET →
// all comments (the client assembles threads). POST { content, parentId?,
// quote? } → comment or reply.
//
// The twin of `kb_docs_id_comments`, on the same engine and the same table.
// What differs is only the gate: an artifact's read permission is answered by
// the artifact's own row, the way every other artifact route answers it, rather
// than by the knowledge-base's doc permissions. 404-as-ACL, like the doc route:
// something you cannot read does not exist as far as this route is concerned.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::kb::comments::{CommentTarget, NewComment, add_comment, list_comments};
use talaria_api_facades::kb::perms::{ITEM_ARTIFACT, can_read, list_editors};
use talaria_artifacts::{get_artifact, guarded};
use talaria_body::{optional_uuid_member, parse, trimmed_string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

/// The artifact, if this caller may read it. `None` is the route's 404 — the
/// same answer for "no such artifact" and "not yours", so probing ids learns
/// nothing.
async fn readable(
    state: &AppState,
    id: &str,
    user: &talaria_session::SessionUser,
) -> Result<Option<talaria_artifacts::Artifact>, Response> {
    let artifact = match get_artifact(&state.pg, id).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(None),
        Err(e) => return Err(internal("[artifacts] read for comments failed", e)),
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, id).await {
        Ok(v) => v,
        Err(e) => return Err(internal("[artifacts] editor read for comments failed", e)),
    };
    let team_ids = match talaria_teams::team_ids_for_user(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Err(internal("[artifacts] team read for comments failed", e)),
    };
    if !can_read(
        &guarded(&artifact),
        Some(&user.id),
        who_of(user).as_deref(),
        &editors,
        &team_ids,
    ) {
        return Ok(None);
    }
    Ok(Some(artifact))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if readable(&state, &id, &user).await?.is_none() {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    Ok(
        match list_comments(&state.pg, CommentTarget::Artifact(&id)).await {
            Ok(comments) => Json(json!({ "comments": comments })).into_response(),
            Err(e) => internal("[artifacts] comment list failed", e),
        },
    )
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let Some(artifact) = readable(&state, &id, &user).await? else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // The same bounds the doc route uses, trimmed-then-validated so the length
    // applies to what is actually stored.
    let content = match trimmed_string_member(obj, "content", 1, 8_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let parent_id = match optional_uuid_member(obj, "parentId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let quote = match obj.get("quote") {
        None | Some(serde_json::Value::Null) => None, // nullish
        Some(_) => Some(match trimmed_string_member(obj, "quote", 0, 500) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    Ok(
        match add_comment(
            &state.pg,
            &notify,
            &NewComment {
                target: CommentTarget::Artifact(&id),
                target_title: artifact.title.as_str(),
                target_owner_user_id: artifact.owner_user_id.as_deref(),
                parent_id: parent_id.as_deref(),
                author_user_id: &user.id,
                author: user
                    .name
                    .as_deref()
                    .or(user.email.as_deref())
                    .unwrap_or("user"),
                quote: quote.as_deref(),
                content: &content,
            },
        )
        .await
        {
            Ok(comment) => Json(json!({ "comment": comment })).into_response(),
            Err(e) => internal("[artifacts] comment insert failed", e),
        },
    )
}
