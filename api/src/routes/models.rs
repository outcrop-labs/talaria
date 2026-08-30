// /api/models — port of ui/src/routes/api/models.ts. The gateway model
// catalog for signed-in users (the /api/llm/v1/models twin without an API
// key) — powers the preferred-model picker. Role-filtered: members see only
// what the admin allowlist permits; admins see everything. Each model carries
// a pretty label + a "what it's good at" blurb when the public catalog knows
// it. Also says which model the caller's muse would use.

use crate::error::house_error;
use crate::harness_model::muse_model_for;
use crate::model_access::gateway_models_for;
use crate::model_info::{maybe_rewrite_blurbs, model_info};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // New registered models get their org-voice blurb (throttled, detached —
    // the pass never blocks the request it was kicked from). The kick is the
    // sweep's only trigger while TS owns the schedule — the proxy shadows
    // TS's own kick, so without this one the org-voice sweep is dark in every
    // proxied install. Once THIS process owns the schedule the blurb-rewrite
    // job carries the cadence and the kick stands down: the throttle would
    // serialize the two anyway, but two triggers racing one throttle is two
    // model calls' worth of contention for one batch of pending ids.
    if !crate::scheduler::rust_owns_schedule() {
        maybe_rewrite_blurbs(&state);
    }
    let catalog = match gateway_models_for(&state.pg, &user.role).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[models] gateway catalog read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
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
            Err(e) => {
                tracing::error!("[models] blurb override read failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        };
        // TS spreads `{...m, label, blurb}` — the three catalog keys in their
        // order, then the two info keys. A model the public catalog doesn't
        // know serves the catalog row alone.
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
        Err(e) => {
            tracing::error!("[models] muse resolution failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    Json(json!({ "models": models, "effective": effective })).into_response()
}
