// /api/admin/apps — port of ui/src/routes/api/admin.apps.ts. App
// administration. GET → installed apps (+ ?catalog=1 for the marketplace
// feed). Reads are open to anyone granted the /apps Manage view; mutations
// (enable/disable, install, uninstall, catalog source) stay admin-only —
// installing an app adds CODE to the deployment.

use crate::apps::{
    catalog_url, enabled_app_slugs, fetch_catalog, install_app_from_git, installed_sources,
    pending_apps, set_app_enabled, set_catalog_url, uninstall_app, wipe_app_data,
};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_admin, require_view};
use crate::state::AppState;
use crate::users::discovered_apps;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

pub async fn get(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    // requireView(request, '/apps') — the one admin-family read that Manage
    // holders share with admins.
    if let Err(gate) = require_view(&state, &headers, "/apps").await {
        return gate;
    }
    let enabled = enabled_app_slugs(&state.pg).await;
    let pending = pending_apps().await;
    let sources = installed_sources(&state.pg).await;
    let apps = discovered_apps()
        .into_iter()
        .map(|a| {
            // {...a, enabled, source} — AppManifest's wire order, surfaces
            // nested with only its truthy keys.
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
            })
        })
        .collect::<Vec<_>>();
    // ?catalog=1
    let want_catalog = crate::google_oauth::query_pairs(uri.query())
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
    Json(serde_json::json!({
        "apps": apps,
        "pending": pending,
        "catalog": catalog,
        "catalogUrl": catalog_url(&state.pg).await,
    }))
    .into_response()
}

/// z.string().url() — any parseable URL.
fn url_ok(v: &str) -> bool {
    url::Url::parse(v).is_ok()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let actor = actor_of(&user);
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[admin/apps] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.union([{ app: z.string().min(1), enabled: z.boolean() },
    //           { catalogUrl: z.string().url().nullable() }]) — dispatched by
    // key presence, which is how zod v4 chooses the branch whose issue it
    // surfaces: `{"catalogUrl":"not a url"}` answers the URL branch's own
    // sentence ("Invalid URL"), not the union's blanket. A body naming
    // neither key fails both branches and gets the blanket.
    if obj.contains_key("app") || obj.contains_key("enabled") {
        let app = match crate::body::string_member(obj, "app", 1, usize::MAX) {
            Ok(a) => a,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        let enabled = match crate::body::boolean_member(obj, "enabled") {
            Ok(e) => e,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        if let Err(msg) = set_app_enabled(&state.pg, &sb, &app, enabled).await {
            return house_error(StatusCode::BAD_REQUEST, &msg);
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
        return Json(serde_json::json!({ "ok": true })).into_response();
    }
    let next_url = match obj.get("catalogUrl") {
        None | Some(Value::Null) => None,
        Some(Value::String(u)) => {
            if !url_ok(u) {
                return house_error(StatusCode::BAD_REQUEST, "Invalid URL");
            }
            Some(u.clone())
        }
        Some(v) => {
            return house_error(
                StatusCode::BAD_REQUEST,
                &crate::body::string_msg(crate::body::zod_type_name(v)),
            );
        }
    };
    if let Err(msg) = set_catalog_url(&state.pg, next_url.as_deref()).await {
        return house_error(StatusCode::BAD_REQUEST, &msg);
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
    Json(serde_json::json!({ "ok": true, "catalogUrl": url })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
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
    // z.object({ installUrl: z.string().url(), slug: z.string().min(1).max(64).optional() })
    let install_url = match obj.get("installUrl") {
        None => {
            return house_error(
                StatusCode::BAD_REQUEST,
                &crate::body::string_msg("undefined"),
            );
        }
        Some(v) => match v.as_str() {
            Some(u) if url_ok(u) => u.to_string(),
            Some(_) => return house_error(StatusCode::BAD_REQUEST, "Invalid URL"),
            None => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    &crate::body::string_msg(crate::body::zod_type_name(v)),
                );
            }
        },
    };
    let slug_override = match crate::body::optional_string_member(obj, "slug", 64) {
        Ok(s) => s,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match install_app_from_git(&state.pg, &install_url, slug_override.as_deref()).await {
        Ok((slug, pending_build)) => {
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
            Json(serde_json::json!({ "slug": slug, "pendingBuild": pending_build })).into_response()
        }
        Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[admin/apps] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.object({ app: z.string().min(1), wipeData: z.boolean().optional() })
    let app = match crate::body::string_member(obj, "app", 1, usize::MAX) {
        Ok(a) => a,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let wipe_data = match crate::body::optional_boolean_member(obj, "wipeData") {
        Ok(w) => w.unwrap_or(false),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(msg) = uninstall_app(&state.pg, &sb, &app).await {
        return house_error(StatusCode::BAD_REQUEST, &msg);
    }
    if wipe_data && let Err(e) = wipe_app_data(&state.pg, &app).await {
        tracing::error!("[admin/apps] data wipe failed: {e}");
        return thrown_internal_error();
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
    Json(serde_json::json!({ "ok": true })).into_response()
}
