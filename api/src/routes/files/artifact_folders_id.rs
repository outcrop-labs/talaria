// /api/artifact-folders/{id} — port of
// ui/src/routes/api/artifact-folders.$id.ts. One artifact folder. GET → the
// folder + its grants (what the Share dialog reads). PUT → rename / icon /
// reparent / re-share. DELETE → remove (its artifacts and child folders fall
// back to the root).
//
// The folder Patch's editors differ from every other surface's: principalId
// has NO max and role has NO default — a stricter, older shape than the
// artifact/KB Editor, so it gets its own parser rather than the shared one.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::artifacts::{delete_folder, get_folder, guarded_folder, update_folder};
use crate::body::{
    as_object, enum_member, object_msg, optional_enum_member, parse,
    present_nullable_max_string_member, present_nullable_uuid_member, string_member, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{
    EditorGrant, can_edit_human, can_govern, can_read, list_editors, set_editors,
};
use crate::routes::knowledge::kb_spaces_id::editors_json;
use crate::session::{require_perm, require_user, who_of};
use crate::state::AppState;

const ITEM_FOLDER: &str = "artifact-folder";

struct Patch {
    name: Option<String>,
    icon: Option<Option<String>>,
    parent_id: Option<Option<String>>,
    visibility: Option<String>,
    edit_policy: Option<String>,
    editors: Option<Vec<EditorGrant>>,
}

fn parse_patch(obj: &serde_json::Map<String, Value>) -> Result<Patch, String> {
    Ok(Patch {
        name: match obj.get("name") {
            None => None,
            Some(_) => Some(string_member(obj, "name", 1, 80)?),
        },
        icon: present_nullable_max_string_member(obj, "icon", 16)?,
        parent_id: present_nullable_uuid_member(obj, "parentId")?,
        visibility: optional_enum_member(obj, "visibility", &["private", "org", "public"])?,
        edit_policy: optional_enum_member(obj, "editPolicy", &["owner", "org", "restricted"])?,
        editors: parse_folder_editors(obj.get("editors"))?,
    })
}

/// The folder Patch's Editor: enum principalType, min-1 principalId, role an
/// enum with no default (absent role is a 400 here, unlike the KB surface).
fn parse_folder_editors(v: Option<&Value>) -> Result<Option<Vec<EditorGrant>>, String> {
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
        out.push(EditorGrant {
            principal_type: enum_member(inner, "principalType", &["user", "agent"])?,
            principal_id: string_member(inner, "principalId", 1, usize::MAX)?,
            role: enum_member(inner, "role", &["viewer", "editor"])?,
        });
    }
    Ok(Some(out))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let folder = match get_folder(&state.pg, &id).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[folders] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(folder) = folder else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let editors = match list_editors(&state.pg, ITEM_FOLDER, &folder.id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[folders] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    if !can_read(
        &guarded_folder(&folder),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    // `editors` is the key the Share dialog reads; it seeds an EDITABLE list
    // from it and PUTs that list back wholesale, so the shape is a contract.
    Json(json!({ "folder": folder, "editors": editors_json(&editors) })).into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_perm(&state, &headers, "artifacts.create").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match parse_patch(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let folder = match get_folder(&state.pg, &id).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[folders] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(folder) = folder else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let editors = match list_editors(&state.pg, ITEM_FOLDER, &folder.id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[folders] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    let g = guarded_folder(&folder);
    if !can_edit_human(&g, Some(&user.id), who_of(&user).as_deref(), &editors) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }

    let sharing = body.visibility.is_some() || body.edit_policy.is_some() || body.editors.is_some();
    if sharing {
        // Same governance rule as artifacts and KB docs: the owner, or an
        // admin (and agent-delegates) for an ownerless workspace folder.
        let who = who_of(&user);
        let governor = match can_govern(&state.pg, &g, &user.id, &user.role, who.as_deref()).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[folders] govern check failed: {e}");
                return thrown_internal_error();
            }
        };
        if !governor {
            return house_error(StatusCode::FORBIDDEN, "not allowed to change sharing");
        }
        if body.visibility.as_deref() == Some("public")
            && !matches!(
                crate::permissions::has_perm(&state.pg, &user.id, &user.role, "artifacts.publish")
                    .await,
                Ok(true)
            )
        {
            return house_error(StatusCode::FORBIDDEN, "no permission to publish to the web");
        }
        if let Some(editors) = &body.editors
            && set_editors(&state.pg, ITEM_FOLDER, &id, editors)
                .await
                .is_err()
        {
            return thrown_internal_error();
        }
    }
    let updated = match update_folder(
        &state.pg,
        &id,
        body.name.as_deref(),
        body.icon.as_ref().map(|o| o.as_deref()),
        body.parent_id.as_ref().map(|o| o.as_deref()),
        body.visibility.as_deref(),
        body.edit_policy.as_deref(),
    )
    .await
    {
        Ok(Some(f)) => f,
        Ok(None) => return house_error(StatusCode::BAD_REQUEST, "invalid"),
        Err(e) => {
            tracing::error!("[folders] update failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "folder": updated })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_perm(&state, &headers, "artifacts.create").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let folder = match get_folder(&state.pg, &id).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[folders] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(folder) = folder else {
        return Json(json!({ "ok": true })).into_response();
    };
    // Deleting a folder scatters everything inside it to the root — a bigger
    // act than an edit, so it takes the same rights as re-sharing rather than
    // letting any org-policy editor dissolve someone else's shared folder.
    let who = who_of(&user);
    let governor = match can_govern(
        &state.pg,
        &guarded_folder(&folder),
        &user.id,
        &user.role,
        who.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[folders] govern check failed: {e}");
            return thrown_internal_error();
        }
    };
    if !governor {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    if let Err(e) = delete_folder(&state.pg, &id).await {
        tracing::error!("[folders] delete failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}
