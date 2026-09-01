// /api/fleet/defs/{id}/edit — port of ui/src/routes/api/fleet.defs.$id.edit.ts.
// POST → save an edit as a NEW immutable version (and optionally apply it to
// the running managed container). Admin. This is "versioned agent internals":
// nothing shifts silently — every change is a version you can diff and revert.

use crate::agent_defs::{
    AliasTarget, ConfigEdits, ModelTarget, NewVersion, add_version_if_changed, apply_config_edits,
    list_versions,
};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    NumKind, array_msg, array_too_big_msg, as_object, nullable_number_member,
    optional_boolean_member, optional_max_string_member, parse, string_member, string_msg,
    too_big_msg, too_small_msg, utf16_len, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet_reconcile::roll_agent;
use crate::gateway::provider::catalog_models;
use crate::gateway::registry::{LlmEndpoint, add_endpoint_models, list_endpoints};
use crate::session::{actor_of, require_perm};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// One Target: `{ endpoint, model, contextLength?, effort? }`. The effort is
/// NOT validated against the model's published levels here: the editor only
/// offers real levels, and every runtime consumer re-validates against the
/// live metadata — a level that went stale between save and use is inert,
/// never a refused turn.
fn parse_target(m: &Map<String, Value>) -> Result<ModelTarget, String> {
    let endpoint = string_member(m, "endpoint", 1, 100)?;
    let model = string_member(m, "model", 1, 200)?;
    let context_length = match m.get("contextLength") {
        None => None,
        Some(Value::Null) => return Err("Invalid input: expected number, received null".into()),
        Some(_) => {
            // `.int().positive()` — the bound is exclusive, which the folded
            // helper's >= min cannot say.
            let n =
                nullable_number_member(m, "contextLength", NumKind::Int, f64::MIN, f64::INFINITY)?
                    .ok_or("unreachable: present non-null read above")?;
            if n <= 0.0 {
                return Err("Too small: expected number to be >0".into());
            }
            Some(n as i64)
        }
    };
    let effort = match m.get("effort") {
        None => None,
        Some(Value::Null) => None,
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            if s.chars().count() < 1 {
                return Err(too_small_msg(1));
            }
            if utf16_len(s) > 24 {
                return Err(too_big_msg(24));
            }
            Some(s.to_string())
        }
    };
    Ok(ModelTarget {
        endpoint,
        model,
        context_length,
        effort,
    })
}

/// A required Target member (`main: Target`) — an object, not an array.
fn target_member(obj: &Map<String, Value>, key: &str) -> Result<ModelTarget, String> {
    let v = obj
        .get(key)
        .ok_or_else(|| crate::body::object_msg("undefined"))?;
    let m = v
        .as_object()
        .ok_or_else(|| crate::body::object_msg(zod_type_name(v)))?;
    parse_target(m)
}

/// A required array of Targets (`aliases` adds the tier `name`). Items run
/// zod's order — each member, then the array's own length bound.
fn target_array<T>(
    obj: &Map<String, Value>,
    key: &str,
    max_items: usize,
    mut item: impl FnMut(&Map<String, Value>) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let v = obj.get(key).ok_or_else(|| array_msg("undefined"))?;
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let m = el
            .as_object()
            .ok_or_else(|| crate::body::object_msg(zod_type_name(el)))?;
        out.push(item(m)?);
    }
    if arr.len() > max_items {
        return Err(array_too_big_msg(max_items));
    }
    Ok(out)
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
    let soul = match string_member(obj, "soul", 0, 200_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let main = match target_member(obj, "main") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let aliases = match target_array(obj, "aliases", 20, |m| {
        Ok(AliasTarget {
            name: string_member(m, "name", 1, 60)?,
            target: parse_target(m)?,
        })
    }) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let fallbacks = match target_array(obj, "fallbacks", 10, parse_target) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let note = match optional_max_string_member(obj, "note", 300) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Re-render + restart the managed container so the edit takes effect now.
    let apply = match optional_boolean_member(obj, "apply") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    }
    .unwrap_or(false);

    let def: Option<(String, String, bool, String)> = match sqlx::query_as(
        "select id::text, department, managed, display_name from agent_defs where id = $1::uuid",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::error!("[fleet/defs/edit] def read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some((def_id, department, managed, display_name)) = def else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };

    let versions = match list_versions(&state.pg, &def_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[fleet/defs/edit] versions read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(latest) = versions.first() else {
        return house_error(StatusCode::BAD_REQUEST, "no base version — import first");
    };

    // A picked model must actually route: auto-register it on its endpoint
    // when the provider's LIVE catalog serves it (no maintained lists — the
    // registration follows the choice), refuse clearly when it doesn't.
    // Without this, an unregistered model renders fine and the agent then
    // dies with a gateway 404 on its first turn — a silent-freeze chat.
    let endpoints: Vec<LlmEndpoint> = match list_endpoints(&state.pg).await {
        Ok(eps) => eps,
        Err(e) => {
            tracing::error!("[fleet/defs/edit] endpoints read failed: {e}");
            return thrown_internal_error();
        }
    };
    let mut endpoints: HashMap<String, LlmEndpoint> =
        endpoints.into_iter().map(|e| (e.name.clone(), e)).collect();
    let targets = std::iter::once((&main.endpoint, &main.model))
        .chain(
            aliases
                .iter()
                .map(|a| (&a.target.endpoint, &a.target.model)),
        )
        .chain(fallbacks.iter().map(|f| (&f.endpoint, &f.model)));
    for (endpoint, model) in targets {
        let Some(ep) = endpoints.get(endpoint) else {
            return house_error(
                StatusCode::BAD_REQUEST,
                &format!("endpoint \"{endpoint}\" does not exist"),
            );
        };
        if ep.models.iter().any(|m| m == model) {
            continue;
        }
        let live: Option<Vec<String>> = match catalog_models(&state, ep).await {
            Ok(catalog) => Some(catalog.into_iter().map(|m| m.id).collect()),
            // `.catch(() => null)` — an unreachable catalog is "cannot vouch",
            // not an error.
            Err(_) => None,
        };
        if live
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|m| m == model))
        {
            let ep = endpoints.get_mut(endpoint).expect("present above");
            if let Err(e) =
                add_endpoint_models(&state.pg, &ep.name, std::slice::from_ref(model)).await
            {
                tracing::error!("[fleet/defs/edit] endpoint model register failed: {e}");
                return thrown_internal_error();
            }
            ep.models.push(model.to_string());
        } else {
            return house_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "\"{model}\" is not registered on \"{endpoint}\"{} — pick it on /models first",
                    if live.is_some() {
                        " and its live catalog does not list it"
                    } else {
                        ""
                    }
                ),
            );
        }
    }

    // applyConfigEdits' refusal ("cannot edit what has no models" and
    // friends) is the catch's 400 in the TS; the version write beneath it is
    // a db error → 500.
    let edits = ConfigEdits {
        main,
        aliases,
        fallbacks,
    };
    let config = match apply_config_edits(&state.pg, &latest.config, &edits).await {
        Ok(c) => c,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let created_by = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let note = note.unwrap_or_else(|| "edited in Talaria".into());
    let (version, created) = match add_version_if_changed(
        &state.pg,
        &def_id,
        &NewVersion {
            soul: &soul,
            config: &config,
            note: Some(&note),
            created_by: Some(&created_by),
        },
    )
    .await
    {
        Ok(vc) => vc,
        Err(e) => {
            tracing::error!("[fleet/defs/edit] version write failed: {e}");
            return thrown_internal_error();
        }
    };
    if created {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "agent.edit",
                target_type: "agent",
                target_id: Some(&def_id),
                target_label: Some(&display_name),
                before: None,
                after: Some(json!({ "version": version })),
            },
        )
        .await;
    }
    let mut applied = false;
    if created && apply && managed {
        // Roll, don't restart: the new config comes up beside the old
        // container and traffic cuts over only after health — applying an
        // edit never interrupts conversations in flight.
        let sb = match state.secretbox().await {
            Ok(sb) => sb,
            Err(e) => {
                tracing::error!("[fleet/defs/edit] secretbox unavailable: {e}");
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
