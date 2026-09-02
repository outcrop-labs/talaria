// /api/admin/instance. The instance's hosting domain. GET → current config.
// PUT { domain } → set (unverified until the round trip passes); { domain:
// null } clears. POST { verify: true } → run the self-fetch (the action).
// Admins only.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, literal_true_member, nullable_string_member, parse};
use crate::error::house_error;
use crate::instance::{
    VerifyResult, get_instance_domain, set_instance_domain, verify_instance_domain,
};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    // The raw stored config rides the wire — null when unset,
    // {domain, verified, verifiedAt} as stored.
    Json(json!({ "instance": get_instance_domain(&state.pg).await })).into_response()
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
    let domain = match nullable_string_member(obj, "domain", 3, 253) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match set_instance_domain(&state.pg, domain.as_deref()).await {
        Ok(instance) => {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor_of(&user),
                    action: "instance.domain_set",
                    target_type: "instance",
                    target_id: Some("domain"),
                    target_label: None,
                    before: None,
                    // The RAW input, pre-normalization — what the admin sent.
                    after: Some(json!({ "domain": domain })),
                },
            )
            .await;
            Json(json!({ "instance": instance })).into_response()
        }
        // The normalization refusal is its own 400 sentence, verbatim.
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn post(
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
    if let Err(msg) = literal_true_member(obj, "verify") {
        return house_error(StatusCode::BAD_REQUEST, &msg);
    }
    let r: VerifyResult = verify_instance_domain(&state.pg).await;
    if r.verified {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "instance.domain_verify",
                target_type: "instance",
                target_id: Some("domain"),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
    }
    Json(r).into_response()
}
