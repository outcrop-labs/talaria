// /api/admin/app-harness-models. The model each app harness runs on — the
// setting an admin assigns on the harness's column in Admin → Models →
// Fitness. GET → the assignments and the app harness slots that can take one.
// PUT { slot, model|null } → assign (null = back to the harness's own chain).
// Admins only.
//
// A SLOT IS `app:<slug>/<harnessId>` (`ModelSpec.slot`), and the setting it
// lives in, `app_harness_models`, is read by both resolvers: the app host's
// bound runner (ui/src/server/harness/model.ts, the 'slot' step) and
// talaria-harness-model. Only a harness an enabled app registers can be
// assigned — the app host is asked, as the fitness matrix asks it — while a
// stale assignment can always be cleared.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use talaria_api_facades::model::access::gateway_models;
use talaria_app_harnesses::{app_harness_slot, app_layer};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{nullish_member, optional_max_string_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_harness_model::{get_app_harness_models, set_app_harness_model};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

/// Every app harness slot the app host lists right now, with its labels.
async fn registered_slots(state: &AppState) -> Result<Vec<Value>, String> {
    let token = state.cfg.secret_root.internal_token();
    let layer = app_layer(&token).await?;
    Ok(layer
        .harnesses
        .iter()
        .filter_map(|h| {
            let slug = h.app_slug()?;
            Some(serde_json::json!({
                "slot": app_harness_slot(slug, h.def.id),
                "app": slug,
                "harness": h.def.id,
                "label": h.def.label,
            }))
        })
        .collect())
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let assignments = match get_app_harness_models(&state.pg).await {
        Value::Object(o) => o,
        _ => serde_json::Map::new(),
    };
    // The app host down is not an error page: the assignments still read, the
    // list of what can take one is empty and says why.
    let (slots, slots_error) = match registered_slots(&state).await {
        Ok(s) => (s, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    Ok(Json(serde_json::json!({
        "assignments": Value::Object(assignments),
        "slots": slots,
        "slotsError": slots_error,
    }))
    .into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let actor = actor_of(&user);
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let slot = match obj.get("slot").and_then(Value::as_str).map(str::trim) {
        Some(s) if s.starts_with("app:") && s.len() <= 200 => s.to_string(),
        _ => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "slot must be an app harness's slot, app:<slug>/<harnessId>",
            ));
        }
    };
    let model = match nullish_member(obj, "model", |o, k| optional_max_string_member(o, k, 200)) {
        Ok(Some(m)) => m.filter(|m| !m.is_empty()),
        Ok(None) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "model is required: a model id, or null for the harness's own chain",
            ));
        }
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // ASSIGNING needs a harness an enabled app registers and a model the
    // gateway serves. CLEARING needs neither: an app since disabled or
    // uninstalled must not leave an assignment nobody can remove.
    if let Some(m) = &model {
        let slots = match registered_slots(&state).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(house_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &format!(
                        "the app host could not list its harnesses, so the slot could not be checked: {e}"
                    ),
                ));
            }
        };
        if !slots
            .iter()
            .any(|s| s.get("slot").and_then(Value::as_str) == Some(slot.as_str()))
        {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!("\"{slot}\" is not a harness an enabled app registers"),
            ));
        }
        let on_gateway = match gateway_models(&state.pg).await {
            Ok(models) => models.iter().any(|g| &g.id == m),
            Err(e) => {
                return Ok(internal(
                    "[admin/app-harness-models] gateway read failed",
                    e,
                ));
            }
        };
        if !on_gateway {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "that model is not on the gateway",
            ));
        }
    }

    if let Err(e) = set_app_harness_model(&state.pg, &slot, model.as_deref()).await {
        return Ok(internal("[admin/app-harness-models] write failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "app_harness.assign",
            target_type: "app-harness",
            target_id: Some(&slot),
            target_label: None,
            before: None,
            after: Some(serde_json::json!({
                "model": model.clone().map(Value::String).unwrap_or(Value::Null),
            })),
        },
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}
