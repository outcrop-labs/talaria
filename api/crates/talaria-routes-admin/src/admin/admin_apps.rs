// /api/admin/apps. App administration. GET → installed apps, each with its
// triggers and their newest runs (+ ?catalog=1 for the marketplace feed,
// ?updates=1 for whether each app's source has a newer commit); an app a
// Developer Agent maintains carries `plugin` (its maintainer, owner, board
// and update policy). Reads are open to anyone granted the /apps Manage view;
// mutations (enable/disable, install, update, uninstall, catalog source, a
// plugin's update policy) and the update check stay admin-only — installing
// or updating an app adds CODE to the deployment, and a check spends a
// credential.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_apps::{
    AppHost, InstallError, InstallRequest, KeptDataChoice, PlatformCredentials, catalog_url,
    enabled_app_slugs, fetch_catalog, install_app, set_app_enabled, set_catalog_url,
    source_records, uninstall_app, update_app, update_checks,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::parse;
use talaria_error::{house_error, object_or_400};
use talaria_session::{actor_of, require_admin, require_view, secretbox_or_500};
use talaria_state::AppState;
use talaria_users::{WireBuild, app_build_status, app_builds_dir, broken_apps, discovered_apps};

/// The build status, plus what the app host's load probe made of the app's
/// `harnesses/*.ts` (`harnesses: { registered, problems }` on current.json,
/// ui/src/server/app-harnesses.ts). A harness the host skipped never disables
/// the app, so this row is where an admin learns it is missing from the
/// fitness matrix and why. Absent when the app ships none or the pointer
/// cannot be read.
fn with_harness_report(slug: &str, build: WireBuild) -> Value {
    let mut out = serde_json::to_value(build).unwrap_or(Value::Null);
    let report = std::fs::read_to_string(app_builds_dir().join(slug).join("current.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("harnesses").cloned())
        .filter(Value::is_object);
    if let (Some(obj), Some(report)) = (out.as_object_mut(), report) {
        obj.insert("harnesses".into(), report);
    }
    out
}

/// The app host as this api calls it (`talaria_apps::host`), with the
/// root-derived service credential its lifecycle routes take.
fn app_host(state: &AppState) -> AppHost {
    AppHost::new(state.cfg.secret_root.internal_token())
}

/// The enabled set changed: have the app host rewrite every enabled app's
/// harness report, so an id one app took from another shows on the app that
/// lost it (talaria_app_harnesses::refresh_app_reports). Best-effort — the
/// change itself is done, and the next load probe writes the reports too.
async fn refresh_harness_reports(state: &AppState) {
    let token = state.cfg.secret_root.internal_token();
    if let Err(e) = talaria_app_harnesses::refresh_app_reports(&token).await {
        tracing::warn!("[admin/apps] the harness reports were not refreshed: {e}");
    }
}

/// A plugin as the listing serves it: who maintains it, who it is for, where
/// its maintenance tickets go, and how its updates land. The source half
/// (repo, ref, commit) is the app's `git`.
fn plugin_of(p: &talaria_plugins::Plugin) -> Value {
    json!({
        "maintainer": p.maintainer_agent_id.as_ref().map(|id| json!({
            "id": id,
            "model": p.maintainer_model,
            "name": p.maintainer_name,
        })),
        "owner": p.owner_user_id.as_ref().map(|id| json!({ "id": id, "name": p.owner_name })),
        "board": p.board_id.as_ref().map(|id| json!({ "id": id, "name": p.board_name })),
        "taskId": p.task_id,
        "ticketRef": p.ticket_ref,
        "gitHost": p.git_host_id,
        "updatePolicy": p.update_policy,
    })
}

/// One source record as the listing serves it: where the app came from and
/// the commit it runs. The workspace secret it is read with stays out.
fn git_of(rec: &talaria_apps::SourceRecord) -> Value {
    json!({
        "url": rec.url,
        "host": rec.host,
        "repo": rec.repo,
        "ref": rec.git_ref,
        "sha": rec.sha,
        "previousSha": rec.previous_sha,
        "installedAt": rec.installed_at,
        "updatedAt": rec.updated_at,
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    // The one admin-family read that /apps Manage holders share with admins.
    let viewer = require_view(&state, &headers, "/apps").await?;
    let query = talaria_api_facades::google::oauth::query_pairs(uri.query());
    // ?updates=1 is admin-only: each check mints the app's read token. (A
    // source read with a workspace secret is never checked: that would spend
    // a use of the secret.)
    let want_updates = query.get("updates").map(String::as_str) == Some("1");
    if want_updates && viewer.role != "admin" {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "only an admin may check apps for updates",
        ));
    }
    let enabled = enabled_app_slugs(&state.pg).await;
    let sources = source_records(&state.pg).await;
    let plugins: std::collections::BTreeMap<String, Value> =
        match talaria_plugins::plugins(&state.pg).await {
            Ok(rows) => rows
                .iter()
                .map(|p| (p.app_slug.clone(), plugin_of(p)))
                .collect(),
            Err(e) => {
                tracing::warn!("[admin/apps] could not read plugins: {e}");
                Default::default()
            }
        };
    let plugin_for = |slug: &str| plugins.get(slug).cloned().unwrap_or(Value::Null);
    let source_of = |slug: &str| {
        sources
            .get(slug)
            .map(|r| (Value::String(r.url.clone()), git_of(r)))
            .unwrap_or((Value::Null, Value::Null))
    };

    let mut apps = discovered_apps()
        .into_iter()
        .map(|a| {
            let mut surfaces = serde_json::Map::new();
            if let Some(w) = &a.work {
                surfaces.insert("work".into(), json!(w));
            }
            if let Some(m) = &a.manage {
                surfaces.insert("manage".into(), json!(m));
            }
            if let Some(s) = &a.settings {
                surfaces.insert("settings".into(), json!(s));
            }
            let (source, git) = source_of(&a.slug);
            let build = with_harness_report(&a.slug, app_build_status(&a.slug));
            json!({
                "slug": a.slug,
                "name": a.name,
                "icon": a.icon,
                "description": a.description,
                "version": a.version,
                "surfaces": Value::Object(surfaces),
                "mcp": a.mcp,
                "enabled": enabled.contains(&a.slug),
                "source": source,
                "git": git,
                "build": build,
                "plugin": plugin_for(&a.slug),
            })
        })
        .collect::<Vec<_>>();
    // Each app's triggers and their newest runs (talaria-app-triggers): where
    // an admin sees that a schedule fired, and why a run failed — a failing
    // trigger never turns the app off, so this line is how it is noticed.
    for app in apps.iter_mut() {
        let slug = app["slug"].as_str().unwrap_or_default().to_string();
        let report = talaria_app_triggers::trigger_report(&state.pg, &slug).await;
        if let Some(obj) = app.as_object_mut() {
            obj.insert("triggers".into(), report);
        }
    }
    // An installed directory whose talaria.json the rules refuse: listed with
    // the reason, never enabled, and still removable.
    for b in broken_apps() {
        let (source, git) = source_of(&b.slug);
        apps.push(json!({
            "slug": b.slug,
            "name": b.slug,
            "icon": "⬡",
            "description": "",
            "version": "",
            "surfaces": {},
            "mcp": false,
            "enabled": false,
            "source": source,
            "git": git,
            "build": app_build_status(&b.slug),
            "broken": b.reason,
            "plugin": plugin_for(&b.slug),
        }));
    }
    // ?catalog=1
    let want_catalog = query.get("catalog").map(String::as_str) == Some("1");
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
    // ?updates=1 (admin): `git ls-remote` against each app's recorded ref.
    let updates = if want_updates {
        let sb = secretbox_or_500(&state, "[admin/apps] secretbox unavailable").await?;
        let actor = actor_of(&viewer);
        let creds = PlatformCredentials {
            pg: &state.pg,
            sb: &sb,
            actor: &actor,
        };
        serde_json::to_value(update_checks(&state.pg, &creds).await).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    Ok(Json(json!({
        "apps": apps,
        "catalog": catalog,
        "catalogUrl": catalog_url(&state.pg).await,
        "updates": updates,
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
    // Four bodies on one PUT, dispatched by key presence: { app, update }
    // updates an app to its source's newer commit, { app, updatePolicy } sets
    // a plugin's update policy, { app, enabled } toggles an app, { catalogUrl }
    // (nullable — null resets to the default source) sets the catalog source.
    // A body naming NONE of the keys answers 400 rather than silently falling
    // into the catalog arm and resetting the source; a catalogUrl that is a
    // string but not a URL answers the field's own sentence ("Invalid URL").
    if obj.contains_key("update") {
        let app = match talaria_body::string_member(obj, "app", 1, usize::MAX) {
            Ok(a) => a,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        match talaria_body::boolean_member(obj, "update") {
            Ok(true) => {}
            Ok(false) => {
                return Ok(house_error(StatusCode::BAD_REQUEST, "update must be true"));
            }
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
        // ref: follow another branch, tag or commit from now on (default: the recorded one).
        let git_ref = match talaria_body::optional_string_member(obj, "ref", 200) {
            Ok(r) => r,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        let creds = PlatformCredentials {
            pg: &state.pg,
            sb: &sb,
            actor: &actor,
        };
        let host = app_host(&state);
        let outcome =
            match update_app(&state.pg, &creds, &host, &app, git_ref.as_deref(), &actor).await {
                Ok(o) => o,
                Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
            };
        if !outcome.up_to_date {
            log_audit(
                &state.pg,
                AuditEntry {
                    actor: &actor,
                    action: if outcome.ok {
                        "app.update"
                    } else {
                        "app.update-failed"
                    },
                    target_type: "app",
                    target_id: Some(&app),
                    target_label: outcome.error.as_deref(),
                    before: Some(json!({ "sha": outcome.from })),
                    // The commit apps/<slug> holds now: the new one when it
                    // landed, or when the swap back failed (not rolled back).
                    after: Some(json!({
                        "sha": if outcome.ok || !outcome.rolled_back { outcome.to.clone() } else { outcome.from.clone() },
                        "ref": outcome.git_ref,
                        "attempted": outcome.to,
                        "rolledBack": outcome.rolled_back,
                    })),
                },
            )
            .await;
        }
        return Ok(Json(json!({
            "ok": outcome.ok,
            "slug": outcome.slug,
            "from": outcome.from,
            "to": outcome.to,
            "ref": outcome.git_ref,
            "upToDate": outcome.up_to_date,
            "rolledBack": outcome.rolled_back,
            "error": outcome.error,
            "build": outcome.build,
        }))
        .into_response());
    }
    if obj.contains_key("updatePolicy") {
        let app = match talaria_body::string_member(obj, "app", 1, usize::MAX) {
            Ok(a) => a,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        // approve: every update of this plugin waits for an admin's approval;
        // auto: it does not.
        let policy =
            match talaria_body::enum_member(obj, "updatePolicy", &talaria_plugins::UPDATE_POLICIES)
            {
                Ok(p) => p,
                Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
            };
        let previous = match talaria_plugins::set_update_policy(&state.pg, &app, &policy).await {
            Ok(Some(previous)) => previous,
            Ok(None) => {
                return Ok(house_error(
                    StatusCode::NOT_FOUND,
                    &format!("{app} is not a plugin: no agent maintains it"),
                ));
            }
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "app.update-policy",
                target_type: "app",
                target_id: Some(&app),
                target_label: Some(&policy),
                before: Some(json!({ "updatePolicy": previous })),
                after: Some(json!({ "updatePolicy": policy })),
            },
        )
        .await;
        return Ok(Json(json!({ "ok": true, "updatePolicy": policy })).into_response());
    }
    if obj.contains_key("app") || obj.contains_key("enabled") {
        let app = match talaria_body::string_member(obj, "app", 1, usize::MAX) {
            Ok(a) => a,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        let enabled = match talaria_body::boolean_member(obj, "enabled") {
            Ok(e) => e,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        if enabled && let Some(b) = broken_apps().into_iter().find(|b| b.slug == app) {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!("{app} cannot be enabled: {}", b.reason),
            ));
        }
        if let Err(msg) = set_app_enabled(&state.pg, &sb, &app, enabled).await {
            return Ok(house_error(StatusCode::BAD_REQUEST, &msg));
        }
        refresh_harness_reports(&state).await;
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
        return Ok(Json(json!({ "ok": true })).into_response());
    }
    let next_url = match obj.get("catalogUrl") {
        None => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "body must name app + update, app + updatePolicy, app + enabled, or catalogUrl",
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
    Ok(Json(json!({ "ok": true, "catalogUrl": url })).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let actor = actor_of(&user);
    let sb = secretbox_or_500(&state, "[admin/apps] secretbox unavailable").await?;
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
    // ref: a branch, tag or commit to install (default: the default branch).
    let git_ref = match talaria_body::optional_string_member(obj, "ref", 200) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // secret: a workspace secret holding an https token for a host other than
    // the GitHub connection's (its Spendable at list must name the host).
    let secret = match talaria_body::optional_string_member(obj, "secret", 200) {
        Ok(s) => s,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // keptData: data an earlier install of the slug kept — "adopt" gives it
    // to this app, "wipe" deletes it first (default: adopt only its own).
    let kept_data = match talaria_body::optional_enum_member(obj, "keptData", &["adopt", "wipe"]) {
        Ok(k) => k.map(|k| {
            if k == "wipe" {
                KeptDataChoice::Wipe
            } else {
                KeptDataChoice::Adopt
            }
        }),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let creds = PlatformCredentials {
        pg: &state.pg,
        sb: &sb,
        actor: &actor,
    };
    let host = app_host(&state);
    let req = InstallRequest {
        url: &install_url,
        slug: slug_override.as_deref(),
        git_ref: git_ref.as_deref(),
        secret: secret.as_deref(),
        kept_data,
        actor: &actor,
    };
    Ok(
        match install_app(&state.pg, &creds, Some(&host), req).await {
            Ok(installed) => {
                log_audit(
                    &state.pg,
                    AuditEntry {
                        actor: &actor,
                        action: "app.install",
                        target_type: "app",
                        target_id: Some(&installed.slug),
                        target_label: Some(&install_url),
                        before: None,
                        after: Some(json!({
                            "sha": installed.sha,
                            "ref": git_ref,
                            "keptData": installed.kept_data,
                            "wipedData": installed.wiped_data,
                        })),
                    },
                )
                .await;
                Json(json!({
                    "slug": installed.slug,
                    "sha": installed.sha,
                    "keptData": installed.kept_data,
                    "wipedData": installed.wiped_data,
                    "build": installed.build,
                    "buildError": installed.build_error,
                }))
                .into_response()
            }
            // Data kept from another repository (or an unrecorded one)
            // stands in the way of an install that did not choose: 409, with
            // `conflict: {slug, keptFrom, installing}` naming both, and
            // Manage → Apps offers the two choices.
            Err(InstallError {
                error,
                kept_data: Some(conflict),
            }) => (
                StatusCode::CONFLICT,
                Json(json!({ "error": error, "conflict": conflict })),
            )
                .into_response(),
            Err(InstallError { error, .. }) => house_error(StatusCode::BAD_REQUEST, &error),
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
    // app: required, min 1 char; wipeData: optional boolean — true deletes
    // the data the app stored, absent or false keeps it for a reinstall.
    let app = match talaria_body::string_member(obj, "app", 1, usize::MAX) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let wipe_data = match talaria_body::optional_boolean_member(obj, "wipeData") {
        Ok(w) => w.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let host = app_host(&state);
    let data = match uninstall_app(&state.pg, &sb, Some(&host), &app, wipe_data).await {
        Ok(d) => d,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    refresh_harness_reports(&state).await;
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "app.uninstall",
            target_type: "app",
            target_id: Some(&app),
            target_label: None,
            before: None,
            after: Some(json!({ "wipeData": wipe_data, "data": data })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true, "data": data })).into_response())
}
