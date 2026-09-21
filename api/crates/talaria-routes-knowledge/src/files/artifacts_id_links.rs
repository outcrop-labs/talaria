// /api/artifacts/{id}/links.
// Attach / detach an artifact to/from a target (KB doc, ticket, channel).
// The caller must be able to read the artifact — a human-only surface, so the
// gate runs before the body is even parsed: a bad body on a link you can't
// make is a 403, not a 400.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_api_facades::kb::perms::{ITEM_ARTIFACT, can_read, list_editors};
use talaria_artifacts::{attach_artifact, detach_artifact, get_artifact, guarded};
use talaria_body::{as_object, parse, string_member};
use talaria_error::{house_error, internal};
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

struct LinkBody {
    target_type: String,
    target_id: String,
}

fn parse_link_body(obj: &serde_json::Map<String, Value>) -> Result<LinkBody, String> {
    Ok(LinkBody {
        target_type: string_member(obj, "targetType", 1, 40)?,
        target_id: string_member(obj, "targetId", 1, 200)?,
    })
}

async fn gate(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
) -> Result<(talaria_session::SessionUser, talaria_artifacts::Artifact), Response> {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return Err(gate),
    };
    let artifact = match get_artifact(&state.pg, id).await {
        Ok(a) => a,
        Err(e) => return Err(internal("[artifacts] read failed", e)),
    };
    let Some(artifact) = artifact else {
        return Err(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => return Err(internal("[artifacts] grants read failed", e)),
    };
    let team_ids = match talaria_teams::team_ids_for_user(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Err(internal("[artifacts] team membership read failed", e)),
    };
    if !can_read(
        &guarded(&artifact),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
        &team_ids,
    ) {
        return Err(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok((user, artifact))
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let (user, _artifact) = match gate(&state, &headers, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match parse_link_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let actor = who_of(&user).unwrap_or_else(|| "user".into());
    if let Err(e) =
        attach_artifact(&state.pg, &id, &body.target_type, &body.target_id, &actor).await
    {
        return internal("[artifacts] link write failed", e);
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let (_user, _artifact) = match gate(&state, &headers, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match parse_link_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = detach_artifact(&state.pg, &id, &body.target_type, &body.target_id).await {
        return internal("[artifacts] link delete failed", e);
    }
    Json(json!({ "ok": true })).into_response()
}
