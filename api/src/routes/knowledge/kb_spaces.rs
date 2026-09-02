// /api/kb/spaces. KB spaces (any member). GET → all the caller can read
// (agents over MCP see org/public + granted; humans see visibility-read +
// granted). POST → create (agents find-or-create by name; humans need
// kb.official).

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::agent_caller;
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, optional_max_string_member, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{can_read, can_read_agent, granted_item_ids, granted_item_ids_for_agent};
use crate::kb::{NewSpace, create_space, list_spaces};
use crate::session::{actor_of, require_perm, require_user, who_of};
use crate::state::AppState;

/// The Guarded view of a space row — what every kb_perms predicate takes.
pub(crate) fn guarded_of(space: &crate::kb::KbSpace) -> crate::kb::perms::Guarded {
    crate::kb::perms::Guarded {
        owner_user_id: space.owner_user_id.clone(),
        created_by: space.created_by.clone(),
        visibility: space.visibility.clone(),
        edit_policy: space.edit_policy.clone(),
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Agents (over MCP) see org/public spaces + ones granted to them.
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let all = match list_spaces(&state.pg).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] space list failed: {e}");
            return thrown_internal_error();
        }
    };
    if let Some(caller) = caller {
        let granted = match granted_item_ids_for_agent(&state.pg, "space", &caller.model).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[kb] grant read failed: {e}");
                return thrown_internal_error();
            }
        };
        let spaces: Vec<_> = all
            .into_iter()
            .filter(|s| {
                granted.contains(&s.id) || can_read_agent(&guarded_of(s), &caller.model, &[])
            })
            .collect();
        return Json(json!({ "spaces": spaces })).into_response();
    }
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // Hide folders the caller can't read, but keep ones shared with them.
    // can_read runs with no grant list — it covers only the visibility half;
    // the granted-set check beside it covers the grant half.
    let granted = match granted_item_ids(&state.pg, "space", &user.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] grant read failed: {e}");
            return thrown_internal_error();
        }
    };
    let who = who_of(&user);
    let spaces: Vec<_> = all
        .into_iter()
        .filter(|s| {
            granted.contains(&s.id) || can_read(&guarded_of(s), Some(&user.id), who.as_deref(), &[])
        })
        .collect();
    Json(json!({ "spaces": spaces })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let description = match optional_max_string_member(obj, "description", 400) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let icon = match optional_max_string_member(obj, "icon", 8) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Agents (over MCP) may create spaces too — a space is just a shelf, and
    // docs stay drafts until a human officializes them. No owner, so
    // sharing/deletion stay human calls.
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Some(caller) = caller {
        let model = caller.model.clone();
        let dup = match list_spaces(&state.pg).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[kb] space list failed: {e}");
                return thrown_internal_error();
            }
        }
        .into_iter()
        .find(|s| s.name.trim().to_lowercase() == name.trim().to_lowercase());
        if let Some(dup) = dup {
            // find-or-create: agents retry; duplicates rot the KB.
            return Json(json!({ "space": dup })).into_response();
        }
        let space = match create_space(
            &state.pg,
            &NewSpace {
                name,
                description,
                icon,
                created_by: model,
                owner_user_id: None,
            },
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[kb] space create failed: {e}");
                return thrown_internal_error();
            }
        };
        return Json(json!({ "space": space })).into_response();
    }
    let user = match require_perm(&state, &headers, "kb.official").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let created_by = who_of(&user).unwrap_or_else(|| "user".to_string());
    let space = match create_space(
        &state.pg,
        &NewSpace {
            name,
            description,
            icon,
            owner_user_id: Some(user.id.clone()),
            created_by,
        },
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[kb] space create failed: {e}");
            return thrown_internal_error();
        }
    };
    let (pg, actor, target_id, target_label) = (
        state.pg.clone(),
        actor_of(&user),
        space.id.clone(),
        space.name.clone(),
    );
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "kb.space_create",
                target_type: "kb-space",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "space": space })).into_response()
}
