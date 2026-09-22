// /api/models.
// The gateway model catalog for signed-in users (the /api/llm/v1/models
// twin without an API key) — powers the preferred-model picker.
// Role-filtered: members see only what the admin allowlist permits; admins
// see everything. Each model carries a pretty label + a "what it's good at"
// blurb when the public catalog knows it. Also says which model the caller's
// muse would use.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::model::access::gateway_models_for;
use talaria_api_facades::model::info::model_info;
use talaria_error::internal;
use talaria_harness_model::muse_model_for;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // New registered models get their org-voice blurb from the registered
    // blurb-rewrite job (model_info.rs) — same pass, same throttle, on the
    // cadence. This route never kicks the job: a kick AND a job on one
    // throttle is two model calls for one batch of pending ids.
    let catalog = match gateway_models_for(&state.pg, &user.role).await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[models] gateway catalog read failed", e)),
    };
    let mut models = Vec::with_capacity(catalog.len());
    for m in &catalog {
        // A qualified pin ("ep/model") looks its BARE upstream id up in the
        // public catalog — the blurb knows "deepseek/v4", not
        // "openrouter/deepseek/v4". Bare ids are already the lookup spelling.
        let lookup = if m.qualified {
            &m.id[m.id.find('/').map_or(0, |i| i + 1)..]
        } else {
            &m.id[..]
        };
        let info = match model_info(&state.pg, lookup).await {
            Ok(i) => i,
            Err(e) => return Ok(internal("[models] blurb override read failed", e)),
        };
        // `{...m, label, blurb}` — the three catalog keys in their order,
        // then the two info keys. A model the public catalog doesn't know
        // serves the catalog row alone.
        models.push(match info {
            Some(info) => json!({
                "id": m.id,
                "endpoints": m.endpoints,
                "qualified": m.qualified,
                "label": info.label,
                "blurb": info.blurb,
            }),
            None => m.to_json(),
        });
    }
    let effective = match muse_model_for(&state.pg, &user.id).await {
        Ok(e) => e,
        Err(e) => return Ok(internal("[models] muse resolution failed", e)),
    };
    Ok(Json(json!({ "models": models, "effective": effective })).into_response())
}
