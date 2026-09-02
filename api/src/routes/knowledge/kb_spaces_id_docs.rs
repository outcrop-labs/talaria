// /api/kb/spaces/{id}/docs. A space's doc tree. GET → doc metadata list
// (agents gate on agent space-access — org/public, a grant, or a personal
// assistant's owner's own reach — then per-doc audience on the same terms;
// humans gate on the folder, then inherited docs show and customized ones
// filter). POST → new doc (agent docs are drafts owned by the agent's
// responsible user — a personal assistant's owner, or the human an org agent
// is answering; humans create where they can read).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, optional_max_string_member, optional_uuid_member, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{
    ITEM_SPACE, can_read, can_read_agent, granted_item_ids, granted_item_ids_for_agent,
    list_editors,
};
use crate::kb::{NewDoc, create_doc, get_space, list_docs, save_doc};
use crate::retrieval::{embed, qdrant};
use crate::session::{actor_of, require_perm, who_of};
use crate::state::AppState;

use super::kb_spaces::guarded_of;

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
        return Json(json!({ "docs": [] })).into_response();
    };
    // Agents (over MCP): gate the tree on agent space-access, then filter docs
    // by their own audience (inherited from the readable folder, or granted).
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let docs = match list_docs(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] doc list failed: {e}");
            return thrown_internal_error();
        }
    };
    let space_editors = match list_editors(&state.pg, ITEM_SPACE, &id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] editor read failed: {e}");
            return thrown_internal_error();
        }
    };
    if let Some(caller) = caller {
        let owner =
            match crate::users::assistant_owner_for(&state.pg, &AgentSubject::Caller(caller.clone()))
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[kb] owner resolve failed: {e}");
                    return thrown_internal_error();
                }
            };
        if !can_read_agent(
            &guarded_of(&space),
            &caller.model,
            owner.as_deref(),
            &space_editors,
        ) {
            return Json(json!({ "docs": [] })).into_response();
        }
        let granted = match granted_item_ids_for_agent(&state.pg, "doc", &caller.model).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[kb] grant read failed: {e}");
                return thrown_internal_error();
            }
        };
        let docs: Vec<_> = docs
            .into_iter()
            .filter(|d| {
                d.perms_inherited
                    || granted.contains(&d.id)
                    || doc_readable_by_agent(d, owner.as_deref())
            })
            .collect();
        return Json(json!({ "docs": docs })).into_response();
    }
    let user = match require_perm(&state, &headers, "kb.edit").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // Gate the whole tree on folder access first.
    let who = who_of(&user);
    if !can_read(
        &guarded_of(&space),
        Some(&user.id),
        who.as_deref(),
        &space_editors,
    ) {
        return Json(json!({ "docs": [] })).into_response();
    }
    // Inherited docs are as visible as the (readable) folder, so they show.
    // Customized docs are filtered by their own audience (or an explicit
    // grant) — can_read with no grant list, the same shape as the spaces
    // filter (the granted-set beside it is the grant half).
    let granted = match granted_item_ids(&state.pg, "doc", &user.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[kb] grant read failed: {e}");
            return thrown_internal_error();
        }
    };
    let docs: Vec<_> = docs
        .into_iter()
        .filter(|d| {
            d.perms_inherited
                || granted.contains(&d.id)
                || can_read(
                    &crate::kb::perms::Guarded {
                        owner_user_id: d.owner_user_id.clone(),
                        created_by: d.created_by.clone(),
                        visibility: d.visibility.clone(),
                        edit_policy: d.edit_policy.clone(),
                    },
                    Some(&user.id),
                    who.as_deref(),
                    &[],
                )
        })
        .collect();
    Json(json!({ "docs": docs })).into_response()
}

/// can_read_agent's non-grant halves on a doc META row (no grants in hand —
/// the granted-set check beside it covers the grant half): org/public, or a
/// private doc the assistant's OWNER owns — the inherited-read arm.
fn doc_readable_by_agent(d: &crate::kb::KbDocMeta, owner_user_id: Option<&str>) -> bool {
    d.visibility != "private"
        || (owner_user_id.is_some() && d.owner_user_id.as_deref() == owner_user_id)
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let title = match optional_max_string_member(obj, "title", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let parent_id = match optional_uuid_member(obj, "parentId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let kind = match crate::body::optional_enum_member(obj, "kind", &["human", "agent"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Initial markdown body (the MCP create_kb_doc path sets it in one shot).
    let body_text = match optional_max_string_member(obj, "body", 500_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // Agents (over MCP) create docs in spaces they can read. Agent docs start
    // as drafts — they never ground the org brain until a human officializes
    // them, so the write guardrail holds.
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Some(caller) = caller {
        let model = caller.model.clone();
        let space = match get_space(&state.pg, &id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[kb] space read failed: {e}");
                return thrown_internal_error();
            }
        };
        // Two different questions about the same caller, deliberately two
        // different answers. The READ gate below uses the personal
        // assistant's owner only — an org agent's read reach is its own, and
        // feeding it the responsible user here would let it read that
        // human's private spaces. The STAMP on the new doc uses the
        // attribution ladder — a personal assistant's owner, or the human an
        // org agent is answering — because whose row it is and who it may
        // read for are different facts.
        let owner =
            match crate::users::assistant_owner_for(&state.pg, &AgentSubject::Caller(caller.clone())).await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[kb] owner resolve failed: {e}");
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
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[kb] responsible-user resolve failed: {e}");
                return thrown_internal_error();
            }
        };
        let readable = match (&space, list_editors(&state.pg, ITEM_SPACE, &id).await) {
            (Some(s), Ok(editors)) => {
                can_read_agent(&guarded_of(s), &model, owner.as_deref(), &editors)
            }
            _ => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        };
        if !readable {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        let doc = match create_doc(
            &state.pg,
            &NewDoc {
                space_id: id.clone(),
                parent_id,
                title,
                kind: Some("human".into()),
                created_by: model.clone(),
                owner_user_id: responsible,
            },
        )
        .await
        {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("[kb] doc create failed: {e}");
                return thrown_internal_error();
            }
        };
        let saved = match &body_text {
            Some(b) => {
                save_doc(
                    &state.pg,
                    &qdrant::real_deps(),
                    &embed::real_deps(),
                    &doc.id,
                    &crate::kb::DocPatch {
                        body: Some(b.clone()),
                        ..Default::default()
                    },
                    &model,
                )
                .await
            }
            None => Ok(Some(doc.clone())),
        };
        return match saved {
            Ok(Some(d)) => Json(json!({ "doc": d })).into_response(),
            _ => Json(json!({ "doc": doc })).into_response(),
        };
    }

    // Humans create where they can read: the same gate the GET on this route
    // uses, so a private space stays closed on write as well as on read —
    // requiring only a session would let any signed-in member drop a doc into
    // someone else's private space.
    let user = match require_perm(&state, &headers, "kb.edit").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
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
    let editors = match list_editors(&state.pg, ITEM_SPACE, &id).await {
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
    let created_by = who_of(&user).unwrap_or_else(|| "user".into());
    let doc = match create_doc(
        &state.pg,
        &NewDoc {
            space_id: id.clone(),
            parent_id,
            title,
            kind,
            created_by: created_by.clone(),
            owner_user_id: Some(user.id.clone()),
        },
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[kb] doc create failed: {e}");
            return thrown_internal_error();
        }
    };
    let saved = match &body_text {
        Some(b) => {
            save_doc(
                &state.pg,
                &qdrant::real_deps(),
                &embed::real_deps(),
                &doc.id,
                &crate::kb::DocPatch {
                    body: Some(b.clone()),
                    ..Default::default()
                },
                &created_by,
            )
            .await
        }
        None => Ok(Some(doc.clone())),
    };
    let (pg, actor, target_id, target_label) = (
        state.pg.clone(),
        actor_of(&user),
        doc.id.clone(),
        doc.title.clone(),
    );
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "kb.doc_create",
                target_type: "kb-doc",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: None,
            },
        )
        .await;
    });
    match saved {
        Ok(Some(d)) => Json(json!({ "doc": d })).into_response(),
        _ => Json(json!({ "doc": doc })).into_response(),
    }
}
