// /api/admin/instance. The instance's hosting domain and display name.
// GET → both configs. PUT { domain } sets the domain (unverified until the
// round trip passes), { domain: null } clears it; { companyName } sets the
// display name, { companyName: null } clears it — one PUT may carry either
// or both, but not neither. POST { verify: true } runs the self-fetch (the
// action). Admins only.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, literal_true_member, nullable_string_member, parse, present_nullable_string_member,
};
use crate::error::house_error;
use crate::instance::{
    VerifyResult, get_company_name, get_instance_domain, set_company_name, set_instance_domain,
    verify_instance_domain,
};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// The validated PUT body — each field Option<"present">, which is the very
/// thing the at-least-one check counts. `None` (absent) means "don't touch";
/// `Some(None)` is the explicit clear.
#[derive(Debug)]
struct InstancePatch {
    domain: Option<Option<String>>,
    company_name: Option<Option<String>>,
}

/// The PUT body schema, checks in declaration order (domain, companyName —
/// the first bad field's message is the answer), then the at-least-one
/// check. Every message is pinned in the test at the bottom of this file.
fn validate_instance_patch(obj: &serde_json::Map<String, Value>) -> Result<InstancePatch, String> {
    let domain = nullable_or_absent_domain(obj)?;
    let company_name = present_nullable_string_member(obj, "companyName", 80)?;
    if domain.is_none() && company_name.is_none() {
        return Err("nothing to update".into());
    }
    Ok(InstancePatch {
        domain,
        company_name,
    })
}

/// `domain` was a REQUIRED nullable member before companyName joined this
/// route; its error message (min 3, max 253) stays exactly what it was, so
/// the domain panel's refusals read unchanged — the message table pins it.
fn nullable_or_absent_domain(
    obj: &serde_json::Map<String, Value>,
) -> Result<Option<Option<String>>, String> {
    match obj.get("domain") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(_) => nullable_string_member(obj, "domain", 3, 253).map(Some),
    }
}

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    // The raw stored domain config rides the wire — null when unset,
    // {domain, verified, verifiedAt} as stored — beside the display name.
    Json(json!({
        "instance": get_instance_domain(&state.pg).await,
        "companyName": get_company_name(&state.pg).await,
    }))
    .into_response()
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
    let patch = match validate_instance_patch(obj) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // The fields apply in declaration order with no transaction: a body
    // carrying a good domain and a refused name has already set the domain
    // by the time the name's 400 answers (the /api/me PUT's own ordering).
    if let Some(domain) = &patch.domain {
        match set_instance_domain(&state.pg, domain.as_deref()).await {
            Ok(_) => {
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
            }
            // The normalization refusal is its own 400 sentence, verbatim.
            Err(e) => return house_error(StatusCode::BAD_REQUEST, &e),
        }
    }
    if let Some(name) = &patch.company_name {
        match set_company_name(&state.pg, name.as_deref()).await {
            Ok(_) => {
                log_audit(
                    &state.pg,
                    AuditEntry {
                        actor: &actor_of(&user),
                        action: "instance.company_name_set",
                        target_type: "instance",
                        target_id: Some("companyName"),
                        target_label: None,
                        before: None,
                        after: Some(json!({ "companyName": name })),
                    },
                )
                .await;
            }
            Err(e) => return house_error(StatusCode::BAD_REQUEST, &e),
        }
    }
    Json(json!({
        "instance": get_instance_domain(&state.pg).await,
        "companyName": get_company_name(&state.pg).await,
    }))
    .into_response()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn patch(v: Value) -> Result<InstancePatch, String> {
        validate_instance_patch(v.as_object().unwrap())
    }

    #[test]
    fn the_put_body_grammar_is_pinned() {
        // The domain panel's shapes still parse exactly as before.
        let p = patch(json!({ "domain": "talia.example.com" })).unwrap();
        assert_eq!(p.domain, Some(Some("talia.example.com".into())));
        assert_eq!(p.company_name, None);
        assert_eq!(patch(json!({ "domain": null })).unwrap().domain, Some(None));

        // The name rides alone or beside the domain; null clears.
        let p = patch(json!({ "companyName": "Outcrop Labs" })).unwrap();
        assert_eq!(p.company_name, Some(Some("Outcrop Labs".into())));
        assert_eq!(p.domain, None);
        let p = patch(json!({ "domain": "talia.example.com", "companyName": null })).unwrap();
        assert_eq!(p.domain, Some(Some("talia.example.com".into())));
        assert_eq!(p.company_name, Some(None));

        // Neither field is a refusal; a bad value for either is, verbatim.
        assert_eq!(patch(json!({})).unwrap_err(), "nothing to update");
        assert_eq!(
            patch(json!({ "domain": "ab" })).unwrap_err(),
            "Too small: expected string to have >=3 characters"
        );
        assert_eq!(
            patch(json!({ "companyName": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert_eq!(
            patch(json!({ "domain": 5 })).unwrap_err(),
            "Invalid input: expected string, received number"
        );
    }
}
