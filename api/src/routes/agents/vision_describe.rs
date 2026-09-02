// /api/vision/describe.
//
// READ AN IMAGE ON BEHALF OF A MODEL THAT CANNOT — the endpoint behind the
// `describe_image` tool.
//
// THE ACCESS CHECK IS THE WHOLE SECURITY STORY HERE, and it is not this file's
// to invent: `canAccessUpload` already answers "may this principal read this
// upload" for every attachment path, board policy included. An endpoint that
// resolved an upload id itself would be a second answer to that question, and
// the second answer is always the one that turns out to be wrong.
//
// WHAT COMES BACK IS ATTRIBUTED. The description carries the model that produced
// it, because the calling agent is about to treat it as fact and a surface that
// presents it as the caller's own observation is lying by omission.

use crate::agent_auth::agent_caller;
use crate::body::{as_object, parse, string_member};
use crate::error::house_error;
use crate::session::require_user;
use crate::state::AppState;
use crate::uploads::{UploadViewer, can_access_upload, get_upload};
use crate::vision::describe_image;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // The viewer borrows its identity, so the identity lives HERE rather
    // than inside the arm that built it.
    let agent_model: String;
    let human: crate::session::SessionUser;
    let viewer = match caller {
        Some(caller) => {
            agent_model = caller.model;
            UploadViewer::Agent {
                model: &agent_model,
            }
        }
        None => {
            human = match require_user(&state, &headers).await {
                Ok(u) => u,
                Err(gate) => return gate,
            };
            UploadViewer::Human {
                user_id: &human.id,
                who: human.email.as_deref(),
                is_admin: human.role == "admin",
            }
        }
    };

    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let upload_id = match string_member(obj, "uploadId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let question = match string_member(obj, "question", 3, 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // ASKED BEFORE THE BYTES ARE FETCHED, and asked of the one function that
    // already answers it everywhere else.
    if !can_access_upload(&state.pg, &upload_id, viewer).await {
        return house_error(
            StatusCode::NOT_FOUND,
            "no attachment with that id, or you are not allowed to read it",
        );
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let file = get_upload(&state.pg, &sb, &upload_id)
        .await
        .unwrap_or_default();
    let Some((bytes, mime, _filename)) = file else {
        return house_error(
            StatusCode::NOT_FOUND,
            "no attachment with that id, or you are not allowed to read it",
        );
    };
    if !mime.starts_with("image/") {
        // Named rather than generic: the calling model has `fetch_attachment`
        // for this and the sentence is what tells it to use that instead.
        return house_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "that attachment is {mime}, not an image — read it with fetch_attachment instead"
            ),
        );
    }

    let image = format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    let out = describe_image(&state, &image, &question).await;
    if let Some(err) = out.error {
        let mut resp = Json(json!({ "error": err })).into_response();
        *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        return resp;
    }
    Json(json!({ "description": out.text, "model": out.model })).into_response()
}
