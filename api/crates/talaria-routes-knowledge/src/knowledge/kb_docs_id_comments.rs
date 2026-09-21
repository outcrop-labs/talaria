// /api/kb/docs/{id}/comments. Doc comment threads. GET → all comments (client
// assembles threads). POST { content, parentId?, quote? } → comment/reply.
// Read access to the doc is the gate for both — discussion is part of the
// document. 404-as-ACL: a doc you can't discuss doesn't exist as far as this
// route is concerned.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::kb::comments::{NewComment, add_comment, can_discuss_doc, list_comments};
use talaria_body::{optional_uuid_member, parse, trimmed_string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let who = who_of(&user);
    if !can_discuss_doc(&state.pg, &id, &user.id, who.as_deref()).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    Ok(match list_comments(&state.pg, &id).await {
        Ok(comments) => Json(json!({ "comments": comments })).into_response(),
        Err(e) => internal("[kb] comment list failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let who = who_of(&user);
    if !can_discuss_doc(&state.pg, &id, &user.id, who.as_deref()).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // content/quote are trim-then-validate members — the length bounds apply
    // to the TRIMMED value, which is also what gets stored.
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
                doc_id: &id,
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
            Err(e) => internal("[kb] comment insert failed", e),
        },
    )
}
