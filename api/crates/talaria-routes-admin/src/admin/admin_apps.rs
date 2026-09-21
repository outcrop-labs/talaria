// /api/admin/apps. App administration. GET → installed apps (+ ?catalog=1
// for the marketplace feed). Reads are open to anyone granted the /apps
// Manage view; mutations (enable/disable, install, uninstall, catalog
// source) stay admin-only — installing an app adds CODE to the deployment.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use talaria_apps::{
    catalog_url, enabled_app_slugs, fetch_catalog, install_app_from_git, installed_sources,
    set_app_enabled, set_catalog_url, uninstall_app, wipe_app_data,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::parse;
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin, require_view, secretbox_or_500};
use talaria_state::AppState;
use talaria_users::{app_build_status, discovered_apps};

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    // The one admin-family read that /apps Manage holders share with admins.
    require_view(&state, &headers, "/apps").await?;
    let enabled = enabled_app_slugs(&state.pg).await;
    let sources = installed_sources(&state.pg).await;

    let apps = discovered_apps()
        .into_iter()
        .map(|a| {
            let mut surfaces = serde_json::Map::new();
            if let Some(w) = &a.work {
                surfaces.insert("work".into(), serde_json::json!(w));
            }
            if let Some(m) = &a.manage {
                surfaces.insert("manage".into(), serde_json::json!(m));
            }
            if let Some(s) = &a.settings {
                surfaces.insert("settings".into(), serde_json::json!(s));
            }
            let source = sources
                .get(&a.slug)
                .and_then(|s| s.get("source"))
                .cloned()
                .unwrap_or(Value::Null);
            let build = app_build_status(&a.slug);
            serde_json::json!({
                "slug": a.slug,
                "name": a.name,
                "icon": a.icon,
                "description": a.description,
                "version": a.version,
                "surfaces": Value::Object(surfaces),
                "mcp": a.mcp,
                "enabled": enabled.contains(&a.slug),
                "source": source,
                "build": build,
            })
        })
        .collect::<Vec<_>>();
    // ?catalog=1
    let want_catalog = talaria_api_facades::google::oauth::query_pairs(uri.query())
        .get("catalog")
        .map(String::as_str)
        == Some("1");
    let catalog = if want_catalog {
        let (apps, error) = fetch_catalog(&state.pg).await;
        let mut m = serde_json::Map::new();
        m.insert("apps".into(), Value::Array(apps));
        if let Some(e) = error {
            m.insert("error".into(), Value::String(e));
        }
        Value::Object(m)
    } else {
        Value::Null
    };
    Ok(Json(serde_json::json!({
        "apps": apps,
        "catalog": catalog,
        "catalogUrl": catalog_url(&state.pg).await,
    }))
    .into_response())
}

/// any parseable URL.
fn url_ok(v: &str) -> bool {
    url::Url::parse(v).is_ok()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let actor = actor_of(&user);
    let sb = secretbox_or_500(&state, "[admin/apps] secretbox unavailable").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Two bodies on one PUT, dispatched by key presence: { app, enabled }
    // toggles an app, { catalogUrl } (nullable — null resets to the default
    // source) sets the catalog source. A body naming NONE of the keys answers
    // 400 rather than silently falling into the catalog arm and resetting the
    // source; a catalogUrl that is a string but not a URL answers the field's
    // own sentence ("Invalid URL").
    if obj.contains_key("app") || obj.contains_key("enabled") {
        let app = match talaria_body::string_member(obj, "app", 1, usize::MAX) {
            Ok(a) => a,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        let enabled = match talaria_body::boolean_member(obj, "enabled") {
            Ok(e) => e,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        if let Err(msg) = set_app_enabled(&state.pg, &sb, &app, enabled).await {
            return Ok(house_error(StatusCode::BAD_REQUEST, &msg));
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: if enabled { "app.enable" } else { "app.disable" },
                target_type: "app",
                target_id: Some(&app),
                target_label: None,
                before: None,
                after: None,
            },
        )
        .await;
        return Ok(Json(serde_json::json!({ "ok": true })).into_response());
    }
    let next_url = match obj.get("catalogUrl") {
        None => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "body must name app + enabled, or catalogUrl",
            ));
        }
        Some(Value::Null) => None,
        Some(Value::String(u)) => {
            if !url_ok(u) {
                return Ok(house_error(StatusCode::BAD_REQUEST, "Invalid URL"));
            }
            Some(u.clone())
        }
        Some(v) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &talaria_body::string_msg(talaria_body::zod_type_name(v)),
            ));
        }
    };
    if let Err(msg) = set_catalog_url(&state.pg, next_url.as_deref()).await {
        return Ok(house_error(StatusCode::BAD_REQUEST, &msg));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "app.catalog-url",
            target_type: "app",
            target_id: None,
            target_label: Some(next_url.as_deref().unwrap_or("default")),
            before: None,
            after: None,
        },
    )
    .await;
    let url = catalog_url(&state.pg).await;
    Ok(Json(serde_json::json!({ "ok": true, "catalogUrl": url })).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // installUrl: a required URL; slug: optional override, 1..64 chars.
    let install_url = match obj.get("installUrl") {
        None => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &talaria_body::string_msg("undefined"),
            ));
        }
        Some(v) => match v.as_str() {
            Some(u) if url_ok(u) => u.to_string(),
            Some(_) => return Ok(house_error(StatusCode::BAD_REQUEST, "Invalid URL")),
            None => {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    &talaria_body::string_msg(talaria_body::zod_type_name(v)),
                ));
            }
        },
    };
    let slug_override = match talaria_body::optional_string_member(obj, "slug", 64) {
        Ok(s) => s,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
        match install_app_from_git(&state.pg, &install_url, slug_override.as_deref()).await {
            Ok(slug) => {
                log_audit(
                    &state.pg,
                    AuditEntry {
                        actor: &actor_of(&user),
                        action: "app.install",
                        target_type: "app",
                        target_id: Some(&slug),
                        target_label: Some(&install_url),
                        before: None,
                        after: None,
                    },
                )
                .await;
                Json(serde_json::json!({ "slug": slug })).into_response()
            }
            Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
        },
    )
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let sb = secretbox_or_500(&state, "[admin/apps] secretbox unavailable").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // app: required, min 1 char; wipeData: optional boolean.
    let app = match talaria_body::string_member(obj, "app", 1, usize::MAX) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let wipe_data = match talaria_body::optional_boolean_member(obj, "wipeData") {
        Ok(w) => w.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(msg) = uninstall_app(&state.pg, &sb, &app).await {
        return Ok(house_error(StatusCode::BAD_REQUEST, &msg));
    }
    if wipe_data && let Err(e) = wipe_app_data(&state.pg, &app).await {
        return Ok(internal("[admin/apps] data wipe failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "app.uninstall",
            target_type: "app",
            target_id: Some(&app),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}
