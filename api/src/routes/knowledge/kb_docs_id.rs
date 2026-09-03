// /api/kb/docs/{id}. One KB doc. Read/edit gated by the doc's EFFECTIVE
// audience — inherited from its folder unless customized. Sharing changes are
// owner-only; routing owner-only;
// officializing needs kb.official. Agents (by key) read org/public, granted
// private, and — when a personal assistant — their owner's private docs
// (can_read_agent's owner arm: the brain already serves those, so the file
// plane refusing them was two planes disagreeing). Agents only edit content
// when they authored the doc, hold an editor grant, or are an elevated
// assistant on non-private material — and never touch sharing, officialness
// or routing. Delete is the one power held tighter than edit: an edit is
// versioned and recoverable, a delete is not, so an agent may delete only a
// doc it authored (its own drafts and misfiles) — anything else waits for a
// human.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, optional_boolean_member, optional_enum_member, optional_max_string_member, parse,
};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::okf::{generate_doc_okf, queue_doc_okf};
use crate::kb::perms::{
    EditorGrant, Guarded, ITEM_DOC, can_edit_agent, can_edit_human, can_govern, can_read,
    can_read_agent, set_editors,
};
use crate::kb::{
    DocPatch, delete_doc, effective_doc_perms, get_doc, save_doc, set_doc_routing, set_official,
};
use crate::retrieval::{embed, qdrant};
use crate::session::{actor_of, require_perm, require_user, who_of};
use crate::state::AppState;

use super::kb_spaces_id::{editors_json, parse_editors};

/// `{ ...doc, visibility, editPolicy, governs? }` — the GET/PUT response
/// overlay. Overriding a key keeps it at its ORIGINAL position (the struct's
/// declaration order); a new key appends. preserve_order keeps the serde_json
/// Map insertion-ordered so the wire carries that key order.
fn doc_overlay(doc: &crate::kb::KbDoc, eff: &Guarded, governs: Option<bool>) -> Value {
    let mut v = serde_json::to_value(doc).expect("KbDoc serializes");
    let obj = v.as_object_mut().expect("KbDoc serializes to an object");
    obj.insert("visibility".into(), json!(eff.visibility));
    obj.insert("editPolicy".into(), json!(eff.edit_policy));
    if let Some(g) = governs {
        obj.insert("governs".into(), json!(g));
    }
    v
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let doc = match get_doc(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[kb] doc read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(doc) = doc else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let eff = match effective_doc_perms(&state.pg, &doc).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[kb] perms read failed: {e}");
            return thrown_internal_error();
        }
    };
    // Agents (over MCP) read by effective audience: org/public, a grant — or,
    // for a personal assistant, its owner's own read reach (can_read_agent).
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
                tracing::error!("[kb] owner resolve failed: {e}");
                return thrown_internal_error();
            }
        };
        if !can_read_agent(&eff.perms, &reader.model, owner.as_deref(), &eff.grants) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        return Json(
            json!({ "doc": doc_overlay(&doc, &eff.perms, None), "editors": editors_json(&eff.grants) }),
        )
        .into_response();
    }
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let who = who_of(&user);
    if !can_read(&eff.perms, Some(&user.id), who.as_deref(), &eff.grants) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let governs =
        match can_govern(&state.pg, &eff.perms, &user.id, &user.role, who.as_deref()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::error!("[kb] govern check failed: {e}");
                return thrown_internal_error();
            }
        };
    // Surface the effective visibility/policy so the UI shows what applies.
    Json(
        json!({ "doc": doc_overlay(&doc, &eff.perms, Some(governs)), "editors": editors_json(&eff.grants) }),
    )
    .into_response()
}

/// The PUT body, holding every field the handler's state machine mutates.
struct PutBody {
    title: Option<String>,
    body: Option<String>,
    icon: Option<Option<String>>,
    visibility: Option<String>,
    edit_policy: Option<String>,
    editors: Option<Vec<EditorGrant>>,
    perms_inherited: Option<bool>,
    parent_id: Option<Option<String>>,
    official: Option<bool>,
    regenerate_okf: bool,
    rag_routing: Option<String>,
}

fn parse_put_body(obj: &serde_json::Map<String, Value>) -> Result<PutBody, String> {
    Ok(PutBody {
        title: optional_max_string_member(obj, "title", 200)?,
        body: optional_max_string_member(obj, "body", 500_000)?,
        icon: crate::body::present_nullable_max_string_member(obj, "icon", 16)?,
        visibility: optional_enum_member(obj, "visibility", &["private", "org", "public"])?,
        edit_policy: optional_enum_member(obj, "editPolicy", &["owner", "org", "restricted"])?,
        editors: parse_editors(obj.get("editors"))?,
        perms_inherited: optional_boolean_member(obj, "permsInherited")?,
        parent_id: crate::body::present_nullable_uuid_member(obj, "parentId")?,
        official: optional_boolean_member(obj, "official")?,
        regenerate_okf: optional_boolean_member(obj, "regenerateOkf")?.unwrap_or(false),
        rag_routing: optional_max_string_member(obj, "ragRouting", 60)?,
    })
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let doc = match get_doc(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[kb] doc read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(doc) = doc else {
        return house_error(StatusCode::NOT_FOUND, "not found");
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
    let eff = match effective_doc_perms(&state.pg, &doc).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[kb] perms read failed: {e}");
            return thrown_internal_error();
        }
    };

    let actor: String;
    let mut owner = false;
    let agent = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if let Some(agent) = agent.clone() {
        let name = agent.model.clone();
        // Its own authored doc, an editor grant — or an admin-elevated
        // assistant on any non-private doc. Without the authorship rule an
        // agent gets 403 on the doc it JUST created (create_kb_doc grants
        // nothing) and works around it by creating duplicates.
        let elevated = eff.perms.visibility != "private"
            && match crate::users::is_elevated_assistant(
                &state.pg,
                &AgentSubject::Caller(agent.clone()),
            )
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[kb] elevation read failed: {e}");
                    return thrown_internal_error();
                }
            };
        let may_edit = doc.created_by.as_deref() == Some(name.as_str())
            || can_edit_agent(&name, &eff.grants)
            || elevated;
        if !may_edit {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        actor = name;
        body.visibility = None;
        body.edit_policy = None;
        body.editors = None;
        body.perms_inherited = None;
        body.official = None;
    } else {
        let user = match require_perm(&state, &headers, "kb.edit").await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        let who = who_of(&user);
        if !can_edit_human(&eff.perms, Some(&user.id), who.as_deref(), &eff.grants) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        // Marking OFFICIAL grounds every agent — a curation power of its own.
        if body.official.is_some()
            && !matches!(
                crate::permissions::has_perm(&state.pg, &user.id, &user.role, "kb.official").await,
                Ok(true)
            )
        {
            return house_error(
                StatusCode::FORBIDDEN,
                "no permission to curate official knowledge",
            );
        }
        actor = actor_of(&user);
        owner = match can_govern(&state.pg, &eff.perms, &user.id, &user.role, who.as_deref()).await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[kb] govern check failed: {e}");
                return thrown_internal_error();
            }
        };
        let sharing = body.visibility.is_some()
            || body.edit_policy.is_some()
            || body.editors.is_some()
            || body.perms_inherited.is_some();
        if !owner && sharing {
            return house_error(StatusCode::FORBIDDEN, "only the owner can change sharing");
        }
        // Routing decides which brain can retrieve the doc — owner's call.
        if !owner && body.rag_routing.is_some() {
            return house_error(
                StatusCode::FORBIDDEN,
                "only the owner can change brain routing",
            );
        }
    }
    if agent.is_some() {
        body.rag_routing = None;
    }

    if owner {
        if body.perms_inherited == Some(true) {
            // Reset to inherit from the folder — drop the doc's own grants.
            if set_editors(&state.pg, ITEM_DOC, &id, &[]).await.is_err() {
                return thrown_internal_error();
            }
            body.editors = None;
        } else if body.visibility.is_some() || body.edit_policy.is_some() || body.editors.is_some()
        {
            // Any explicit sharing change customizes the doc (stops
            // inheriting).
            body.perms_inherited = Some(false);
            if let Some(editors) = &body.editors
                && set_editors(&state.pg, ITEM_DOC, &id, editors)
                    .await
                    .is_err()
            {
                return thrown_internal_error();
            }
        }
    }

    if let Some(routing) = body.rag_routing.clone() {
        // set_doc_routing's own error (the "unknown brain" sentence) IS the
        // answer, at 400.
        if let Err(msg) = set_doc_routing(
            &state.pg,
            &qdrant::real_deps(),
            &embed::real_deps(),
            &id,
            &routing,
            &actor,
        )
        .await
        {
            return house_error(StatusCode::BAD_REQUEST, &msg);
        }
    }
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    let mut updated = match save_doc(
        &state.pg,
        &qd,
        &ed,
        &id,
        &DocPatch {
            title: body.title.clone(),
            body: body.body.clone(),
            icon: body.icon.clone(),
            visibility: body.visibility.clone(),
            edit_policy: body.edit_policy.clone(),
            perms_inherited: body.perms_inherited,
            parent_id: body.parent_id.clone(),
        },
        &actor,
    )
    .await
    {
        Ok(Some(d)) => d,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[kb] doc save failed: {e}");
            return thrown_internal_error();
        }
    };
    if let Some(official) = body.official
        && official != updated.official
    {
        updated = match set_official(&state.pg, &qd, &ed, &id, official, &actor).await {
            Ok(Some(d)) => d,
            Ok(None) => updated,
            Err(e) => {
                tracing::error!("[kb] officialize failed: {e}");
                return thrown_internal_error();
            }
        };
        let (pg, actor_, action, target_id, target_label) = (
            state.pg.clone(),
            actor.clone(),
            if official {
                "kb.officialize"
            } else {
                "kb.deofficialize"
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
                    target_type: "kb-doc",
                    target_id: Some(&target_id),
                    target_label: Some(&target_label),
                    before: None,
                    after: None,
                },
            )
            .await;
        });
        // the Librarian writes/clears this doc's OKF
        queue_doc_okf(state.clone(), id.clone());
    } else if updated.official && body.body.is_some() {
        // promoted content changed
        queue_doc_okf(state.clone(), id.clone());
    }
    if body.regenerate_okf {
        // Explicit regen runs against the FINAL state (post any
        // promote/demote).
        let _ = generate_doc_okf(&state, &id).await;
        if let Ok(Some(d)) = get_doc(&state.pg, &id).await {
            updated = d;
        }
    }
    let eff = match effective_doc_perms(&state.pg, &updated).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[kb] perms read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(
        json!({ "doc": doc_overlay(&updated, &eff.perms, None), "editors": editors_json(&eff.grants) }),
    )
    .into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let doc = match get_doc(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[kb] doc read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(doc) = doc else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    // Delete is unrecoverable — versions and embeddings go with the row — so
    // an agent's admission here is NARROWER than the edit it holds on the same
    // doc: authorship only. An editor grant or elevation buys edits, which are
    // versioned; a doc somebody else created leaves with a human, and the
    // agent's honest move for a misfiled doc it cannot delete is move, not
    // duplicate.
    let agent = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let actor: String;
    if let Some(agent) = agent {
        if doc.created_by.as_deref() != Some(agent.model.as_str()) {
            return house_error(
                StatusCode::FORBIDDEN,
                "agents can only delete docs they created",
            );
        }
        actor = agent.model;
    } else {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        let eff = match effective_doc_perms(&state.pg, &doc).await {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("[kb] perms read failed: {e}");
                return thrown_internal_error();
            }
        };
        let who = who_of(&user);
        if !can_edit_human(&eff.perms, Some(&user.id), who.as_deref(), &eff.grants) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        actor = actor_of(&user);
    }
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    if delete_doc(&state.pg, &qd, &ed, &id).await.is_err() {
        return thrown_internal_error();
    }
    // The one kb write with no undo, so it is the one that always lands in the
    // audit log — whoever pulled the trigger.
    let (pg, target_id, target_label) = (state.pg.clone(), id.clone(), doc.title.clone());
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "kb.doc_delete",
                target_type: "kb-doc",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "ok": true })).into_response()
}
