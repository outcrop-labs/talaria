// POST /api/artifacts/{id}/pull/google. Overwrite the Talaria body from the
// Google Doc this artifact was last exported to. The editor confirms first;
// this route is the overwrite.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_auth::now_ms;
use talaria_api_facades::google::connections::get_access_token;
use talaria_api_facades::google::docs::get_doc_with_token;
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::kb::perms::{ITEM_ARTIFACT, can_read, list_editors};
use talaria_artifacts::{SaveArtifactPatch, get_artifact, guarded, save_artifact};
use talaria_error::{house_error, house_error_msg, internal};
use talaria_session::{require_user, who_of};
use talaria_state::AppState;
use talaria_teams::team_ids_for_user;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[artifacts] read failed", e)),
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => return Ok(internal("[artifacts] grants read failed", e)),
    };
    let team_ids = match team_ids_for_user(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[artifacts] team membership read failed", e)),
    };
    if !can_read(
        &guarded(&artifact),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
        &team_ids,
    ) {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let Some(file_id) = artifact.google_file_id.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(house_error_msg(
            StatusCode::CONFLICT,
            "no_google_file",
            "This file has not been exported to Google yet.",
        ));
    };
    let sb = state.secretbox().await.unwrap_or_default();
    let token = match get_access_token(&state.pg, &sb, &user.id, now_ms()).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return Ok(house_error_msg(
                StatusCode::CONFLICT,
                "not_connected",
                "Connect a Google account first (Settings → Integrations).",
            ));
        }
        Err(e) => return Ok(internal("[artifacts] google token read failed", e)),
    };
    let doc = match get_doc_with_token(&token, file_id).await {
        Ok(d) => d,
        Err(e) => return Ok(google_fail(e, "Docs")),
    };
    let actor = who_of(&user).unwrap_or_else(|| "user".into());
    if let Err(e) = save_artifact(
        &state.pg,
        &artifact.id,
        SaveArtifactPatch {
            title: Some(Some(doc.title.as_str())),
            body: Some(doc.markdown.as_str()),
            icon: None,
            storage_ref: None,
            content_type: None,
            folder_id: None,
            visibility: None,
            edit_policy: None,
        },
        actor.as_str(),
    )
    .await
    {
        return Ok(internal("[artifacts] pull save failed", e));
    }
    Ok(Json(json!({
        "title": doc.title,
        "markdown": doc.markdown,
        "url": doc.url,
    }))
    .into_response())
}
