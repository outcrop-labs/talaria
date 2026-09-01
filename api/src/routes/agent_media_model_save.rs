// /api/agent-media/{model}/save — port of ui/src/routes/api/agent-media.$model.save.ts.
// POST { path, title?, folderId? | folder? } → copy an image out of the
// agent's container into a durable FILE artifact (uploads-backed), optionally
// straight into a folder. For science. And company meme folders. Callable by
// humans (session; any agent they may use) AND by the agent itself over the
// talaria MCP (agent key; its OWN container only). Same path/type guardrails
// as viewing the image inline.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::agent_media::read_agent_image;
use crate::artifacts::{
    SaveArtifactPatch, agent_category_folder, create_artifact, create_folder, list_folders,
    save_artifact,
};
use crate::body::{
    as_object, optional_uuid_member, parse, string_member, string_msg, too_big_msg, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::{describe_agent, usable_agent_gate};
use crate::session::require_user;
use crate::state::AppState;
use crate::uploads::save_upload;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// `z.string().trim().max(N).optional()` — trim runs before the bound, and
/// there is no minimum: a title of only whitespace clears (as the empty
/// string, which the fallback below replaces with the filename).
fn trimmed_optional(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            let trimmed = s.trim();
            if trimmed.chars().count() > max {
                return Err(too_big_msg(max));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let actor: String;
    let owner_user_id: Option<String>;
    let mut agent_actor = false;
    match agent_caller(&state.pg, &headers).await {
        Err(resp) => return resp,
        Ok(Some(caller)) => {
            agent_actor = true;
            if caller.model != model {
                return house_error(
                    StatusCode::FORBIDDEN,
                    "agents can only save from their own workspace",
                );
            }
            actor = caller.model.clone();
            // A personal assistant saves media FOR ITS OWNER — owned +
            // private. Asked with the CALLER: writing into a human's account
            // needs a proven identity, not an asserted one.
            owner_user_id =
                match crate::users::assistant_owner_for(&state.pg, &AgentSubject::Caller(caller))
                    .await
                {
                    Ok(o) => o,
                    Err(e) => {
                        tracing::error!("[agent-media] owner lookup failed: {e}");
                        return thrown_internal_error();
                    }
                };
        }
        Ok(None) => {
            let user = match require_user(&state, &headers).await {
                Ok(u) => u,
                Err(gate) => return gate,
            };
            let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
                Ok(g) => g,
                Err(e) => {
                    tracing::error!("[agent-media] gate read failed: {e}");
                    return thrown_internal_error();
                }
            };
            if !gate(&model) {
                return house_error(StatusCode::FORBIDDEN, "forbidden");
            }
            actor = user
                .email
                .clone()
                .or(user.name.clone())
                .unwrap_or("user".into());
            owner_user_id = Some(user.id);
        }
    }

    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let path = match string_member(obj, "path", 1, 1000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let title = match trimmed_optional(obj, "title", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let mut folder_id = match optional_uuid_member(obj, "folderId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let folder = match trimmed_optional(obj, "folder", 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let media = match read_agent_image(&state.pg, &model, &path).await {
        Ok(m) => m,
        Err(media) => {
            let status =
                StatusCode::from_u16(media.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return house_error(status, media.error);
        }
    };

    // Folder by name: find-or-create (case-insensitive) — "Memes" just works.
    // With no folder given, media files under the agent's own cabinet.
    if folder_id.is_none()
        && let Some(want) = folder.as_deref().filter(|f| !f.is_empty())
    {
        let existing = match list_folders(&state.pg).await {
            Ok(folders) => folders
                .into_iter()
                .find(|f| f.name.to_lowercase() == want.to_lowercase()),
            Err(e) => {
                tracing::error!("[agent-media] folder list failed: {e}");
                return thrown_internal_error();
            }
        };
        folder_id = match existing {
            Some(f) => Some(f.id),
            None => match create_folder(&state.pg, want, None, &actor, None, Some("org")).await {
                Ok(f) => Some(f.id),
                Err(e) => {
                    tracing::error!("[agent-media] folder create failed: {e}");
                    return thrown_internal_error();
                }
            },
        };
    }
    if folder_id.is_none() {
        folder_id =
            agent_category_folder(&state.pg, &describe_agent(&model).label, "Media", &actor).await;
    }

    let filename = path.rsplit('/').next().unwrap_or("image");
    let sb = state.secretbox().await.unwrap_or_default();
    let upload = match save_upload(
        &state.pg,
        &sb,
        filename,
        media.mime,
        &media.bytes,
        owner_user_id.as_deref(),
    )
    .await
    {
        Ok(u) => u,
        // No catch in the TS — the route lets the throw carry to the
        // framework's own 500.
        Err(e) => {
            tracing::error!("[agent-media] upload save failed: {e}");
            return thrown_internal_error();
        }
    };
    let title = match title.filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => filename.to_string(),
    };
    let created = match create_artifact(
        &state.pg,
        Some("file"),
        Some(&title),
        &actor,
        owner_user_id.as_deref(),
        None,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[agent-media] artifact create failed: {e}");
            return thrown_internal_error();
        }
    };
    // ORG-agent media is for the TEAM (a private no-owner artifact would be
    // invisible to humans). A personal assistant's media belongs to its
    // owner — private, shareable by the human.
    let visibility = (agent_actor && owner_user_id.is_none()).then_some("org");
    let saved = match save_artifact(
        &state.pg,
        &created.id,
        SaveArtifactPatch {
            title: None,
            body: None,
            icon: None,
            storage_ref: Some(Some(&upload.id)),
            content_type: Some(Some(media.mime)),
            folder_id: Some(folder_id.as_deref()),
            visibility,
            edit_policy: None,
        },
        &actor,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[agent-media] artifact save failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "artifact": saved.unwrap_or(created) })).into_response()
}
