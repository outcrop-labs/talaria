// Agent Drive management. Creating a folder is immediate — cheap and
// reversible. Moving or renaming mutates the human's Drive, so those queue.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::{resolve_agent_google, resolve_agent_principal};
use talaria_api_facades::google::drive::create_drive_folder_with_token;
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::pending_actions::{QueueAction, queue_action};
use talaria_body::{optional_max_string_member, parse, string_member};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_realtime_watch::RealtimeDeps;
use talaria_state::AppState;

pub async fn post_folder(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Drive") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match string_member(obj, "name", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let parent = match optional_max_string_member(obj, "parentId", 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(denied) =
        super::integrations_google_agent_queue::refuse_unresolved_write(&state.pg, &agent_model)
            .await
            .map_err(|e| internal("[integrations/google/agent/drive] principal read failed", e))?
    {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            "No Google account is connected for this agent (its owner, or the org account).",
        ));
    };
    match create_drive_folder_with_token(
        &google.token,
        &name,
        parent.as_deref().filter(|s| !s.is_empty()),
    )
    .await
    {
        Ok(folder) => Ok(Json(json!({
            "id": folder.id,
            "name": folder.name,
            "url": folder.web_view_link,
            "message": "Created the folder. It can be moved or renamed later, with approval.",
        }))
        .into_response()),
        Err(e) => Ok(google_fail(e, "Drive")),
    }
}

async fn queue_drive(
    state: &AppState,
    headers: HeaderMap,
    body: Bytes,
    kind: &'static str,
    summary_of: impl Fn(&str) -> String,
    extra: impl Fn(
        &serde_json::Map<String, Value>,
        &mut serde_json::Map<String, Value>,
    ) -> Result<(), String>,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Drive") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let file_id = match string_member(obj, "fileId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mut payload = serde_json::Map::new();
    payload.insert("fileId".into(), json!(file_id));
    if let Err(msg) = extra(obj, &mut payload) {
        return Ok(house_error(StatusCode::BAD_REQUEST, &msg));
    }
    if let Some(denied) =
        super::integrations_google_agent_queue::refuse_unresolved_write(&state.pg, &agent_model)
            .await
            .map_err(|e| internal("[integrations/google/agent/drive] principal read failed", e))?
    {
        return Ok(denied);
    }
    let principal = match resolve_agent_principal(&state.pg, &agent_model).await {
        Ok(p) => p,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/drive] principal read failed",
                e,
            ));
        }
    };
    let summary = summary_of(&file_id);
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    let queued = match queue_action(
        &state.pg,
        realtime,
        &QueueAction {
            kind,
            summary: &summary,
            payload: &Value::Object(payload),
            agent_model: &agent_model,
            owner_user_id: principal.owner_user_id.as_deref(),
            is_org: principal.is_org,
            principal_kind: principal.kind.as_str(),
        },
    )
    .await
    {
        Ok(q) => q,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/drive] queue failed",
                e,
            ));
        }
    };
    Ok(super::integrations_google_agent_queue::answer_queued(
        &state.pg,
        queued,
        "An identical Drive change is already waiting — nothing new queued.",
        "Queued — waiting for the owner to approve before Drive changes.",
        "Queued — waiting for an admin to approve before Drive changes.",
    )
    .await)
}

pub async fn post_move(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    queue_drive(
        &state,
        headers,
        body,
        "drive_move",
        |id| format!("Move Google Drive file {id}"),
        |obj, payload| {
            let parent = string_member(obj, "parentId", 1, 200)?;
            payload.insert("parentId".into(), json!(parent));
            Ok(())
        },
    )
    .await
}

pub async fn post_rename(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    queue_drive(
        &state,
        headers,
        body,
        "drive_rename",
        |id| format!("Rename Google Drive file {id}"),
        |obj, payload| {
            let name = string_member(obj, "name", 1, 200)?;
            payload.insert("name".into(), json!(name));
            Ok(())
        },
    )
    .await
}
