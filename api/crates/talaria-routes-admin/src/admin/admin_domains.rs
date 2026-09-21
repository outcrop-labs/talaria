// /api/admin/domains. Sign-up domains. GET → the list. POST { domain } → add
// (returns the TXT token to publish). POST { verifyId } → run the DNS check.
// DELETE { id } → remove (self-joins from it stop immediately). Admins only.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, zod_uuid_ok};
use talaria_error::{house_error, internal, object_or_400};
use talaria_org_domains::{add_org_domain, list_org_domains, remove_org_domain, verify_org_domain};
use talaria_session::{SessionUser, require_admin};
use talaria_state::AppState;

/// The audit actor: the email, else the name, else 'admin'.
fn domain_actor(user: &SessionUser) -> String {
    user.email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".to_string())
}

pub async fn get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let domains = match list_org_domains(&state.pg).await {
        Ok(d) => d,
        Err(e) => return Ok(internal("[admin/domains] list failed", e)),
    };
    Ok(Json(serde_json::json!({ "domains": domains })).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let actor = domain_actor(&user);
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    Ok(
        // dispatch by key presence; each branch's fields answer their own
        // messages (a short domain is "Too small: ... >=3 characters", a
        // non-uuid verifyId is "Invalid UUID").
        if let Some(v) = obj.get("domain") {
            let domain = match v.as_str() {
                Some(d) => d,
                None => {
                    return Ok(house_error(
                        StatusCode::BAD_REQUEST,
                        &talaria_body::string_msg(talaria_body::zod_type_name(v)),
                    ));
                }
            };
            if talaria_body::utf16_len(domain) < 3 {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    &talaria_body::too_small_msg(3),
                ));
            }
            if talaria_body::utf16_len(domain) > 253 {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    &talaria_body::too_big_msg(253),
                ));
            }
            match add_org_domain(&state.pg, domain, &actor).await {
                Ok(d) => {
                    log_audit(
                        &state.pg,
                        AuditEntry {
                            actor: &actor,
                            action: "domain.add",
                            target_type: "org-domain",
                            target_id: d.get("id").and_then(Value::as_str),
                            target_label: d.get("domain").and_then(Value::as_str),
                            before: None,
                            after: None,
                        },
                    )
                    .await;
                    Json(serde_json::json!({ "domain": d })).into_response()
                }
                Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
            }
        } else if let Some(v) = obj.get("verifyId") {
            let verify_id = match v.as_str() {
                Some(id) if zod_uuid_ok(id) => id,
                Some(_) => return Ok(house_error(StatusCode::BAD_REQUEST, "Invalid UUID")),
                None => {
                    return Ok(house_error(
                        StatusCode::BAD_REQUEST,
                        &talaria_body::string_msg(talaria_body::zod_type_name(v)),
                    ));
                }
            };
            match verify_org_domain(&state.pg, verify_id).await {
                Ok(r) => {
                    if r.get("verified") == Some(&Value::Bool(true)) {
                        log_audit(
                            &state.pg,
                            AuditEntry {
                                actor: &actor,
                                action: "domain.verify",
                                target_type: "org-domain",
                                target_id: Some(verify_id),
                                target_label: None,
                                before: None,
                                after: None,
                            },
                        )
                        .await;
                    }
                    Json(r).into_response()
                }
                Err(e) => internal("[admin/domains] verify failed", e),
            }
        } else {
            // Neither key — the blanket "Invalid input".
            house_error(StatusCode::BAD_REQUEST, "Invalid input")
        },
    )
}

pub async fn delete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // just { id } — a uuid.
    let id = match talaria_body::uuid_member(obj, "id") {
        Ok(i) => i,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(e) = remove_org_domain(&state.pg, &id).await {
        return Ok(internal("[admin/domains] remove failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &talaria_session::actor_of(&user),
            action: "domain.remove",
            target_type: "org-domain",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}
