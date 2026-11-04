// doc: /api/cost/prices. An admin's pricing decision for one (endpoint, model)
// doc: no published price covers. PUT `mode: "manual"` sets a rate per 1M
// doc: input / output tokens (cache-write and cache-read optional; the input
// doc: rate times 1.25 / 0.1 when absent); PUT `mode: "included"` marks it
// doc: included in a plan or free — priced at $0 and labelled `included`, never
// doc: a silent zero. DELETE removes the decision (404 when there is none).
// doc: The endpoint is the ledger's own text, "" for usage booked without one,
// doc: so a deleted endpoint's history can still be priced. Pricing is
// doc: read-time: every past row of the model re-prices at once, and each
// doc: answer says how (`repriced`). Precedence: the provider's own charge,
// doc: then included, then a published price, then a manual rate.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::gateway::usage::AdminPricing;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    NumKind, enum_member, nullable_number_member, nullish_max_string_member, parse, string_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_price_oracle::unpriced::{
    PriceInput, clear_price, repriced, set_price, validate_price,
};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

/// The (endpoint, model) a decision is about — both bodies carry it. The
/// endpoint is the ledger's text, "" allowed (rows booked without one); a
/// model id can be a long path (`accounts/fireworks/models/…`).
fn target(obj: &serde_json::Map<String, serde_json::Value>) -> Result<(String, String), String> {
    let endpoint = string_member(obj, "endpoint", 0, 200).map_err(|m| format!("endpoint: {m}"))?;
    let model = string_member(obj, "model", 1, 300).map_err(|m| format!("model: {m}"))?;
    Ok((endpoint, model))
}

fn parse_input(obj: &serde_json::Map<String, serde_json::Value>) -> Result<PriceInput, String> {
    let (endpoint, model) = target(obj)?;
    let mode = match enum_member(obj, "mode", &["manual", "included"])
        .map_err(|m| format!("mode: {m}"))?
        .as_str()
    {
        "manual" => AdminPricing::Manual,
        _ => AdminPricing::Included,
    };
    // Dollars per 1M tokens, 0 to 100000 (MAX_RATE_PER_MTOK) — null or absent
    // is "not given". Spelled out per key so the API reference reads them.
    let named = |key: &str, r: Result<Option<f64>, String>| r.map_err(|m| format!("{key}: {m}"));
    let input = PriceInput {
        endpoint,
        model,
        mode,
        price_in_per_mtok: named(
            "priceInPerMtok",
            nullable_number_member(obj, "priceInPerMtok", NumKind::Float, 0.0, 100000.0),
        )?,
        price_out_per_mtok: named(
            "priceOutPerMtok",
            nullable_number_member(obj, "priceOutPerMtok", NumKind::Float, 0.0, 100000.0),
        )?,
        price_cache_write_per_mtok: named(
            "priceCacheWritePerMtok",
            nullable_number_member(obj, "priceCacheWritePerMtok", NumKind::Float, 0.0, 100000.0),
        )?,
        price_cache_read_per_mtok: named(
            "priceCacheReadPerMtok",
            nullable_number_member(obj, "priceCacheReadPerMtok", NumKind::Float, 0.0, 100000.0),
        )?,
        note: nullish_max_string_member(obj, "note", 200).map_err(|m| format!("note: {m}"))?,
    };
    validate_price(&input)?;
    Ok(input)
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let input = match parse_input(obj) {
        Ok(i) => i,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let actor = actor_of(&user);
    if let Err(e) = set_price(&state.pg, &input, &actor).await {
        return Ok(internal("[cost] price save failed", e));
    }
    let after = json!({
        "endpoint": input.endpoint,
        "model": input.model,
        "mode": if input.mode == AdminPricing::Manual { "manual" } else { "included" },
        "priceInPerMtok": input.price_in_per_mtok,
        "priceOutPerMtok": input.price_out_per_mtok,
        "priceCacheWritePerMtok": input.price_cache_write_per_mtok,
        "priceCacheReadPerMtok": input.price_cache_read_per_mtok,
        "note": input.note,
    });
    let label = format!(
        "{} · {}",
        if input.endpoint.is_empty() {
            "(no endpoint)"
        } else {
            &input.endpoint
        },
        input.model
    );
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "price.set",
            target_type: "model_price",
            target_id: None,
            target_label: Some(&label),
            before: None,
            after: Some(after.clone()),
        },
    )
    .await;
    match repriced(&state.pg, &input.endpoint, &input.model).await {
        Ok(r) => Ok(Json(json!({ "price": after, "repriced": r })).into_response()),
        Err(e) => Ok(internal("[cost] repriced read failed", e)),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let (endpoint, model) = match target(obj) {
        Ok(t) => t,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match clear_price(&state.pg, &endpoint, &model).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[cost] price remove failed", e)),
    }
    let actor = actor_of(&user);
    let label = format!(
        "{} · {model}",
        if endpoint.is_empty() {
            "(no endpoint)"
        } else {
            &endpoint
        }
    );
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "price.clear",
            target_type: "model_price",
            target_id: None,
            target_label: Some(&label),
            before: None,
            after: None,
        },
    )
    .await;
    match repriced(&state.pg, &endpoint, &model).await {
        Ok(r) => Ok(Json(json!({ "ok": true, "repriced": r })).into_response()),
        Err(e) => Ok(internal("[cost] repriced read failed", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn the_literal_rate_bound_is_the_oracle_ceiling() {
        // parse_input spells 100000 per key so the API reference reads it.
        assert!(
            (talaria_price_oracle::unpriced::MAX_RATE_PER_MTOK - 100000.0).abs() < f64::EPSILON
        );
    }

    #[test]
    fn a_manual_body_parses_every_rate_and_the_note() {
        let p = parse_input(&obj(json!({
            "endpoint": "openai", "model": "gpt-4.1-2025-04-14", "mode": "manual",
            "priceInPerMtok": 2, "priceOutPerMtok": 8, "priceCacheReadPerMtok": 0.5,
            "priceCacheWritePerMtok": null, "note": "from the OpenAI pricing page"
        })))
        .unwrap();
        assert_eq!(p.mode, AdminPricing::Manual);
        assert_eq!(
            (p.price_in_per_mtok, p.price_out_per_mtok),
            (Some(2.0), Some(8.0))
        );
        assert_eq!(p.price_cache_read_per_mtok, Some(0.5));
        assert_eq!(p.price_cache_write_per_mtok, None);
        assert_eq!(p.note.as_deref(), Some("from the OpenAI pricing page"));
    }

    #[test]
    fn an_included_body_needs_only_the_target_and_refuses_rates() {
        let p = parse_input(&obj(
            json!({ "endpoint": "zai-plan", "model": "glm-4.6", "mode": "included" }),
        ))
        .unwrap();
        assert_eq!(p.mode, AdminPricing::Included);
        let err = parse_input(&obj(json!({
            "endpoint": "zai-plan", "model": "glm-4.6", "mode": "included", "priceInPerMtok": 1
        })))
        .unwrap_err();
        assert!(err.contains("$0"), "{err}");
    }

    #[test]
    fn the_first_wrong_field_is_the_sentence() {
        let err = parse_input(&obj(json!({ "model": "m", "mode": "manual" }))).unwrap_err();
        assert!(err.starts_with("endpoint: "), "{err}");
        let err = parse_input(&obj(
            json!({ "endpoint": "e", "model": "", "mode": "manual" }),
        ))
        .unwrap_err();
        assert!(err.starts_with("model: "), "{err}");
        let err = parse_input(&obj(
            json!({ "endpoint": "e", "model": "m", "mode": "free" }),
        ))
        .unwrap_err();
        assert!(err.starts_with("mode: "), "{err}");
        let err = parse_input(&obj(json!({
            "endpoint": "e", "model": "m", "mode": "manual", "priceInPerMtok": -1, "priceOutPerMtok": 1
        })))
        .unwrap_err();
        assert!(err.starts_with("priceInPerMtok: "), "{err}");
        let err = parse_input(&obj(
            json!({ "endpoint": "e", "model": "m", "mode": "manual", "priceInPerMtok": 1 }),
        ))
        .unwrap_err();
        assert!(err.contains("priceOutPerMtok"), "{err}");
        // "" is the ledger's value for rows booked without an endpoint.
        assert!(
            parse_input(&obj(
                json!({ "endpoint": "", "model": "m", "mode": "included" })
            ))
            .is_ok()
        );
    }
}
