// /api/admin/judge. The automated QA judge config (admin). GET → current +
// available models. PUT → enable/disable + pick the judge model.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use talaria_api_facades::model::access::gateway_models;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{boolean_member, nullish_max_string_member, optional_enum_member, parse};
use talaria_error::{house_error, object_or_400};
use talaria_judge::{get_judge_config, set_judge_config};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let config = get_judge_config(&state.pg).await;
    // A gateway read failure is an empty model list, never a 500.
    let models: Vec<String> = gateway_models(&state.pg)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.id)
        .collect();
    Ok(Json(serde_json::json!({ "config": config, "models": models })).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Keys in schema order, each rejection in the schema's own words:
    // enabled (bool), model (string max 200, nullish), mode (enum
    // advisory|enforcing, optional).
    let enabled = match boolean_member(obj, "enabled") {
        Ok(e) => e,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let model = match nullish_max_string_member(obj, "model", 200) {
        Ok(m) => m,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mode = match optional_enum_member(obj, "mode", &["advisory", "enforcing"]) {
        Ok(m) => m.unwrap_or_else(|| "enforcing".to_string()), // mode defaults to 'enforcing'
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // a blank model string clears the pick.
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
    Ok(Json(serde_json::json!({ "config": config })).into_response())
}
