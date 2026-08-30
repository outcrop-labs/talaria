// /api/kb/docs/{id}/move — port of ui/src/routes/api/kb.docs.$id.move.ts.
// Reparent / reorder a doc in the sidebar tree. Rejects cycles server-side.
// Moving a doc is an edit of it, so it takes the same gate the PUT does —
// otherwise any signed-in member could reparent a private doc out of a
// folder they can't even read. `moveDoc` itself only detects cycles and
// always has.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::audit::{AuditEntry, log_audit};
use crate::body::{NumKind, as_object, nullable_uuid_member, number_member, parse};
use crate::error::house_error;
use crate::kb::{effective_doc_perms, get_doc, move_doc};
use crate::kb_perms::can_edit_human;
use crate::session::{actor_of, require_perm, who_of};
use crate::state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let existing = match get_doc(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[kb] doc read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(existing) = existing else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let user = match require_perm(&state, &headers, "kb.edit").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let eff = match effective_doc_perms(&state.pg, &existing).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[kb] perms read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let who = who_of(&user);
    if !can_edit_human(&eff.perms, Some(&user.id), who.as_deref(), &eff.grants) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let parent_id = match nullable_uuid_member(obj, "parentId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.number().int().default(0) — absent is 0.
    let sort = match obj.get("sort") {
        None => 0i32,
        Some(_) => match number_member(obj, "sort", NumKind::Int, i32::MIN as f64, i32::MAX as f64)
        {
            Ok(v) => v as i32,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        },
    };
    // The tree is per-space: a cross-space parent would drag the doc under a
    // folder whose audience never vetted it.
    if let Some(parent) = &parent_id {
        let parent_doc = match get_doc(&state.pg, parent).await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("[kb] parent read failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        };
        match parent_doc {
            Some(p) if p.space_id == existing.space_id => {}
            _ => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    "parent must live in the same space",
                );
            }
        }
    }
    let doc = match move_doc(&state.pg, &id, parent_id.as_deref(), sort).await {
        Ok(Some(d)) => d,
        Ok(None) => return house_error(StatusCode::BAD_REQUEST, "invalid move"),
        Err(e) => {
            tracing::error!("[kb] move failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let (pg, actor, target_id, target_label, after) = (
        state.pg.clone(),
        actor_of(&user),
        id.clone(),
        doc.title.clone(),
        json!({ "parentId": parent_id, "sort": sort }),
    );
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "kb.doc_move",
                target_type: "kb-doc",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: Some(after),
            },
        )
        .await;
    });
    Json(json!({ "doc": doc })).into_response()
}
