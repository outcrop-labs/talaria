// POST /api/integrations/google/agent/drive/import — pull a Drive file into
// a Talaria artifact, using the Google account the agent acts for. Immediate:
// it creates a new artifact and does not change the Drive file.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::resolve_agent_google;
use talaria_api_facades::google::drive::import_drive_file_with_token;
use talaria_api_facades::google::errors::google_fail;
use talaria_artifacts::{SaveArtifactPatch, create_artifact, record_google_export, save_artifact};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Drive access") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let file_id = match string_member(obj, "fileId", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
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
    let content = match import_drive_file_with_token(
        &state.pg,
        &sb,
        &google.token,
        google.owner_user_id.as_deref(),
        &file_id,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => return Ok(google_fail(e, "Drive")),
    };
    let actor = format!("agent:{agent_model}");
    let artifact = match create_artifact(
        &state.pg,
        Some(&content.kind),
        Some(&content.title),
        &actor,
        google.owner_user_id.as_deref(),
        None,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/drive] artifact create failed",
                e,
            ));
        }
    };
    if let Err(e) = save_artifact(
        &state.pg,
        &artifact.id,
        SaveArtifactPatch {
            title: Some(Some(content.title.as_str())),
            body: Some(content.body.as_str()),
            icon: None,
            storage_ref: Some(content.storage_ref.as_deref()),
            content_type: Some(content.content_type.as_deref()),
            folder_id: None,
            visibility: None,
            edit_policy: None,
        },
        &actor,
    )
    .await
    {
        return Ok(internal(
            "[integrations/google/agent/drive] artifact save failed",
            e,
        ));
    }
    if let Some(source_url) = &content.source_url
        && let Err(e) = record_google_export(&state.pg, &artifact.id, &file_id, source_url).await
    {
        return Ok(internal(
            "[integrations/google/agent/drive] source stamp failed",
            e,
        ));
    }
    let mut wire = serde_json::to_value(&artifact).unwrap_or(Value::Null);
    if let Some(obj) = wire.as_object_mut() {
        obj.insert("kind".into(), json!(content.kind));
        obj.insert("title".into(), json!(content.title));
        obj.insert("url".into(), json!(content.source_url));
    }
    Ok(Json(json!({
        "artifact": wire,
        "message": "Imported. The Drive file was not changed. The link is the one Google returned.",
    }))
    .into_response())
}
