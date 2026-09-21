// /api/rag/collections/{id}. One collection, admin. PUT → replace its access
// bindings wholesale; an unknown (but well-formed) id 404s. DELETE → drop it
// (the two auto collections are protected); a missing id is a no-op delete —
// it still answers ok, it still audits.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::retrieval::collections;
use talaria_api_facades::retrieval::qdrant;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{array_msg, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

use super::rag_collections::parse_bindings;

/// The one delete refusal answered as a 400; every other failure falls to the
/// generic 500.
const AUTO_DELETE_REFUSAL: &str = "auto collections cannot be deleted";

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // bindings are REQUIRED on this route: absent yields the array message on
    // "undefined", not an empty set.
    let bindings = match parse_bindings(obj.get("bindings")) {
        Ok(Some(v)) => v,
        Ok(None) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &array_msg("undefined"),
            ));
        }
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Body validation runs before the id's uuid gate — an invalid body
    // answers the validation error, never the uuid one.
    if let Some(gate) = talaria_params::uuid_gate("rag-collections", "PUT", &id) {
        return Ok(gate);
    }
    match collections::get_collection(&state.pg, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[knowledge] get_collection failed", e)),
    }
    if let Err(e) = collections::set_bindings(&state.pg, &id, &bindings).await {
        return Ok(internal("[knowledge] set_bindings failed", e));
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
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("rag-collections", "DELETE", &id) {
        return Ok(gate);
    }
    let qd = qdrant::real_deps();
    match collections::delete_collection_by_id(&state.pg, &qd, &id).await {
        Ok(()) => {}
        Err(msg) if msg == AUTO_DELETE_REFUSAL => {
            return Ok(house_error(StatusCode::BAD_REQUEST, &msg));
        }
        Err(e) => return Ok(internal("[knowledge] delete_collection_by_id failed", e)),
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
    Ok(Json(json!({ "ok": true })).into_response())
}
