// /api/models/efforts — port of ui/src/routes/api/models.efforts.ts. The
// composer's effort-picker feed: which reasoning-effort levels THIS model id
// may be asked for, plus the default it should start from. Thin by the house
// rule (routes parse and serialize; the decisions live in model_efforts.rs
// and persona.rs) — the route adds only the auth gate and the query string.
//
// `model` accepts both spellings the chat surfaces speak: a fleet persona id
// (base or tier) or a gateway catalog id. `efforts` is `[]` when nothing
// vouches for any level, and `[]` is what the composer renders as "no
// picker". `default` is the AGENT-CONFIGURED effort when the id is a persona
// whose config names one — the pick an admin set beside the model in the
// agent editor, validated against the same levels; null everywhere else.
// Precedence at the surfaces: conversation pick > agent default > the user's
// platform default > the model's own.
//
// An empty first read runs the BACKFILL before answering: a catalog stored by
// a build before the effort extraction has no levels for anyone, and the
// picker would stay hidden until an admin happened to re-open the model
// modal. ensure_efforts_catalog refreshes the serving endpoints' catalogs
// (once — post-feature entries never re-trigger) and answers from the fresh
// store, so the first request after an upgrade may take a few seconds and
// every one after is a settings read.

use crate::error::house_error;
use crate::model::efforts::{efforts_for_model, ensure_efforts_catalog};
use crate::persona::persona_configured_effort;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    // searchParams.get('model')?.trim() — the client sends encodeURIComponent,
    // so an OpenRouter name's slashes arrive as %2F and must be decoded
    // (form_urlencoded is URLSearchParams' decoder), and `get` answers with
    // the FIRST occurrence when a query string repeats the key, so this finds
    // rather than collects.
    let model = uri.query().and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "model")
            .map(|(_, v)| v.into_owned())
    });
    let Some(model) = model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
    else {
        return house_error(StatusCode::BAD_REQUEST, "model is required");
    };
    let mut efforts = efforts_for_model(&state.pg, &model).await;
    if efforts.is_empty() {
        efforts = ensure_efforts_catalog(&state, &model).await;
    }
    if efforts.is_empty() {
        return Json(json!({ "efforts": [], "default": null })).into_response();
    }
    // The configured default, held against the levels just read: a level the
    // model no longer publishes (the admin swapped models, the metadata
    // changed) is not a default, it is a stale string. persona_configured_effort
    // never fails — the TS side's `.catch(() => null)` is already its nature.
    let configured = persona_configured_effort(&state.pg, &model).await;
    let default = configured.filter(|c| efforts.contains(c));
    Json(json!({ "efforts": efforts, "default": default })).into_response()
}
