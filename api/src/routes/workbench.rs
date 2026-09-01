// /api/workbench — port of ui/src/routes/api/workbench.ts. Workbench
// profiles: the role-agnostic sandbox registry ('dev' seeded; designer/data
// ride the same table). GET → any member (env values masked — they are the
// documented home for scoped credentials); PUT → agents.manage, except the
// infrastructure fields, which are admin-only.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, object_msg, optional_boolean_member, optional_max_string_member,
    optional_string_array_member, optional_string_member, parse, record_msg, string_member,
    too_big_msg, utf16_len, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::users::has_perm;
use crate::workbench::{
    AutoAttach, ProfilePatch, list_profiles, mount_error, profile_wire, update_profile,
};

/// The fields that render straight into the fleet's compose services.
const INFRA_FIELDS: [&str; 2] = ["image", "mounts"];

/// `z.record(z.string(), z.string().max(500)).optional()` — any keys, values
/// capped. The value Map preserves the body's key order, which is the stored
/// and answered order.
fn optional_env_member(
    obj: &Map<String, Value>,
    key: &str,
    value_max: usize,
) -> Result<Option<Map<String, Value>>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Object(m)) => {
            for v in m.values() {
                let Some(s) = v.as_str() else {
                    return Err(format!(
                        "Invalid input: expected string, received {}",
                        zod_type_name(v)
                    ));
                };
                if utf16_len(s) > value_max {
                    return Err(too_big_msg(value_max));
                }
            }
            Ok(Some(m.clone()))
        }
        Some(other) => Err(record_msg(zod_type_name(other))),
    }
}

/// `z.object({ departments: …, roles: … }).optional()` — both members
/// optional string arrays (≤60 each, ≤20 items).
fn optional_auto_attach_member(
    obj: &Map<String, Value>,
    key: &str,
) -> Result<Option<AutoAttach>, String> {
    let Some(v) = obj.get(key) else {
        return Ok(None);
    };
    let Value::Object(m) = v else {
        return Err(object_msg(zod_type_name(v)));
    };
    Ok(Some(AutoAttach {
        departments: optional_string_array_member(m, "departments", 0, 60, 20)?.unwrap_or_default(),
        roles: optional_string_array_member(m, "roles", 0, 60, 20)?.unwrap_or_default(),
    }))
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let profiles = match list_profiles(&state.pg).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[workbench] profile read failed: {e}");
            return thrown_internal_error();
        }
    };
    if has_perm(&state.pg, &user.id, &user.role, "agents.manage")
        .await
        .unwrap_or(false)
    {
        return Json(json!({
            "profiles": profiles.iter().map(|p| profile_wire(p, false)).collect::<Vec<_>>(),
        }))
        .into_response();
    }
    // A profile's env is injected straight into agent containers and is the
    // documented home for scoped credentials, so its VALUES are not
    // member-readable. Keys stay so the attachment UI can still explain
    // itself.
    Json(json!({
        "profiles": profiles.iter().map(|p| profile_wire(p, true)).collect::<Vec<_>>(),
    }))
    .into_response()
}

pub async fn put(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let mut user = match crate::session::require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Patch — schema order (the audit trail's `after` rides it).
    let slug = match string_member(obj, "slug", 1, 40) {
        Ok(s) => s,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match optional_string_member(obj, "name", 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let description = match optional_max_string_member(obj, "description", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let env = match optional_env_member(obj, "env", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let harnesses = match optional_string_array_member(obj, "harnesses", 0, 40, 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let auto_attach = match optional_auto_attach_member(obj, "autoAttach") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let enabled = match optional_boolean_member(obj, "enabled") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // ── admin-only below: these two reach the host, not just the sandbox ──
    let image = match optional_max_string_member(obj, "image", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let mounts = match optional_string_array_member(obj, "mounts", 0, 300, 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // image and mounts become compose volumes / the image the sandbox runs as
    // root from — a host mount plus a fleet roll is host root. agents.manage
    // is grantable to non-admins, and the catalog entry promises its holders
    // that "infrastructure stays admin-only", so hold that line here.
    let infra: Vec<&str> = INFRA_FIELDS
        .iter()
        .copied()
        .filter(|f| obj.contains_key(*f))
        .collect();
    if !infra.is_empty() {
        match crate::session::require_admin(&state, &headers).await {
            Ok(admin) => user = admin,
            Err(_gate) => {
                return house_error(
                    StatusCode::FORBIDDEN,
                    &format!("{} are admin-only", infra.join(" and ")),
                );
            }
        }
    }
    if let Some(mounts) = &mounts {
        for mount in mounts {
            if let Some(why) = mount_error(mount) {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    &format!("mount \"{mount}\" rejected: {why}"),
                );
            }
        }
    }

    let patch = ProfilePatch {
        name: name.clone(),
        description: description.clone(),
        image: image.clone(),
        env: env.clone(),
        mounts: mounts.clone(),
        harnesses: harnesses.clone(),
        auto_attach: auto_attach.clone(),
        config: None,
        enabled,
    };
    match update_profile(&state.pg, &slug, &patch).await {
        Ok(true) => {}
        Ok(false) => return house_error(StatusCode::NOT_FOUND, "unknown profile"),
        Err(e) => {
            tracing::error!("[workbench] profile write failed: {e}");
            return thrown_internal_error();
        }
    }
    // after: {...patch, env: keys} — schema key order, env replaced by its
    // KEY LIST. Env values are per-profile config that can carry credentials
    // — the trail records which names moved, never what they were set to.
    let mut after = Map::new();
    if let Some(name) = &name {
        after.insert("name".into(), json!(name));
    }
    if let Some(description) = &description {
        after.insert("description".into(), json!(description));
    }
    if let Some(env) = &env {
        after.insert(
            "env".into(),
            json!(env.keys().map(String::as_str).collect::<Vec<_>>()),
        );
    }
    if let Some(harnesses) = &harnesses {
        after.insert("harnesses".into(), json!(harnesses));
    }
    if let Some(auto_attach) = &auto_attach {
        after.insert(
            "autoAttach".into(),
            json!({
                "departments": auto_attach.departments,
                "roles": auto_attach.roles,
            }),
        );
    }
    if let Some(enabled) = enabled {
        after.insert("enabled".into(), json!(enabled));
    }
    if let Some(image) = &image {
        after.insert("image".into(), json!(image));
    }
    if let Some(mounts) = &mounts {
        after.insert("mounts".into(), json!(mounts));
    }
    let audit_after = after;
    let audit_label = name.unwrap_or_else(|| slug.clone());
    let pg = state.pg.clone();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "workbench.profile_update",
                target_type: "workbench-profile",
                target_id: Some(&slug),
                target_label: Some(&audit_label),
                before: None,
                after: Some(Value::Object(audit_after)),
            },
        )
        .await;
    });
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_member_passes_records_and_refuses_scalars() {
        let mut obj = Map::new();
        obj.insert("env".into(), json!({ "A": "1", "B": "" }));
        assert_eq!(
            optional_env_member(&obj, "env", 500)
                .unwrap()
                .unwrap()
                .len(),
            2
        );
        obj.insert("env".into(), json!("nope"));
        assert_eq!(
            optional_env_member(&obj, "env", 500).unwrap_err(),
            record_msg("string")
        );
        obj.insert("env".into(), json!({ "A": 5 }));
        assert_eq!(
            optional_env_member(&obj, "env", 500).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        let long = "x".repeat(501);
        obj.insert("env".into(), json!({ "A": long }));
        assert_eq!(
            optional_env_member(&obj, "env", 500).unwrap_err(),
            too_big_msg(500)
        );
        obj.insert("env".into(), Value::Null);
        assert_eq!(
            optional_env_member(&obj, "env", 500).unwrap_err(),
            record_msg("null")
        );
        obj.remove("env");
        assert!(optional_env_member(&obj, "env", 500).unwrap().is_none());
    }
}
