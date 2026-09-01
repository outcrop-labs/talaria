// /api/fleet/endpoints — port of ui/src/routes/api/fleet.endpoints.ts. The
// model-backend registry (Models tab). GET → all endpoints. POST → add one.

use crate::audit::{log_audit, AuditEntry};
use crate::body::{
    as_object, js_numberify, optional_max_string_member, optional_string_array_member, parse,
    string_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::gateway::provider::migrate_env_keys_to_cipher;
use crate::gateway::registry::{create_endpoint, list_endpoints_wire};
use crate::price_oracle::{kick_auto_prices, maybe_refresh_auto_prices};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let listed = list_endpoints_wire(&state.pg).await;
    maybe_refresh_auto_prices(&state.pg); // background; persisted rates show on the next load
    // one-time: seal any config-only keys into the DB (fire-and-forget)
    tokio::spawn(async move {
        let _ = migrate_env_keys_to_cipher(&state).await;
    });
    match listed {
        Ok(endpoints) => {
            let mut body = json!({ "endpoints": endpoints });
            js_numberify(&mut body);
            Json(body).into_response()
        }
        Err(_) => thrown_internal_error(),
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
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
    let parsed = match validate(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(_) => return thrown_internal_error(),
    };
    match create_endpoint(
        &state.pg,
        &sb,
        &parsed.name,
        &parsed.provider,
        parsed.base_url.as_deref(),
        &parsed.class,
        parsed.api_key_env.as_deref(),
        parsed.api_key.as_deref(),
        &parsed.models,
        &parsed.model_prices,
    )
    .await
    {
        Ok(id) => {
            let actor = actor_of(&user);
            let after = json!({
                "provider": parsed.provider,
                "class": parsed.class,
            });
            let label = parsed.name.clone();
            let pg = state.pg.clone();
            tokio::spawn(async move {
                log_audit(
                    &pg,
                    AuditEntry {
                        actor: &actor,
                        action: "endpoint.create",
                        target_type: "endpoint",
                        target_id: None,
                        target_label: Some(&label),
                        before: None,
                        after: Some(after),
                    },
                )
                .await;
            });
            // Price the new provider's models in the background — never block
            // an interactive save on a fetch to openrouter.ai (15s worst case
            // offline).
            kick_auto_prices(&state.pg);
            Json(json!({ "ok": true, "id": id })).into_response()
        }
        Err(e) => {
            let message = if e.to_string().contains("duplicate") {
                "an endpoint with that name exists".to_string()
            } else {
                e.to_string()
            };
            house_error(StatusCode::BAD_REQUEST, &message)
        }
    }
}

struct Validated {
    name: String,
    provider: String,
    base_url: Option<String>,
    class: String,
    api_key_env: Option<String>,
    api_key: Option<String>,
    models: Vec<String>,
    model_prices: Value,
}

/// The POST body (zod): name 2–60, provider 2–40, baseUrl url-or-null, class
/// enum, apiKeyEnv provider-key-shaped, apiKey raw, models ≤100 ids,
/// modelPrices an {in,out} record.
fn validate(obj: &serde_json::Map<String, Value>) -> Result<Validated, String> {
    let name = string_member(obj, "name", 2, 60)?;
    let provider = string_member(obj, "provider", 2, 40)?;
    // z.string().url().max(300).nullish() — absent and null both map to null.
    let base_url = match obj.get("baseUrl") {
        None | Some(Value::Null) => None,
        Some(_) => Some(crate::body::url_member(obj, "baseUrl", 300)?),
    };
    let class = crate::body::enum_member(obj, "class", &["local", "cloud"])?;
    // Provider-key-shaped names only (see provider-catalog KEY_ENV_RE) — the
    // catalog fetch sends this var's VALUE to the endpoint's base URL.
    let api_key_env = nullish_key_env(obj, "apiKeyEnv")?;
    // Raw provider API key — sealed (secretbox) server-side, never stored or
    // returned in the clear.
    let api_key = optional_max_string_member(obj, "apiKey", 400)?;
    let models = optional_string_array_member(obj, "models", 1, 120, 100)?
        .unwrap_or_default();
    let model_prices = match obj.get("modelPrices") {
        None => json!({}),
        Some(v) => price_record(v, 120)?,
    };
    Ok(Validated {
        name,
        provider,
        base_url,
        class,
        api_key_env,
        api_key,
        models,
        model_prices,
    })
}

/// `z.string().regex(/^(LLM_API_KEY|[A-Z][A-Z0-9_]*_API_KEY)$/) .max(80).nullish()`.
fn nullish_key_env(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| crate::body::string_msg(
                crate::body::zod_type_name(v),
            ))?;
            if !crate::gateway::provider::key_env_allowed(s) {
                return Err(format!(
                    "Invalid string: must match pattern /^(LLM_API_KEY|[A-Z][A-Z0-9_]*_API_KEY)$/"
                ));
            }
            if crate::body::utf16_len(s) > 80 {
                return Err(crate::body::too_big_msg(80));
            }
            Ok(Some(s.to_string()))
        }
    }
}

/// `z.record(z.string().max(k), z.object({ in: nonneg?, out: nonneg? }))` —
/// keys bounded, values an object of two optional non-negative numbers.
/// (shared with the PUT patch in fleet_endpoints_id.rs)
pub(crate) fn price_record(v: &Value, key_max: usize) -> Result<Value, String> {
    let map = v.as_object().ok_or_else(|| crate::body::record_msg("object"))?;
    let mut out = serde_json::Map::new();
    for (k, val) in map {
        if crate::body::utf16_len(k) > key_max {
            return Err(crate::body::too_big_msg(key_max));
        }
        let entry = val
            .as_object()
            .ok_or_else(|| "Invalid input: expected object, received ".to_string()
                + crate::body::zod_type_name(val))?;
        let mut shaped = serde_json::Map::new();
        for field in ["in", "out"] {
            match entry.get(field) {
                None => {}
                Some(Value::Null) => {
                    return Err(format!(
                        "Invalid input: expected number, received null at \"{k}.{field}\""
                    ));
                }
                Some(n @ Value::Number(_)) => {
                    let f = n.as_f64().unwrap_or(0.0);
                    if f < 0.0 {
                        return Err(crate::body::too_small_msg(0));
                    }
                    shaped.insert(field.into(), json!(f));
                }
                Some(other) => {
                    return Err(format!(
                        "Invalid input: expected number, received {}",
                        crate::body::zod_type_name(other)
                    ));
                }
            }
        }
        out.insert(k.clone(), Value::Object(shaped));
    }
    Ok(Value::Object(out))
}
