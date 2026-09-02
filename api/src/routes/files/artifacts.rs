// /api/artifacts. The artifact LIST
// (what the Files browser opens on) and CREATE. Read is gated exactly like
// the KB list beside it: org/public visible to all, private only to owner,
// grants — and, for a personal assistant, its owner's own reach (the single-
// artifact GET has always had that arm; the LIST now mirrors it). Creation
// differs by caller: a PERSONAL assistant's output belongs to its owner and
// starts private-to-them; a general agent's output is workspace material and
// starts org-visible — stamped to the human the agent answers to (the
// attribution ladder), so a person governs it instead of an allow-list.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::artifacts::{
    AGENT_CATEGORIES, Artifact, SaveArtifactPatch, agent_category_folder, create_artifact,
    list_artifacts, named_root_folder, save_artifact,
};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, optional_enum_member, optional_max_string_member, parse, too_big_msg, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::describe_agent;
use crate::kb::perms::{
    EditorGrant, ITEM_ARTIFACT, can_read, can_read_agent, granted_item_ids,
    granted_item_ids_for_agent, set_editors,
};
use crate::session::{actor_of, require_perm, require_user, who_of};
use crate::state::AppState;

struct Body {
    kind: Option<String>,
    title: Option<String>,
    body: Option<String>,
    folder: Option<String>,
    visibility: Option<String>,
}

fn parse_body(obj: &serde_json::Map<String, Value>) -> Result<Body, String> {
    Ok(Body {
        kind: optional_enum_member(obj, "kind", &["doc", "sheet", "microsite", "file"])?,
        title: optional_max_string_member(obj, "title", 200)?,
        body: optional_max_string_member(obj, "body", 2_000_000)?,
        // folder: trimmed, max 120, optional — the only trimmed-OPTIONAL
        // member on this surface, so it reads inline rather than growing a
        // body.rs helper for one caller.
        folder: match obj.get("folder") {
            None => None,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| {
                        format!(
                            "Invalid input: expected string, received {}",
                            zod_type_name(v)
                        )
                    })?
                    .trim()
                    .to_string();
                if crate::body::utf16_len(&s) > 120 {
                    return Err(too_big_msg(120));
                }
                Some(s)
            }
        },
        visibility: optional_enum_member(obj, "visibility", &["private", "org", "public"])?,
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Some(caller) = caller {
        let name = caller.model.clone();
        // The LIST must answer the same question the single-artifact GET
        // answers: a personal assistant reads its owner's private artifacts
        // too. Without this arm the two planes disagreed — the GET opened
        // what the browser's own listing never showed.
        let owner =
            match crate::users::assistant_owner_for(&state.pg, &AgentSubject::Caller(caller)).await
            {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!("[artifacts] owner lookup failed: {e}");
                    return thrown_internal_error();
                }
            };
        let granted = match granted_item_ids_for_agent(&state.pg, ITEM_ARTIFACT, &name).await {
            Ok(g) => g,
            Err(e) => {
                tracing::error!("[artifacts] grants read failed: {e}");
                return thrown_internal_error();
            }
        };
        let artifacts = match list_artifacts(&state.pg).await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("[artifacts] list failed: {e}");
                return thrown_internal_error();
            }
        };
        // Agents see org/public artifacts, ones they've been granted, and —
        // for a personal assistant — the owner's own private ones. `&[]`
        // grants here is right: the granted-set clause above already covers
        // grants, so can_read_agent is asked only its owner arm.
        let artifacts: Vec<&Artifact> = artifacts
            .iter()
            .filter(|a| {
                a.visibility != "private"
                    || granted.contains(&a.id)
                    || can_read_agent(&crate::artifacts::guarded(a), &name, owner.as_deref(), &[])
            })
            .collect();
        return Json(json!({ "artifacts": artifacts })).into_response();
    }
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let who = who_of(&user);
    let granted = match granted_item_ids(&state.pg, ITEM_ARTIFACT, &user.id).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[artifacts] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    let artifacts = match list_artifacts(&state.pg).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] list failed: {e}");
            return thrown_internal_error();
        }
    };
    let artifacts: Vec<&Artifact> = artifacts
        .iter()
        .filter(|a| {
            granted.contains(&a.id)
                || can_read(
                    &crate::artifacts::guarded(a),
                    Some(&user.id),
                    who.as_deref(),
                    &[],
                )
        })
        .collect();
    Json(json!({ "artifacts": artifacts })).into_response()
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
    let body = match parse_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Some(caller) = caller {
        let name = caller.model.clone();
        // WHO the agent works for decides two things, deliberately two
        // different answers. A PERSONAL assistant's output stays private to
        // its owner (they can share it; the assistant cannot make it
        // org-wide) — pa_owner drives that visibility branch below. The
        // owner STAMP is the attribution ladder: a PA's owner, or the human
        // an org agent is answering — a general org agent's output stays
        // org-visible, but now belongs to a person who can govern it instead
        // of an allow-list. Both asks need the CALLER: attaching output to a
        // human's account is owner-proxying, proven identity only.
        let pa_owner =
            match crate::users::assistant_owner_for(&state.pg, &AgentSubject::Caller(caller.clone())).await
            {
                Ok(o) => o,
                Err(e) => {
                    tracing::error!("[artifacts] owner lookup failed: {e}");
                    return thrown_internal_error();
                }
            };
        let responsible = match crate::attribution::responsible_user_for(
            &state.pg,
            state.redis().await.ok(),
            &AgentSubject::Caller(caller),
        )
        .await
        {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("[artifacts] responsible-user lookup failed: {e}");
                return thrown_internal_error();
            }
        };
        let folder_id = match body.folder.as_deref() {
            Some(f) if !f.is_empty() => named_root_folder(&state.pg, f, &name).await,
            _ => None,
        };
        let cabinet = agent_category_folder(
            &state.pg,
            &describe_agent(&name).label,
            AGENT_CATEGORIES[0], // "Documents"
            &name,
        )
        .await;
        let artifact = match create_artifact(
            &state.pg,
            body.kind.as_deref(),
            body.title.as_deref(),
            &name,
            responsible.as_deref(),
            // Named folder (find-or-create) when the agent files deliberately;
            // its own cabinet otherwise.
            folder_id.or(cabinet).as_deref(),
        )
        .await
        {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("[artifacts] create failed: {e}");
                return thrown_internal_error();
            }
        };
        if let Err(e) = set_editors(
            &state.pg,
            ITEM_ARTIFACT,
            &artifact.id,
            &[EditorGrant {
                principal_type: "agent".into(),
                principal_id: name.clone(),
                role: "editor".into(),
            }],
        )
        .await
        {
            tracing::error!("[artifacts] editor grant failed: {e}");
            return thrown_internal_error();
        }
        let updated = match save_artifact(
            &state.pg,
            &artifact.id,
            if pa_owner.is_some() {
                SaveArtifactPatch {
                    body: body.body.as_deref(),
                    visibility: Some("private"),
                    edit_policy: Some("owner"),
                    ..Default::default()
                }
            } else {
                SaveArtifactPatch {
                    body: body.body.as_deref(),
                    visibility: Some(body.visibility.as_deref().unwrap_or("org")),
                    edit_policy: Some("org"),
                    ..Default::default()
                }
            },
            &name,
        )
        .await
        {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("[artifacts] save failed: {e}");
                return thrown_internal_error();
            }
        };
        return Json(json!({ "artifact": updated.unwrap_or(artifact) })).into_response();
    }

    let user = match require_perm(&state, &headers, "artifacts.create").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let actor = who_of(&user).unwrap_or_else(|| "user".into());
    let artifact = match create_artifact(
        &state.pg,
        body.kind.as_deref(),
        body.title.as_deref(),
        &actor,
        Some(&user.id),
        None,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] create failed: {e}");
            return thrown_internal_error();
        }
    };
    let updated = if body.body.is_some() {
        match save_artifact(
            &state.pg,
            &artifact.id,
            SaveArtifactPatch {
                body: body.body.as_deref(),
                ..Default::default()
            },
            &actor,
        )
        .await
        {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("[artifacts] save failed: {e}");
                return thrown_internal_error();
            }
        }
    } else {
        None
    };
    let (pg, audit_actor, target_id, target_label) = (
        state.pg.clone(),
        actor_of(&user),
        artifact.id.clone(),
        artifact.title.clone(),
    );
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &audit_actor,
                action: "artifact.create",
                target_type: "artifact",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "artifact": updated.unwrap_or(artifact) })).into_response()
}
