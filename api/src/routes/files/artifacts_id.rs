// /api/artifacts/{id}. One artifact: read/edit gated by its audience,
// sharing owner-only, agents (by key) only edit content when granted the
// Editor role — a personal assistant READS its owner's artifacts the way it
// reads their docs (can_read_agent's owner arm), and edit stays grant-only.
// The PUT is the plane's whole state machine — content edits, sharing,
// official curation and brain routing all land here, in this order.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::artifacts::{
    SaveArtifactPatch, delete_artifact, get_artifact, guarded, index_plan_doc, save_artifact,
    set_artifact_official, set_artifact_routing, targets_for_artifact,
};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, optional_boolean_member, optional_enum_member, optional_max_string_member, parse,
    present_nullable_max_string_member, present_nullable_uuid_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{
    ITEM_ARTIFACT, can_edit_agent, can_edit_human, can_govern, can_read, can_read_agent,
    list_editors, set_editors,
};
use crate::retrieval::artifact_routing::apply_artifact_routing;
use crate::retrieval::{embed, qdrant};
use crate::routes::knowledge::kb_spaces_id::{editors_json, parse_editors};
use crate::session::{actor_of, require_user, who_of};
use crate::state::AppState;
use crate::users::is_elevated_assistant;

struct PutBody {
    title: Option<String>,
    body: Option<String>,
    icon: Option<Option<String>>,
    storage_ref: Option<Option<String>>,
    content_type: Option<Option<String>>,
    folder_id: Option<Option<String>>,
    visibility: Option<String>,
    edit_policy: Option<String>,
    editors: Option<Vec<crate::kb::perms::EditorGrant>>,
    official: Option<bool>,
    rag_routing: Option<String>,
}

fn parse_put_body(obj: &serde_json::Map<String, Value>) -> Result<PutBody, String> {
    Ok(PutBody {
        title: optional_max_string_member(obj, "title", 200)?,
        body: optional_max_string_member(obj, "body", 2_000_000)?,
        icon: present_nullable_max_string_member(obj, "icon", 16)?,
        storage_ref: present_nullable_uuid_member(obj, "storageRef")?,
        content_type: present_nullable_max_string_member(obj, "contentType", 200)?,
        folder_id: present_nullable_uuid_member(obj, "folderId")?,
        visibility: optional_enum_member(obj, "visibility", &["private", "org", "public"])?,
        edit_policy: optional_enum_member(obj, "editPolicy", &["owner", "org", "restricted"])?,
        editors: parse_editors(obj.get("editors"))?,
        official: optional_boolean_member(obj, "official")?,
        rag_routing: optional_max_string_member(obj, "ragRouting", 60)?,
    })
}

fn not_found() -> Response {
    house_error(StatusCode::NOT_FOUND, "not found")
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(artifact) = artifact else {
        return not_found();
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[artifacts] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    // Agents (over MCP) read org/public artifacts, ones granted to them, and —
    // for a personal assistant — its owner's own (can_read_agent's owner arm).
    let reader = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Some(reader) = reader {
        let owner = match crate::users::assistant_owner_for(
            &state.pg,
            &AgentSubject::Caller(reader.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[artifacts] owner resolve failed: {e}");
                return thrown_internal_error();
            }
        };
        if !can_read_agent(
            &guarded(&artifact),
            &reader.model,
            owner.as_deref(),
            &editors,
        ) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        return Json(json!({ "artifact": artifact, "editors": editors_json(&editors) }))
            .into_response();
    }
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !can_read(
        &guarded(&artifact),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    Json(json!({ "artifact": artifact, "editors": editors_json(&editors) })).into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(artifact) = artifact else {
        return not_found();
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let mut body = match parse_put_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[artifacts] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    let g = guarded(&artifact);

    let actor: String;
    let mut owner = false;
    let agent = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Some(agent) = agent {
        let name = agent.model.clone();
        // Editor grant — or an admin-elevated assistant on any non-private artifact.
        let elevated = artifact.visibility != "private"
            && match is_elevated_assistant(&state.pg, &AgentSubject::Caller(agent)).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[artifacts] elevation read failed: {e}");
                    return thrown_internal_error();
                }
            };
        let may_edit = can_edit_agent(&name, &editors) || elevated;
        if !may_edit {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        actor = name;
        body.visibility = None;
        body.edit_policy = None;
        body.editors = None;
        body.official = None;
        body.rag_routing = None;
    } else {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        let who = who_of(&user);
        if !can_edit_human(&g, Some(&user.id), who.as_deref(), &editors) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        actor = actor_of(&user);
        // `canGovern`, not `isOwner` — the same rule kb/docs/{id} already
        // uses, and the reason canGovern exists. An org agent's artifact is
        // OWNERLESS on purpose, so strict ownership left every workspace file
        // an orphan whose sharing literally nobody could change: the owner
        // check could never pass, and the surface offered a Share dialog that
        // always 403'd. canGovern hands those to admins and to whoever may
        // use the agent that wrote them, while human-owned artifacts stay
        // owner-only exactly as before.
        owner = match can_govern(&state.pg, &g, &user.id, &user.role, who.as_deref()).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[artifacts] govern check failed: {e}");
                return thrown_internal_error();
            }
        };
        if body.visibility.as_deref() == Some("public")
            && !matches!(
                crate::permissions::has_perm(&state.pg, &user.id, &user.role, "artifacts.publish")
                    .await,
                Ok(true)
            )
        {
            return house_error(StatusCode::FORBIDDEN, "no permission to publish to the web");
        }
        let sharing =
            body.visibility.is_some() || body.edit_policy.is_some() || body.editors.is_some();
        if !owner && sharing {
            return house_error(StatusCode::FORBIDDEN, "not allowed to change sharing");
        }
        // Routing decides which brain retrieves the content — owner's call.
        if !owner && body.rag_routing.is_some() {
            return house_error(
                StatusCode::FORBIDDEN,
                "only the owner can change brain routing",
            );
        }
    }

    if !owner {
        body.official = None;
    }
    if owner
        && let Some(editors) = &body.editors
        && set_editors(&state.pg, ITEM_ARTIFACT, &id, editors)
            .await
            .is_err()
    {
        return thrown_internal_error();
    }
    let mut updated = match save_artifact(
        &state.pg,
        &id,
        SaveArtifactPatch {
            // The Patch's title is string-optional, never nullish — absent
            // means "don't touch", never "clear" (the column is not-null).
            title: body.title.as_deref().map(Some),
            body: body.body.as_deref(),
            icon: body.icon.as_ref().map(|o| o.as_deref()),
            storage_ref: body.storage_ref.as_ref().map(|o| o.as_deref()),
            content_type: body.content_type.as_ref().map(|o| o.as_deref()),
            folder_id: body.folder_id.as_ref().map(|o| o.as_deref()),
            visibility: body.visibility.as_deref(),
            edit_policy: body.edit_policy.as_deref(),
        },
        &actor,
    )
    .await
    {
        Ok(Some(a)) => a,
        Ok(None) => return not_found(),
        Err(e) => {
            tracing::error!("[artifacts] save failed: {e}");
            return thrown_internal_error();
        }
    };
    if let Some(official) = body.official
        && official != updated.official
    {
        updated = match set_artifact_official(
            &state.pg,
            &qdrant::real_deps(),
            &embed::real_deps(),
            &id,
            official,
            &actor,
        )
        .await
        {
            Ok(Some(a)) => a,
            Ok(None) => updated,
            Err(e) => {
                tracing::error!("[artifacts] officialize failed: {e}");
                return thrown_internal_error();
            }
        };
        let (pg, actor_, action, target_id, target_label) = (
            state.pg.clone(),
            actor.clone(),
            if official {
                "artifact.officialize"
            } else {
                "artifact.deofficialize"
            },
            id.clone(),
            updated.title.clone(),
        );
        tokio::spawn(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &actor_,
                    action,
                    target_type: "artifact",
                    target_id: Some(&target_id),
                    target_label: Some(&target_label),
                    before: None,
                    after: None,
                },
            )
            .await;
        });
    }
    // Routing change → re-place immediately (and validate the brain).
    if let Some(routing) = body.rag_routing.clone() {
        match set_artifact_routing(&state.pg, &id, &routing, &actor).await {
            Ok(Some(routed)) => {
                updated = routed.clone();
                let (pg, qd, ed, routed) = (
                    state.pg.clone(),
                    qdrant::real_deps(),
                    embed::real_deps(),
                    routed,
                );
                tokio::spawn(async move {
                    apply_artifact_routing(&pg, &qd, &ed, &routed).await;
                });
            }
            Ok(None) => {}
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        }
    }
    // Content edits keep the artifact's retrievable copy current: auto →
    // the plan-doc activity flow; explicit brain → re-index there.
    if body.body.is_some() || body.title.is_some() {
        let u = updated.clone();
        if !u.rag_routing.is_empty() && u.rag_routing != "auto" {
            let (pg, qd, ed) = (state.pg.clone(), qdrant::real_deps(), embed::real_deps());
            tokio::spawn(async move {
                apply_artifact_routing(&pg, &qd, &ed, &u).await;
            });
        } else {
            let (pg, qd, ed, id) = (
                state.pg.clone(),
                qdrant::real_deps(),
                embed::real_deps(),
                id.clone(),
            );
            tokio::spawn(async move {
                // The find-plan → index chain is detached; its errors are
                // swallowed.
                if let Ok(targets) = targets_for_artifact(&pg, &id).await
                    && let Some((_, plan_id)) = targets.iter().find(|(tt, _)| tt == "plan")
                {
                    let _ = index_plan_doc(&pg, &qd, &ed, &u, plan_id).await;
                }
            });
        }
    }
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[artifacts] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "artifact": updated, "editors": editors_json(&editors) })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(artifact) = artifact else {
        return not_found();
    };
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[artifacts] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    if !can_edit_human(
        &guarded(&artifact),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    if let Err(e) = delete_artifact(&state.pg, &qdrant::real_deps(), &embed::real_deps(), &id).await
    {
        tracing::error!("[artifacts] delete failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}
