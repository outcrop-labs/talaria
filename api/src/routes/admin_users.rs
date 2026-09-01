// /api/admin/users — port of ui/src/routes/api/admin.users.ts. The people
// console. GET → every user with role, agent allow-list, view denials.
// PUT → the per-user levers, applied in TS's order (role first, the
// assistant's elevation right behind it — a demotion collapses both).

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use crate::users::{
    admin_count, list_users_admin, set_allowed_manage_views, set_assistant_elevated,
    set_denied_views, set_user_agent_access, set_user_can_mint_keys, set_user_role,
};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let users = match list_users_admin(&state.pg).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("[admin/users] list failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(serde_json::json!({ "users": users })).into_response()
}

/// The PUT body. `agentModels`/`deniedViews`/`allowedManageViews` are
/// TRUTHINESS-gated in TS (an empty array still writes the empty set — the
/// console's "clear everything" gesture); the booleans are !== undefined.
struct PutBody {
    user_id: String,
    role: Option<String>,
    agent_models: Option<Vec<String>>,
    can_mint_keys: Option<bool>,
    denied_views: Option<Vec<String>>,
    allowed_manage_views: Option<Vec<String>>,
    assistant_elevated: Option<bool>,
}

fn parse_put_body(obj: &serde_json::Map<String, Value>) -> Result<PutBody, String> {
    use crate::body::{
        array_msg, array_too_big_msg, optional_boolean_member, optional_enum_member, uuid_member,
        zod_type_name,
    };
    // Keys in schema order; each rejection in zod's own words. The union's
    // old blanket ("Invalid input") was a wire deviation: zod names the
    // field's own failure, and a bad role was silently DROPPED here rather
    // than refused — a 200 for a write that never happened.
    let user_id = uuid_member(obj, "userId")?;
    let role = optional_enum_member(obj, "role", &["admin", "member"])?;
    let string_array =
        |key: &str, item_max: usize, max_items: usize| -> Result<Option<Vec<String>>, String> {
            match obj.get(key) {
                None => Ok(None),
                Some(v) => {
                    let a = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
                    if a.len() > max_items {
                        return Err(array_too_big_msg(max_items));
                    }
                    let mut out = Vec::with_capacity(a.len());
                    for x in a {
                        out.push(optional_max_string_member_value(x, item_max)?);
                    }
                    Ok(Some(out))
                }
            }
        };
    Ok(PutBody {
        user_id,
        role,
        agent_models: string_array("agentModels", 200, 100)?,
        can_mint_keys: optional_boolean_member(obj, "canMintKeys")?,
        denied_views: string_array("deniedViews", 60, 40)?,
        allowed_manage_views: string_array("allowedManageViews", 60, 10)?,
        assistant_elevated: optional_boolean_member(obj, "assistantElevated")?,
    })
}

/// One array element of `z.string().max(n)` — the element-facing half of the
/// string-array members, with zod's element messages.
fn optional_max_string_member_value(v: &Value, max: usize) -> Result<String, String> {
    let s = v
        .as_str()
        .ok_or_else(|| crate::body::string_msg(crate::body::zod_type_name(v)))?;
    if crate::body::utf16_len(s) > max {
        return Err(crate::body::too_big_msg(max));
    }
    Ok(s.to_string())
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
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match parse_put_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // The last admin is undemotable — that includes self-demotion, which is
    // FINE once another admin remains. Roles come from the claim and this
    // page; nothing re-grants admin after the last one is gone.
    if body.role.as_deref() == Some("member") {
        let count = admin_count(&state.pg).await.unwrap_or(0);
        if count <= 1 {
            let target = list_users_admin(&state.pg)
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|u| u.get("id") == Some(&serde_json::json!(body.user_id)));
            if target
                .as_ref()
                .and_then(|u| u.get("role"))
                .and_then(Value::as_str)
                == Some("admin")
            {
                return house_error(StatusCode::BAD_REQUEST, "cannot demote the last admin");
            }
        }
    }

    if let Some(role) = &body.role {
        if let Err(e) = set_user_role(&state.pg, &body.user_id, role).await {
            tracing::error!("[admin/users] role write failed: {e}");
            return thrown_internal_error();
        }
        // Live sessions pick the role up immediately — no re-login dance.
        if let Err(e) = crate::session::update_sessions_for_user(
            &state,
            &body.user_id,
            &serde_json::json!({ "role": role }),
        )
        .await
        {
            tracing::error!("[admin/users] session patch failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "user.role",
                target_type: "user",
                target_id: Some(&body.user_id),
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "role": role })),
            },
        )
        .await;
        // Demotion collapses the assistant's org-wide reach with the human.
        if role == "member" {
            let _ = set_assistant_elevated(&state.pg, &body.user_id, false).await;
        }
    }

    if let Some(elevated) = body.assistant_elevated {
        if elevated {
            // Only an admin's assistant can be elevated — it inherits their
            // standing. (TS's sentence carries a curly apostrophe.)
            let target = list_users_admin(&state.pg)
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|u| u.get("id") == Some(&serde_json::json!(body.user_id)));
            let target_role = body.role.clone().or_else(|| {
                target
                    .as_ref()
                    .and_then(|u| u.get("role"))
                    .and_then(Value::as_str)
                    .map(String::from)
            });
            if target_role.as_deref() != Some("admin") {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    "only an admin\u{2019}s assistant can be elevated",
                );
            }
            let has_assistant = target
                .as_ref()
                .and_then(|u| u.get("assistantModel"))
                .map(|m| !m.is_null())
                .unwrap_or(false);
            if !has_assistant {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    "that user has no personal assistant",
                );
            }
        }
        if let Err(e) = set_assistant_elevated(&state.pg, &body.user_id, elevated).await {
            tracing::error!("[admin/users] assistant elevation failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "user.assistant_elevated",
                target_type: "user",
                target_id: Some(&body.user_id),
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "assistantElevated": elevated })),
            },
        )
        .await;
    }

    // Truthiness: an empty array still writes (the console's clear gesture).
    if let Some(models) = &body.agent_models {
        if let Err(e) = set_user_agent_access(&state.pg, &body.user_id, models).await {
            tracing::error!("[admin/users] agent access write failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "user.agent_access",
                target_type: "user",
                target_id: Some(&body.user_id),
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "agentModels": models })),
            },
        )
        .await;
    }

    if let Some(mint) = body.can_mint_keys {
        if let Err(e) = set_user_can_mint_keys(&state.pg, &body.user_id, mint).await {
            tracing::error!("[admin/users] can-mint-keys write failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "user.can_mint_keys",
                target_type: "user",
                target_id: Some(&body.user_id),
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "canMintKeys": mint })),
            },
        )
        .await;
    }

    if let Some(denied) = &body.denied_views {
        if let Err(e) = set_denied_views(&state.pg, &body.user_id, denied).await {
            tracing::error!("[admin/users] denied views write failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "user.view_access",
                target_type: "user",
                target_id: Some(&body.user_id),
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "deniedViews": denied })),
            },
        )
        .await;
    }

    if let Some(allowed) = &body.allowed_manage_views {
        if let Err(e) = set_allowed_manage_views(&state.pg, &body.user_id, allowed).await {
            tracing::error!("[admin/users] manage views write failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "user.manage_views",
                target_type: "user",
                target_id: Some(&body.user_id),
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "allowedManageViews": allowed })),
            },
        )
        .await;
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}
