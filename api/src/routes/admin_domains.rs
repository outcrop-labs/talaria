// /api/admin/domains — port of ui/src/routes/api/admin.domains.ts. Sign-up
// domains. GET → the list. POST { domain } → add (returns the TXT token to
// publish). POST { verifyId } → run the DNS check. DELETE { id } → remove
// (self-joins from it stop immediately). Admins only.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, zod_uuid_ok};
use crate::error::{house_error, thrown_internal_error};
use crate::org_domains::{add_org_domain, list_org_domains, remove_org_domain, verify_org_domain};
use crate::session::{SessionUser, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

/// admin.domains.ts's actor: the email, else the name, else 'admin'.
fn domain_actor(user: &SessionUser) -> String {
    user.email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".to_string())
}

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let domains = match list_org_domains(&state.pg).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[admin/domains] list failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(serde_json::json!({ "domains": domains })).into_response()
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
    let actor = domain_actor(&user);
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // z.union([{ domain: z.string().min(3).max(253) }, { verifyId: Uuid }]) —
    // dispatch by key presence; each branch's fields answer zod's own
    // messages (a short domain is "Too small: ... >=3 characters", a non-uuid
    // verifyId is "Invalid UUID").
    if let Some(v) = obj.get("domain") {
        let domain = match v.as_str() {
            Some(d) => d,
            None => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    &crate::body::string_msg(crate::body::zod_type_name(v)),
                );
            }
        };
        if crate::body::utf16_len(domain) < 3 {
            return house_error(StatusCode::BAD_REQUEST, &crate::body::too_small_msg(3));
        }
        if crate::body::utf16_len(domain) > 253 {
            return house_error(StatusCode::BAD_REQUEST, &crate::body::too_big_msg(253));
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
            Some(_) => return house_error(StatusCode::BAD_REQUEST, "Invalid UUID"),
            None => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    &crate::body::string_msg(crate::body::zod_type_name(v)),
                );
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
            Err(e) => {
                tracing::error!("[admin/domains] verify failed: {e}");
                thrown_internal_error()
            }
        }
    } else {
        // Neither key: both union branches fail — the union's blanket.
        house_error(StatusCode::BAD_REQUEST, "Invalid input")
    }
}

pub async fn delete(
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
    // IdBody = z.object({ id: Uuid })
    let id = match crate::body::uuid_member(obj, "id") {
        Ok(i) => i,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = remove_org_domain(&state.pg, &id).await {
        tracing::error!("[admin/domains] remove failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &crate::session::actor_of(&user),
            action: "domain.remove",
            target_type: "org-domain",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Json(serde_json::json!({ "ok": true })).into_response()
}
