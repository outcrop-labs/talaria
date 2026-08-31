// /api/admin/permissions — port of ui/src/routes/api/admin.permissions.ts.
// Fine-grained permissions admin. GET → the catalog + org member defaults +
// every user's overrides. PUT { userId, perm, allowed|null } → set/clear a
// per-user override (null = back to the org default). PUT { orgDefault:
// { perm, enabled|null } } → tune what plain members can do out of the box
// (null = back to the shipped default). Admins only; both paths audit.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::permissions::{
    PERM_IDS, PERMISSIONS, get_org_default_perms, get_user_perm_overrides, set_org_default_perm,
    set_user_perm_override,
};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    // Every override row, folded per user in ROW order ((overrides[userId]
    // ??= {})[perm] = allowed — insertion order is the wire order).
    let rows: Result<Vec<(String, String, bool)>, sqlx::Error> =
        sqlx::query_as("select user_id::text, perm, allowed from user_permissions")
            .fetch_all(&state.pg)
            .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[admin/permissions] overrides read failed: {e}");
            return thrown_internal_error();
        }
    };
    let mut overrides = serde_json::Map::new();
    for (user_id, perm, allowed) in rows {
        let entry = overrides
            .entry(user_id)
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Value::Object(o) = entry {
            o.insert(perm, Value::Bool(allowed));
        }
    }
    Json(serde_json::json!({
        "catalog": PERMISSIONS,
        "orgDefaults": get_org_default_perms(&state.pg).await,
        "overrides": Value::Object(overrides),
    }))
    .into_response()
}

/// The PUT body is a z.union of two shapes. Extra keys don't fail a branch
/// (zod strips them), and branch A wins when both parse, because zod tries
/// them in order. When neither parses, zod answers "Invalid input" — with one
/// probed exception (zod 4.3.6): the union returns the sole NON-ABORTED
/// branch's own issues, and the only non-aborting failure this schema can
/// produce is the uuid FORMAT check on a string userId (type errors and enum
/// misses abort their branch; branch B holds nothing but enum/boolean checks,
/// so it is never the survivor). That one shape answers "Invalid UUID".
fn parse_union_body(obj: &serde_json::Map<String, Value>) -> Result<UnionBody, String> {
    // Branch A: { userId: uuid, perm: enum, allowed: bool|null }
    let a = (
        obj.get("userId").and_then(Value::as_str),
        obj.get("perm").and_then(Value::as_str),
        match obj.get("allowed") {
            Some(Value::Bool(b)) => Some(Some(*b)),
            Some(Value::Null) => Some(None),
            _ => None,
        },
    );
    if let (Some(user_id), Some(perm), Some(allowed)) = a
        && crate::body::zod_uuid_ok(user_id)
        && PERM_IDS.contains(&perm)
    {
        return Ok(UnionBody::UserOverride {
            user_id: user_id.to_string(),
            perm: perm.to_string(),
            allowed,
        });
    }
    // Branch B: { orgDefault: { perm: enum, enabled: bool|null } }
    if let Some(inner) = obj.get("orgDefault").and_then(Value::as_object) {
        let b = (
            inner.get("perm").and_then(Value::as_str),
            match inner.get("enabled") {
                Some(Value::Bool(v)) => Some(Some(*v)),
                Some(Value::Null) => Some(None),
                _ => None,
            },
        );
        if let (Some(perm), Some(enabled)) = b
            && PERM_IDS.contains(&perm)
        {
            return Ok(UnionBody::OrgDefault {
                perm: perm.to_string(),
                enabled,
            });
        }
    }
    // Branch A near-miss: string userId that fails the uuid format while perm
    // and allowed are individually fine — the sole non-aborted branch, so its
    // message rides the 400. Checked AFTER branch B: a matching B wins.
    if let (Some(user_id), Some(perm), Some(_)) = a
        && PERM_IDS.contains(&perm)
        && !crate::body::zod_uuid_ok(user_id)
    {
        return Err("Invalid UUID".into());
    }
    Err("Invalid input".into())
}

#[derive(Debug)]
enum UnionBody {
    UserOverride {
        user_id: String,
        perm: String,
        allowed: Option<bool>,
    },
    OrgDefault {
        perm: String,
        enabled: Option<bool>,
    },
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
    let actor = actor_of(&user);
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(_) => return house_error(StatusCode::BAD_REQUEST, "Invalid input"),
    };
    let body = match parse_union_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    match body {
        UnionBody::OrgDefault { perm, enabled } => {
            if let Err(e) = set_org_default_perm(&state.pg, &perm, enabled).await {
                tracing::error!("[admin/permissions] org default write failed: {e}");
                return thrown_internal_error();
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "permissions.org_default",
                    target_type: "permission",
                    target_id: Some(&perm),
                    target_label: None,
                    before: None,
                    // { enabled } — null KEPT as explicit null.
                    after: Some(serde_json::json!({ "enabled": enabled })),
                },
            )
            .await;
            Json(serde_json::json!({
                "orgDefaults": get_org_default_perms(&state.pg).await,
            }))
            .into_response()
        }
        UnionBody::UserOverride {
            user_id,
            perm,
            allowed,
        } => {
            if let Err(e) = set_user_perm_override(&state.pg, &user_id, &perm, allowed).await {
                tracing::error!("[admin/permissions] override write failed: {e}");
                return thrown_internal_error();
            }
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: "permissions.user_override",
                    target_type: "user",
                    target_id: Some(&user_id),
                    target_label: None,
                    before: None,
                    after: Some(serde_json::json!({ "perm": perm, "allowed": allowed })),
                },
            )
            .await;
            let overrides = match get_user_perm_overrides(&state.pg, &user_id).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[admin/permissions] overrides read failed: {e}");
                    return thrown_internal_error();
                }
            };
            Json(serde_json::json!({ "overrides": overrides })).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_union_body;
    use serde_json::json;

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().cloned().unwrap()
    }

    // The probed zod 4.3.6 table for this route's z.union — every row is an
    // observed message, not a guess. The one non-generic row is branch A's
    // uuid format near-miss (the union's sole-non-aborted-branch return).
    #[test]
    fn union_table_matches_zod() {
        let uuid = "d3327760-cdd8-41db-9099-4410ea14043b";
        // Both branches match → A wins (zod tries them in order).
        let both = parse_union_body(&obj(json!({
            "userId": uuid, "perm": "research.run", "allowed": true,
            "orgDefault": { "perm": "research.run", "enabled": true },
        })))
        .unwrap();
        assert!(matches!(both, super::UnionBody::UserOverride { .. }));
        // A alone, B alone.
        assert!(
            parse_union_body(&obj(json!({
                "userId": uuid, "perm": "research.run", "allowed": null,
            })))
            .is_ok()
        );
        assert!(
            parse_union_body(&obj(json!({
                "orgDefault": { "perm": "research.run", "enabled": false },
            })))
            .is_ok()
        );
        // A near-misses on the uuid FORMAT alone → that message rides.
        assert_eq!(
            parse_union_body(&obj(json!({
                "userId": "nope", "perm": "research.run", "allowed": true,
            })))
            .unwrap_err(),
            "Invalid UUID"
        );
        // …even when branch B fails too, however it fails — B holds only
        // abort-class checks, so A stays the sole non-aborted branch.
        assert_eq!(
            parse_union_body(&obj(json!({
                "userId": "nope", "perm": "research.run", "allowed": true,
                "orgDefault": { "perm": "zzz", "enabled": true },
            })))
            .unwrap_err(),
            "Invalid UUID"
        );
        // …but a MATCHING B beats a failing A.
        assert!(
            parse_union_body(&obj(json!({
                "userId": "nope",
                "orgDefault": { "perm": "research.run", "enabled": true },
            })))
            .is_ok()
        );
        // Any second bad member in A aborts it → the generic union message.
        for bad in [
            json!({ "userId": "nope", "perm": "zzz", "allowed": true }),
            json!({ "userId": "nope", "perm": "research.run", "allowed": "x" }),
            json!({ "userId": "nope", "perm": "research.run" }),
            json!({ "userId": 5, "perm": "research.run", "allowed": true }),
            json!({ "userId": uuid, "perm": "zzz", "allowed": true }),
            json!({ "orgDefault": { "perm": "zzz", "enabled": true } }),
            json!({ "what": "nope" }),
        ] {
            assert_eq!(
                parse_union_body(&obj(bad.clone())).unwrap_err(),
                "Invalid input",
                "case {bad}"
            );
        }
    }
}
