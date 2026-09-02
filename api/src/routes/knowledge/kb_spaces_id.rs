// /api/kb/spaces/{id} — port of ui/src/routes/api/kb.spaces.$id.ts. One KB
// folder. Same permission model as docs: read gated by visibility, writes by
// the edit policy + editor grants, sharing owner-only (canGovern).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    array_too_big_msg, as_object, enum_member, object_msg, optional_enum_member, parse,
    present_nullable_max_string_member, string_member, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{
    EditorGrant, ITEM_SPACE, can_edit_human, can_govern, can_read, list_editors, set_editors,
};
use crate::kb::{SpacePatch, delete_space, get_space, update_space};
use crate::retrieval::{embed, qdrant};
use crate::session::{actor_of, require_user, who_of};
use crate::state::AppState;

use super::kb_spaces::guarded_of;

/// The grant list as TS's listEditors aliases it — principalType, principalId,
/// role, in that order.
pub(crate) fn editors_json(grants: &[EditorGrant]) -> Vec<Value> {
    grants
        .iter()
        .map(|g| {
            json!({
                "principalType": g.principal_type,
                "principalId": g.principal_id,
                "role": g.role,
            })
        })
        .collect()
}

/// z.array(Editor).max(200) — elements validate before the array-length check
/// (zod 4's issue order, the same probed behavior the rag bindings are pinned
/// on). The Editor object itself: enum principalType, min-1/max-200
/// principalId, role enum defaulting 'viewer'.
pub(crate) fn parse_editors(v: Option<&Value>) -> Result<Option<Vec<EditorGrant>>, String> {
    let Some(v) = v else {
        return Ok(None); // absent — what `.optional()` admits
    };
    let arr = v.as_array().ok_or_else(|| {
        format!(
            "Invalid input: expected array, received {}",
            zod_type_name(v)
        )
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let inner = el
            .as_object()
            .ok_or_else(|| object_msg(zod_type_name(el)))?;
        let principal_type = enum_member(inner, "principalType", &["user", "agent"])?;
        let principal_id = string_member(inner, "principalId", 1, 200)?;
        let role = match inner.get("role") {
            None | Some(Value::Null) => "viewer".to_string(), // .default('viewer')
            Some(_) => enum_member(inner, "role", &["viewer", "editor"])?,
        };
        out.push(EditorGrant {
            principal_type,
            principal_id,
            role,
        });
    }
    if arr.len() > 200 {
        return Err(array_too_big_msg(200));
    }
    Ok(Some(out))
}

/// The Patch body → the tri-state engine patch. Field-by-field, each with its
/// own zod shape (name min-1/max-80, description/icon nullish, body max
/// 500k, visibility/editPolicy enums, editors the shared parser).
fn parse_patch(obj: &serde_json::Map<String, Value>) -> Result<SpacePatch, String> {
    Ok(SpacePatch {
        name: match obj.get("name") {
            None => None,
            Some(_) => Some(string_member(obj, "name", 1, 80)?),
        },
        description: present_nullable_max_string_member(obj, "description", 400)?,
        icon: present_nullable_max_string_member(obj, "icon", 16)?,
        body: crate::body::optional_max_string_member(obj, "body", 500_000)?,
        visibility: optional_enum_member(obj, "visibility", &["private", "org", "public"])?,
        edit_policy: optional_enum_member(obj, "editPolicy", &["owner", "org", "restricted"])?,
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let space = match get_space(&state.pg, &id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[kb] space read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(space) = space else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let editors = match list_editors(&state.pg, ITEM_SPACE, &space.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] editor read failed: {e}");
            return thrown_internal_error();
        }
    };
    let who = who_of(&user);
    if !can_read(
        &guarded_of(&space),
        Some(&user.id),
        who.as_deref(),
        &editors,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    Json(json!({ "space": space, "editors": editors_json(&editors) })).into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let space = match get_space(&state.pg, &id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[kb] space read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(space) = space else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let patch = match parse_patch(obj) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let editors_req = match parse_editors(obj.get("editors")) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let editors = match list_editors(&state.pg, ITEM_SPACE, &space.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] editor read failed: {e}");
            return thrown_internal_error();
        }
    };
    let who = who_of(&user);
    if !can_edit_human(
        &guarded_of(&space),
        Some(&user.id),
        who.as_deref(),
        &editors,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let owner = match can_govern(
        &state.pg,
        &guarded_of(&space),
        &user.id,
        &user.role,
        who.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] govern check failed: {e}");
            return thrown_internal_error();
        }
    };
    let sharing_touched =
        patch.visibility.is_some() || patch.edit_policy.is_some() || editors_req.is_some();
    if !owner && sharing_touched {
        return house_error(StatusCode::FORBIDDEN, "only the owner can change sharing");
    }
    if owner
        && let Some(grants) = &editors_req
        && set_editors(&state.pg, ITEM_SPACE, &id, grants)
            .await
            .is_err()
    {
        return thrown_internal_error();
    }
    let actor = who_of(&user).unwrap_or_else(|| "user".into());
    let updated = match update_space(&state.pg, &id, &patch, Some(&actor)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[kb] space update failed: {e}");
            return thrown_internal_error();
        }
    };
    let editors_after = match list_editors(&state.pg, ITEM_SPACE, &id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] editor read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "space": updated, "editors": editors_json(&editors_after) })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let space = match get_space(&state.pg, &id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[kb] space read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(space) = space else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let editors = match list_editors(&state.pg, ITEM_SPACE, &space.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] editor read failed: {e}");
            return thrown_internal_error();
        }
    };
    let who = who_of(&user);
    if !can_edit_human(
        &guarded_of(&space),
        Some(&user.id),
        who.as_deref(),
        &editors,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    if delete_space(&state.pg, &qd, &ed, &id).await.is_err() {
        return thrown_internal_error();
    }
    let (pg, actor) = (state.pg.clone(), actor_of(&user));
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "kb.space.delete",
                target_type: "kb-space",
                target_id: Some(&id),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "ok": true })).into_response()
}
