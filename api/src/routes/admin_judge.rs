// /api/admin/judge — port of ui/src/routes/api/admin.judge.ts. The
// automated QA judge config (admin). GET → current + available models.
// PUT → enable/disable + pick the judge model.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, boolean_member, nullish_max_string_member, optional_enum_member, parse};
use crate::error::house_error;
use crate::judge::{get_judge_config, set_judge_config};
use crate::model_access::gateway_models;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let config = get_judge_config(&state.pg).await;
    // gatewayModels().catch(() => []) — a gateway read failure is an empty
    // model list, never a 500.
    let models: Vec<String> = gateway_models(&state.pg)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.id)
        .collect();
    Json(serde_json::json!({ "config": config, "models": models })).into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
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
    // z.object({ enabled: z.boolean(), model: z.string().max(200).nullish(),
    //             mode: z.enum(['advisory','enforcing']).optional() }) — keys in
    // schema order, each rejection in zod's own words.
    let enabled = match boolean_member(obj, "enabled") {
        Ok(e) => e,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let model = match nullish_max_string_member(obj, "model", 200) {
        Ok(m) => m,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let mode = match optional_enum_member(obj, "mode", &["advisory", "enforcing"]) {
        Ok(m) => m.unwrap_or_else(|| "enforcing".to_string()), // body.mode ?? 'enforcing'
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // `model?.trim() || null` — a blank string clears the pick.
    let config = serde_json::json!({
        "enabled": enabled,
        "model": model.as_deref().map(str::trim).filter(|m| !m.is_empty()),
        "mode": mode,
    });
    set_judge_config(&state.pg, &config).await;
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "settings.judge",
            target_type: "settings",
            target_id: None,
            target_label: None,
            before: None,
            after: Some(config.clone()),
        },
    )
    .await;
    Json(serde_json::json!({ "config": config })).into_response()
}
