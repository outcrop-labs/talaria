// /api/fleet/endpoints/{id} — port of ui/src/routes/api/fleet.endpoints.$id.ts.
// PUT → edit an endpoint (class, pricing, model catalog). Removing catalog
// models that agents use returns 409 with the blast radius; retry with
// force:true to cascade (agents get new versions with the tier stripped).
// DELETE → remove the endpoint, same double-opt-in flow (?force=1). An
// agent's MAIN model is never cascaded — reassign it first.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, optional_boolean_member, optional_string_array_member, parse,
    present_nullable_max_string_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::cascade::{ModelUsage, cascade_removal, model_usage};
use crate::gateway::registry::{EndpointPatch, delete_endpoint, list_endpoints, update_endpoint};
use crate::price_oracle::kick_auto_prices;
use crate::routes::fleet::fleet_endpoints::price_record;
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// summarize(usage) — the 409's blast radius, four keys in TS's order.
fn summarize(usage: &[ModelUsage]) -> Vec<Value> {
    usage
        .iter()
        .map(|u| {
            json!({
                "slug": u.slug,
                "asMain": u.as_main,
                "aliases": u.aliases,
                "fallbacks": u.fallbacks,
            })
        })
        .collect()
}

pub async fn put(
    State(state): State<AppState>,
    Path(id): Path<String>,
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
    let patch = match validate_patch(obj) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // One batched cascade: one new version per agent, one render, one restart
    // wave — decided by the CATALOG delta before anything is written.
    let mut cascade: (Vec<String>, Option<String>) = (Vec::new(), None);
    if let Some(models) = &patch.models {
        let eps = match list_endpoints(&state.pg).await {
            Ok(e) => e,
            Err(_) => return thrown_internal_error(),
        };
        let Some(ep) = eps.iter().find(|e| e.id == id) else {
            return house_error(StatusCode::NOT_FOUND, "not found");
        };
        let removed: Vec<String> = ep
            .models
            .iter()
            .filter(|m| !models.contains(m))
            .cloned()
            .collect();
        let usage = if removed.is_empty() {
            Vec::new()
        } else {
            match model_usage(&state.pg, &ep.name, Some(&removed)).await {
                Ok(u) => u,
                Err(_) => return thrown_internal_error(),
            }
        };
        let mains: Vec<&ModelUsage> = usage.iter().filter(|u| u.as_main).collect();
        if !mains.is_empty() {
            // [...new Set(mains.map(m => m.slug))].join(', ')
            let mut seen: Vec<&str> = Vec::new();
            for m in &mains {
                if !seen.contains(&m.slug.as_str()) {
                    seen.push(&m.slug);
                }
            }
            return house_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "main model for: {} — reassign before removing",
                    seen.join(", ")
                ),
            );
        }
        if !usage.is_empty() && !patch.force {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "needsForce": true, "affected": summarize(&usage) })),
            )
                .into_response();
        }
        if !usage.is_empty() {
            let sb = match state.secretbox().await {
                Ok(sb) => sb,
                Err(_) => return thrown_internal_error(),
            };
            let actor = user
                .email
                .clone()
                .or_else(|| user.name.clone())
                .unwrap_or_else(|| "admin".into());
            match cascade_removal(&state.pg, &sb, &ep.name, Some(&removed), &actor).await {
                Ok(r) => cascade = (r.changed, r.render_error),
                Err(_) => return thrown_internal_error(),
            }
        }
    }

    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(_) => return thrown_internal_error(),
    };
    if update_endpoint(&state.pg, &sb, &id, &patch.endpoint)
        .await
        .is_err()
    {
        return thrown_internal_error();
    }
    let actor = actor_of(&user);
    let mut after = serde_json::Map::new();
    if patch.rotates_key {
        after.insert("apiKeyRotated".into(), json!(true));
    }
    if let Some(models) = &patch.models {
        after.insert("models".into(), json!(models.len()));
    }
    let after = Value::Object(after);
    let after_copy = after.clone();
    let id_for_audit = id.clone();
    let pg = state.pg.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "endpoint.update",
                target_type: "endpoint",
                target_id: Some(&id_for_audit),
                target_label: None,
                before: None,
                after: Some(after_copy),
            },
        )
        .await;
    });
    // New catalog models get auto-priced in the background (never block an
    // interactive save on the external catalog fetch).
    if patch.models.is_some() {
        kick_auto_prices(&state.pg);
    }
    Json(cascade_body(true, &cascade)).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let force = uri
        .query()
        .map(|q| q.split('&').any(|pair| pair == "force=1"))
        .unwrap_or(false);
    let eps = match list_endpoints(&state.pg).await {
        Ok(e) => e,
        Err(_) => return thrown_internal_error(),
    };
    let Some(ep) = eps.iter().find(|e| e.id == id) else {
        // Unknown id → ok:true, no body fields beyond it.
        return Json(json!({ "ok": true })).into_response();
    };

    let usage = match model_usage(&state.pg, &ep.name, None).await {
        Ok(u) => u,
        Err(_) => return thrown_internal_error(),
    };
    let mains: Vec<&ModelUsage> = usage.iter().filter(|u| u.as_main).collect();
    if !mains.is_empty() {
        let slugs: Vec<&str> = mains.iter().map(|m| m.slug.as_str()).collect();
        return house_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "main model for: {} — reassign before deleting",
                slugs.join(", ")
            ),
        );
    }
    if !usage.is_empty() && !force {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "needsForce": true, "affected": summarize(&usage) })),
        )
            .into_response();
    }
    let mut cascade: (Vec<String>, Option<String>) = (Vec::new(), None);
    if !usage.is_empty() {
        let sb = match state.secretbox().await {
            Ok(sb) => sb,
            Err(_) => return thrown_internal_error(),
        };
        let actor = user
            .email
            .clone()
            .or_else(|| user.name.clone())
            .unwrap_or_else(|| "admin".into());
        match cascade_removal(&state.pg, &sb, &ep.name, None, &actor).await {
            Ok(r) => cascade = (r.changed, r.render_error),
            Err(_) => return thrown_internal_error(),
        }
    }
    let deleted = match delete_endpoint(&state.pg, &id).await {
        Ok(d) => d,
        Err(_) => return thrown_internal_error(),
    };
    if !deleted.0 {
        return house_error(
            StatusCode::BAD_REQUEST,
            &format!("still in use by: {}", deleted.1.join(", ")),
        );
    }
    let actor = actor_of(&user);
    let label = ep.name.clone();
    let id_for_audit = id.clone();
    let pg = state.pg.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "endpoint.delete",
                target_type: "endpoint",
                target_id: Some(&id_for_audit),
                target_label: Some(&label),
                before: None,
                after: None,
            },
        )
        .await;
    });
    Json(cascade_body(true, &cascade)).into_response()
}

/// `{ok, cascaded, ...(renderError ? {error} : {})}` — key order from the TS
/// spread.
fn cascade_body(ok: bool, cascade: &(Vec<String>, Option<String>)) -> Value {
    let mut body = json!({
        "ok": ok,
        "cascaded": cascade.0,
    });
    if let Some(render_error) = &cascade.1 {
        body["error"] = json!(format!(
            "agents reconfigured but re-render failed ({render_error}) — fix and re-render from /agents"
        ));
    }
    body
}

struct ValidatedPatch {
    endpoint: EndpointPatch,
    models: Option<Vec<String>>,
    force: bool,
    /// Whether the patch rotates the stored key (drives the audit's
    /// apiKeyRotated).
    rotates_key: bool,
}

/// The PUT patch (zod): every member optional; priceIn/Out nonneg nullish;
/// modelPrices/modelEfforts records; requestDefaults a permissive record;
/// apiKey raw ('' clears, omitted leaves); force the double opt-in.
fn validate_patch(obj: &serde_json::Map<String, Value>) -> Result<ValidatedPatch, String> {
    let class = crate::body::optional_enum_member(obj, "class", &["local", "cloud"])?;
    let price_in = nullish_nonneg(obj, "priceInPerMtok")?;
    let price_out = nullish_nonneg(obj, "priceOutPerMtok")?;
    let models = optional_string_array_member(obj, "models", 1, 120, 100)?;
    let model_prices = match obj.get("modelPrices") {
        None => None,
        Some(v) => Some(price_record(v, 120)?),
    };
    // Admin-declared effort ladders for models whose catalog publishes none
    // (or publishes wrong ones). Levels are the provider's own spellings,
    // sent verbatim — the picker must never rename a level into one the model
    // rejects, so no enum here.
    let model_efforts = match obj.get("modelEfforts") {
        None => None,
        Some(v) => Some(effort_record(v, 120)?),
    };
    // Extra request-body defaults for the LLM gateway (deep-merged under the
    // client body). Admin-only, so a permissive record is acceptable here.
    let request_defaults = match obj.get("requestDefaults") {
        None => None,
        Some(Value::Object(m)) => Some(Value::Object(m.clone())),
        Some(v) => {
            return Err(crate::body::record_msg(crate::body::zod_type_name(v)));
        }
    };
    // Empty string = an untouched masked field round-tripping — keep the
    // stored key. Only a non-empty value rotates it. A present NULL round-
    // trips the same way (null?.trim() is undefined, falsy): this route can
    // only ROTATE a key, never clear one.
    let api_key_raw = present_nullable_max_string_member(obj, "apiKey", 400)?;
    let rotates = api_key_raw
        .as_ref()
        .and_then(|o| o.as_deref())
        .is_some_and(|k| !k.trim().is_empty());
    let api_key = if rotates { api_key_raw.flatten() } else { None };
    let rotates_key = rotates;
    let force = optional_boolean_member(obj, "force")?.unwrap_or(false);
    Ok(ValidatedPatch {
        endpoint: EndpointPatch {
            class,
            price_in_per_mtok: price_in,
            price_out_per_mtok: price_out,
            models: models.clone(),
            model_prices,
            model_efforts,
            request_defaults,
            api_key,
        },
        models,
        force,
        rotates_key,
    })
}

/// `z.number().nonnegative().nullish()` — tri-state: absent = don't touch,
/// null = clear, number = set.
fn nullish_nonneg(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<Option<f64>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v @ Value::Number(_)) => {
            let f = v.as_f64().unwrap_or(0.0);
            if f < 0.0 {
                return Err(crate::body::too_small_msg(0));
            }
            Ok(Some(Some(f)))
        }
        Some(v) => Err(format!(
            "Invalid input: expected number, received {}",
            crate::body::zod_type_name(v)
        )),
    }
}

/// `z.record(z.string().max(k), z.array(z.string().min(1).max(24)).min(1).max(12))`.
fn effort_record(v: &Value, key_max: usize) -> Result<Value, String> {
    let map = v
        .as_object()
        .ok_or_else(|| crate::body::record_msg("object"))?;
    let mut out = serde_json::Map::new();
    for (k, val) in map {
        if crate::body::utf16_len(k) > key_max {
            return Err(crate::body::too_big_msg(key_max));
        }
        let arr = val.as_array().ok_or_else(|| {
            format!(
                "Invalid input: expected array, received {}",
                crate::body::zod_type_name(val)
            )
        })?;
        if arr.is_empty() {
            return Err(crate::body::array_too_small_msg(1));
        }
        if arr.len() > 12 {
            return Err(crate::body::array_too_big_msg(12));
        }
        let mut ladder = Vec::with_capacity(arr.len());
        for el in arr {
            let s = el
                .as_str()
                .ok_or_else(|| crate::body::string_msg(crate::body::zod_type_name(el)))?;
            let n = crate::body::utf16_len(s);
            if n < 1 {
                return Err(crate::body::too_small_msg(1));
            }
            if n > 24 {
                return Err(crate::body::too_big_msg(24));
            }
            ladder.push(s.to_string());
        }
        out.insert(k.clone(), json!(ladder));
    }
    Ok(Value::Object(out))
}
