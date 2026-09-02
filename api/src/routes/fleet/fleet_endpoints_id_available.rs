// GET /api/fleet/endpoints/{id}/available — port of
// ui/src/routes/api/fleet.endpoints.$id.available.ts. What this provider
// actually offers right now (live /models call, server-side, keys never leave
// the box). Admin.
//
// THE CALL IS ALREADY BEING MADE, so it also refreshes the stored catalog: an
// admin opening the model picker is the moment the provider's own answer is
// freshest and the moment its descriptive fields are most worth keeping.

use crate::error::house_error;
use crate::error::thrown_internal_error;
use crate::gateway::provider::catalog_models;
use crate::gateway::registry::list_endpoints;
use crate::model::catalog::refresh_endpoint_catalog_with;
use crate::session::require_admin;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let eps = match list_endpoints(&state.pg).await {
        Ok(e) => e,
        Err(_) => return thrown_internal_error(),
    };
    let Some(ep) = eps.iter().find(|e| e.id == id).cloned() else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let models = match catalog_models(&state, &ep).await {
        Ok(m) => m,
        // The catch arm: models [], note = the thrown message (undici's bare
        // "fetch failed" for transport failures).
        Err(message) => {
            return Json(json!({ "models": [], "note": message })).into_response();
        }
    };
    // The ids stay the contract this route has always had; the descriptive
    // fields ride along for the picker that wants to show a window or a price.
    let out = json!({
        "models": models.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        "catalog": models.iter().map(|m| m.to_json()).collect::<Vec<_>>(),
    });
    // Storing is a side effect of answering and must never be able to fail the
    // answer: the picker's job is to list models, and an `app_settings` blip
    // is not a reason to show an admin an empty dropdown.
    let st = state.clone();
    tokio::spawn(async move {
        let _ = refresh_endpoint_catalog_with(&st, &ep, models).await;
    });
    Json(out).into_response()
}
