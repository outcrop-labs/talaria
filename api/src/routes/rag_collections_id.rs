// /api/rag/collections/{id} — port of ui/src/routes/api/rag.collections.$id.ts.
// One collection, admin. PUT → replace its access bindings wholesale.
// DELETE → drop it (the two auto collections are protected). Neither verb
// 404s a missing id: an unknown collection's bindings set is a no-op write,
// and its delete is a no-op delete — both still answer ok, both still audit.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::audit::{AuditEntry, log_audit};
use crate::body::{array_msg, as_object, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::retrieval::collections;
use crate::retrieval::qdrant;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;

use super::rag_collections::parse_bindings;

/// The one delete refusal TS answers as a 400; every other failure there is
/// an unhandled throw to the platform's generic 500.
const AUTO_DELETE_REFUSAL: &str = "auto collections cannot be deleted";

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // REQUIRED here (no `.optional()` in this schema): absent bindings is
    // zod's array message on undefined, not an empty set.
    let bindings = match parse_bindings(obj.get("bindings")) {
        Ok(Some(v)) => v,
        Ok(None) => return house_error(StatusCode::BAD_REQUEST, &array_msg("undefined")),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // TS's parseBody runs before the SQL; a body that fails validation never
    // reaches the id's uuid cast, so neither does the gate.
    if let Some(gate) = crate::params::uuid_gate("rag-collections", "PUT", &id) {
        return gate;
    }
    match collections::get_collection(&state.pg, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(_) => return thrown_internal_error(),
    }
    if collections::set_bindings(&state.pg, &id, &bindings)
        .await
        .is_err()
    {
        return thrown_internal_error();
    }
    let (pg, actor, target_id) = (state.pg.clone(), actor_of(&user), id.clone());
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "rag.bindings",
                target_type: "rag_collection",
                target_id: Some(&target_id),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("rag-collections", "DELETE", &id) {
        return gate;
    }
    let qd = qdrant::real_deps();
    match collections::delete_collection_by_id(&state.pg, &qd, &id).await {
        Ok(()) => {}
        Err(msg) if msg == AUTO_DELETE_REFUSAL => {
            return house_error(StatusCode::BAD_REQUEST, &msg);
        }
        Err(_) => return thrown_internal_error(),
    }
    let (pg, actor, target_id) = (state.pg.clone(), actor_of(&user), id.clone());
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "rag.delete",
                target_type: "rag_collection",
                target_id: Some(&target_id),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(json!({ "ok": true })).into_response()
}
