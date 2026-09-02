// /api/fleet/defs/$id/mcp — port of ui/src/routes/api/fleet.defs.$id.mcp.ts.
// POST → add/remove MCP servers on an agent as a NEW config version (same
// versioned-internals contract as model edits), optionally applied live.

use crate::agent_defs::{add_version_if_changed, list_versions};
use crate::agent_mcp::apply_mcp_edits;
use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    NumKind, array_msg, array_too_big_msg, as_object, nullable_number_member,
    optional_boolean_member, parse, string_msg, too_big_msg, url_member, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::reconcile::roll_agent;
use crate::session::{actor_of, require_perm};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

const NAME_PATTERN: &str = "^[a-z0-9][a-z0-9_-]*$";

fn slug_ok(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .skip(1)
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// One `add` entry: {name, url, timeout?} — timeout is a positive int, and
/// a 0 (or absent) timeout means "none" downstream.
struct AddServer {
    name: String,
    url: String,
    timeout: Option<Value>,
}

fn parse_add(obj: &Map<String, Value>) -> Result<Vec<AddServer>, String> {
    let Some(v) = obj.get("add") else {
        return Ok(Vec::new()); // `.default([])`
    };
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::new();
    for el in arr {
        let m = el
            .as_object()
            .ok_or_else(|| crate::body::object_msg(zod_type_name(el)))?;
        let name = match m.get("name") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => return Err(string_msg(zod_type_name(other))),
            None => return Err(string_msg("undefined")),
        };
        if !slug_ok(&name) {
            return Err(format!(
                "Invalid string: must match pattern /{NAME_PATTERN}/"
            ));
        }
        if crate::body::utf16_len(&name) > 60 {
            return Err(too_big_msg(60));
        }
        let url = url_member(m, "url", 300)?;
        let timeout = match m.get("timeout") {
            None => None,
            Some(_) => {
                let n =
                    nullable_number_member(m, "timeout", NumKind::Int, f64::MIN, f64::INFINITY)?
                        .ok_or("unreachable: present non-null read above")?;
                if n <= 0.0 {
                    return Err("Too small: expected number to be >0".into());
                }
                if n > 3600.0 {
                    return Err(format!("Too big: expected number to be <={}", 3600i64));
                }
                Some(Value::from(crate::body::js_num(n)))
            }
        };
        out.push(AddServer { name, url, timeout });
    }
    if arr.len() > 20 {
        return Err(array_too_big_msg(20));
    }
    Ok(out)
}

fn parse_remove(obj: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(v) = obj.get("remove") else {
        return Ok(Vec::new()); // `.default([])`
    };
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::new();
    for el in arr {
        let s = el.as_str().ok_or_else(|| string_msg(zod_type_name(el)))?;
        if crate::body::utf16_len(s) > 60 {
            return Err(too_big_msg(60));
        }
        out.push(s.to_string());
    }
    if arr.len() > 20 {
        return Err(array_too_big_msg(20));
    }
    Ok(out)
}

pub async fn post(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_perm(&state, &headers, "agents.manage").await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let add = match parse_add(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let remove = match parse_remove(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let apply = match optional_boolean_member(obj, "apply") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    }
    .unwrap_or(false);

    // getAgentDef — the columns this route touches, in its column order.
    let def: Option<(String, String, bool, String, String)> = match sqlx::query_as(
        "select id::text, slug, managed, department, display_name from agent_defs where id = $1::uuid",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            // A non-uuid id is the same postgres refusal TS throws on.
            tracing::error!("[fleet/defs/mcp] def read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some((def_id, slug, managed, department, display_name)) = def else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let latest = match list_versions(&state.pg, &def_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[fleet/defs/mcp] versions read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(latest) = latest.first() else {
        return house_error(StatusCode::BAD_REQUEST, "no base version — import first");
    };

    let add_values: Vec<Value> = add
        .iter()
        .map(|a| {
            let mut e = Map::new();
            e.insert("name".into(), json!(a.name));
            e.insert("url".into(), json!(a.url));
            if let Some(t) = &a.timeout {
                e.insert("timeout".into(), t.clone());
            }
            Value::Object(e)
        })
        .collect();
    let config = apply_mcp_edits(&latest.config, &slug, &add_values, &remove);
    let added = add
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let removed = remove.join(", ");
    let parts: Vec<String> = [
        (!added.is_empty()).then(|| format!("+{added}")),
        (!removed.is_empty()).then(|| format!("-{removed}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let note = if parts.is_empty() {
        "mcp: no-op".to_string()
    } else {
        format!("mcp: {}", parts.join(" "))
    };
    let created_by = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let (version, created) = match add_version_if_changed(
        &state.pg,
        &def_id,
        &crate::agent_defs::NewVersion {
            soul: &latest.soul,
            config: &config,
            note: Some(&note),
            created_by: Some(&created_by),
        },
    )
    .await
    {
        Ok(vc) => vc,
        Err(e) => {
            tracing::error!("[fleet/defs/mcp] version write failed: {e}");
            return thrown_internal_error();
        }
    };
    if created {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "agent.mcp_edit",
                target_type: "agent",
                target_id: Some(&def_id),
                target_label: Some(&display_name),
                before: None,
                after: Some(json!({
                    "added": add.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
                    "removed": remove,
                })),
            },
        )
        .await;
    }
    let mut applied = false;
    if created && apply && managed {
        // Roll, don't restart — see fleet.defs.$id.edit.
        let sb = match state.secretbox().await {
            Ok(sb) => sb,
            Err(e) => {
                tracing::error!("[fleet/defs/mcp] secretbox unavailable: {e}");
                return thrown_internal_error();
            }
        };
        match roll_agent(&state.pg, &sb, &department).await {
            Ok(None) => applied = true,
            Ok(Some(warning)) => {
                return Json(json!({
                    "ok": true,
                    "version": version,
                    "created": created,
                    "applied": false,
                    "warning": warning,
                }))
                .into_response();
            }
            Err(e) => return house_error(StatusCode::BAD_REQUEST, &e),
        }
    }
    Json(json!({ "ok": true, "version": version, "created": created, "applied": applied }))
        .into_response()
}
