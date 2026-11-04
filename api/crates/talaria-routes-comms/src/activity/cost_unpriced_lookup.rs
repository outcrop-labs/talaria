// doc: POST /api/cost/unpriced/lookup. Retry the published-price lookup for one
// doc: (endpoint, model) now, and wait for it: OpenRouter's public catalog,
// doc: plus that model's per-endpoint prices on an OpenRouter endpoint. Answers
// doc: whether the model is priced now and, when it is not, the reason the
// doc: lookup recorded (the source's own words when it did not answer), with
// doc: what the window's past rows price at (`repriced`). 404 when no cloud
// doc: endpoint carries that name — a deleted endpoint's history is priced by
// doc: hand instead (PUT /api/cost/prices).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_price_oracle::lookup_now;
use talaria_price_oracle::unpriced::{reason_now, repriced};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let endpoint = match string_member(obj, "endpoint", 1, 200) {
        Ok(e) => e,
        Err(m) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!("endpoint: {m}"),
            ));
        }
    };
    let model = match string_member(obj, "model", 1, 300) {
        Ok(m) => m,
        Err(m) => return Ok(house_error(StatusCode::BAD_REQUEST, &format!("model: {m}"))),
    };
    if let Err(msg) = lookup_now(&state.pg, &endpoint, &model).await {
        return Ok(house_error(StatusCode::NOT_FOUND, &msg));
    }
    // The lookup writes the oracle's published-price rows for this model;
    // the trail says who asked for it and what it concluded.
    let label = format!("{endpoint} · {model}");
    let (reason, after) = tokio::join!(
        reason_now(&state.pg, &endpoint, &model),
        repriced(&state.pg, &endpoint, &model)
    );
    match (reason, after) {
        (Ok(reason), Ok(after)) => {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor_of(&user),
                    action: "price.lookup",
                    target_type: "model_price",
                    target_id: None,
                    target_label: Some(&label),
                    before: None,
                    after: Some(json!({
                        "priced": reason.is_none(),
                        "reason": reason.as_ref().map(|r| r.code),
                        "basis": after.basis,
                    })),
                },
            )
            .await;
            Ok(Json(json!({
                "priced": reason.is_none(),
                "reason": reason,
                "repriced": after,
            }))
            .into_response())
        }
        (Err(e), _) | (_, Err(e)) => Ok(internal("[cost] lookup read-back failed", e)),
    }
}
