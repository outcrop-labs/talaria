// /api/fleet/agents/{id}/secrets. Per-agent secrets, write-only. GET →
// names + timestamps (never values), and for a CONFIG_* secret where the
// current config names it (usedAt). PUT { name, value } → set/replace.
// DELETE { name, removeSetting? } → remove; a CONFIG_* secret the current
// config still names is a 409 { error, usedAt } unless removeSetting is true,
// which first writes a new version without those settings. Admin, or the
// owner of a personal assistant. Takes effect on the next start from
// Talaria. Every write audits — secret NAMES only, never values.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_defs::{NewVersion, add_version_if_changed, list_versions};
use talaria_agent_secrets::{
    CONFIG_SECRET_PREFIX, config_secret_paths, delete_agent_secret, list_agent_secrets,
    set_agent_secret, withhold_config_secret,
};
use talaria_audit::{AuditEntry, log_audit, spawn_audit};
use talaria_body::{
    as_object, optional_boolean_member, parse, string_member, trimmed_string_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_agent_manager, secretbox_or_500};
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_agent_manager(&state, &headers, &id).await?;
    Ok(match list_agent_secrets(&state.pg, &id).await {
        Ok(secrets) => Json(json!({ "secrets": secrets })).into_response(),
        Err(e) => internal("[fleet] list_agent_secrets failed", e),
    })
}

pub async fn put(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match trimmed_string_member(obj, "name", 2, 64) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // value is UNtrimmed — a leading space is a legal secret character.
    let value = match string_member(obj, "value", 1, 8192) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let sb = secretbox_or_500(&state, "[fleet] secretbox failed").await?;
    let actor = user.email.clone().or_else(|| user.name.clone());
    Ok(
        match set_agent_secret(&state.pg, &sb, &id, &name, &value, actor.as_deref()).await {
            Ok(()) => {
                // Names only, never values, and never on the caller's clock.
                spawn_audit(
                    &state.pg,
                    actor_of(&user),
                    "agent.secret_set",
                    "agent",
                    Some(id.clone()),
                    Some(json!({ "name": name })),
                );
                Json(json!({ "ok": true })).into_response()
            }
            Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
        },
    )
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    // Body { name } — matches PUT's transport. When the body doesn't parse
    // (unparseable JSON, wrong shape, failed validation), the old ?name=
    // query spelling still works, and THAT path is only length-checked by
    // truthiness.
    let parsed = parse(&body);
    let obj = as_object(&parsed).ok();
    let body_name = obj.and_then(|obj| string_member(obj, "name", 1, 64).ok());
    let name = match body_name {
        Some(n) => n,
        None => match query_param(&uri, "name") {
            Some(n) if !n.is_empty() => n,
            _ => return Ok(house_error(StatusCode::BAD_REQUEST, "missing name")),
        },
    };
    // Take the settings that name this CONFIG_* secret out of the config too, in a new version.
    let remove_setting = obj
        .and_then(|obj| optional_boolean_member(obj, "removeSetting").ok().flatten())
        .unwrap_or(false)
        || query_param(&uri, "removeSetting").is_some_and(|v| v == "true" || v == "1");

    // A CONFIG_* secret is a credential sealed out of the agent's own config,
    // which names it as `${CONFIG_…}`. Deleted from under that reference,
    // Hermes would read the handle's own text as the value — a token that
    // fails, or a dashboard password every fleet reader can see. So either
    // the setting goes with it (a new version, as an edit would write), or
    // nothing is deleted.
    let mut removed: Vec<String> = Vec::new();
    let mut version: Option<i32> = None;
    if name.starts_with(CONFIG_SECRET_PREFIX) {
        let versions = match list_versions(&state.pg, &id).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[fleet] secret delete: versions read failed", e)),
        };
        if let Some(latest) = versions.first() {
            let used_at = config_secret_paths(&latest.config, &name);
            if !used_at.is_empty() && !remove_setting {
                return Ok((
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": format!(
                            "{name} is in use by this agent's config ({}). Deleting it alone would leave the agent reading the text \"${{{name}}}\" as the value: replace its value instead, or delete with removeSetting to take the setting out too",
                            used_at.join(", ")
                        ),
                        "usedAt": used_at,
                    })),
                )
                    .into_response());
            }
            if !used_at.is_empty() {
                let (config, withheld) = withhold_config_secret(&latest.config, &name);
                let created_by = user
                    .email
                    .clone()
                    .or_else(|| user.name.clone())
                    .unwrap_or_else(|| "admin".into());
                let note = format!("removed sealed credential at {}", withheld.join(", "));
                let (v, created) = match add_version_if_changed(
                    &state.pg,
                    &id,
                    &NewVersion {
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
                        return Ok(internal("[fleet] secret delete: version write failed", e));
                    }
                };
                if created {
                    log_audit(
                        &state.pg,
                        AuditEntry {
                            actor: &actor_of(&user),
                            action: "agent.edit",
                            target_type: "agent",
                            target_id: Some(&id),
                            target_label: None,
                            before: None,
                            after: Some(json!({ "version": v })),
                        },
                    )
                    .await;
                }
                version = Some(v);
                removed = withheld;
            }
        }
    }
    if let Err(e) = delete_agent_secret(&state.pg, &id, &name).await {
        return Ok(internal("[fleet] query_param failed", e));
    }
    // Names only, never values, and never on the caller's clock.
    let mut after = json!({ "name": name });
    if !removed.is_empty() {
        after["removedSetting"] = json!(removed);
    }
    spawn_audit(
        &state.pg,
        actor_of(&user),
        "agent.secret_delete",
        "agent",
        Some(id.clone()),
        Some(after),
    );
    let mut out = json!({ "ok": true });
    if let Some(v) = version {
        out["version"] = json!(v);
        out["removed"] = json!(removed);
    }
    Ok(Json(out).into_response())
}

/// Query lookup — the FIRST value for the key.
fn query_param(uri: &Uri, key: &str) -> Option<String> {
    uri.query().and_then(|q| {
        q.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            if k == key {
                Some(percent_decode(v))
            } else {
                None
            }
        })
    })
}

// spellings (%20, %2F) plus '+'-as-space.
fn percent_decode(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                // get() handles the trailing-% boundary; a bad hex pair
                // passes through verbatim.
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|s| u8::from_str_radix(s, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
