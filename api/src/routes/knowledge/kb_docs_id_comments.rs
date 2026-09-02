// /api/kb/docs/{id}/comments — port of ui/src/routes/api/kb.docs.$id.comments.ts.
// Doc comment threads. GET → all comments (client assembles threads).
// POST { content, parentId?, quote? } → comment/reply. Read access to the doc
// is the gate for both — discussion is part of the document. 404-as-ACL: a
// doc you can't discuss doesn't exist as far as this route is concerned.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::body::{as_object, optional_uuid_member, parse, trimmed_string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::comments::{NewComment, add_comment, can_discuss_doc, list_comments};
use crate::notify::NotifyDeps;
use crate::session::{require_user, who_of};
use crate::state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let who = who_of(&user);
    if !can_discuss_doc(&state.pg, &id, &user.id, who.as_deref()).await {
        return house_error(StatusCode::NOT_FOUND, "not found");
    }
    match list_comments(&state.pg, &id).await {
        Ok(comments) => Json(json!({ "comments": comments })).into_response(),
        Err(e) => {
            tracing::error!("[kb] comment list failed: {e}");
            thrown_internal_error()
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
    let who = who_of(&user);
    if !can_discuss_doc(&state.pg, &id, &user.id, who.as_deref()).await {
        return house_error(StatusCode::NOT_FOUND, "not found");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // content/quote are `.trim()` members — zod trims BEFORE the length
    // checks and the TRIMMED value is what parseBody hands through.
    let content = match trimmed_string_member(obj, "content", 1, 8_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let parent_id = match optional_uuid_member(obj, "parentId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let quote = match obj.get("quote") {
        None | Some(serde_json::Value::Null) => None, // nullish
        Some(_) => Some(match trimmed_string_member(obj, "quote", 0, 500) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        }),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
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
        Err(e) => {
            tracing::error!("[kb] comment insert failed: {e}");
            thrown_internal_error()
        }
    }
}
