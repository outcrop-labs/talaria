// /api/admin/apps/catalogs. The marketplace catalogs Manage → Apps →
// Discover reads — a list the operator fills (talaria-apps `catalog`).
// GET → the list, in order; POST { name, url } → add one at the end, then
// read it once and say how many apps it lists or why it could not be read;
// PUT { id, name?, url?, enabled? } → edit one, or { order } → reorder the
// whole list; DELETE { id } → remove one. Reads are open to anyone granted the
// /apps Manage view; every write is admin-only, because a catalog decides
// which repositories this instance offers to install code from. Nothing here
// re-adds a default: an empty list is the operator's choice.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_apps::{
    CatalogError, CatalogPatch, create_catalog, delete_catalog, fetch_catalog_one, list_catalogs,
    reorder_catalogs, update_catalog,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    optional_boolean_member, optional_max_string_member, parse, string_member, uuid_array_member,
    uuid_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_admin, require_view};
use talaria_state::AppState;

fn refused(e: CatalogError) -> Response {
    match e {
        CatalogError::Invalid(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
        CatalogError::Duplicate(msg) => house_error(StatusCode::CONFLICT, &msg),
        CatalogError::NotFound => house_error(StatusCode::NOT_FOUND, "no such catalog"),
        CatalogError::Db(e) => internal("[admin/apps/catalogs] write failed", e),
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_view(&state, &headers, "/apps").await?;
    match list_catalogs(&state.pg).await {
        Ok(catalogs) => Ok(Json(json!({ "catalogs": catalogs })).into_response()),
        Err(e) => Ok(internal("[admin/apps/catalogs] list read failed", e)),
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // What Discover calls this catalog, next to each app it lists.
    let name = match string_member(obj, "name", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The index's https URL (`{"apps": [...]}`).
    let url = match string_member(obj, "url", 1, 4000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let actor = actor_of(&user);
    let catalog = match create_catalog(&state.pg, &name, &url, &actor).await {
        Ok(c) => c,
        Err(e) => return Ok(refused(e)),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "app.catalog-add",
            target_type: "app_catalog",
            target_id: Some(&catalog.id),
            target_label: Some(&catalog.name),
            before: None,
            after: Some(json!({ "name": catalog.name, "url": catalog.url })),
        },
    )
    .await;
    // Read it once, so the admin learns now whether it works — a catalog
    // that cannot be read is still added (the host may be down for a minute).
    let (apps, error) = match fetch_catalog_one(&catalog.url).await {
        Ok(list) => (Some(list.len()), None),
        Err(e) => (None, Some(e)),
    };
    Ok(
        Json(json!({ "ok": true, "catalog": catalog, "apps": apps, "error": error }))
            .into_response(),
    )
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let actor = actor_of(&user);
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Two bodies, dispatched by key presence: { order } puts the whole list in
    // that order (every catalog's id, exactly once — an earlier catalog wins
    // a slug two of them list), and { id, name?, url?, enabled? } edits one.
    if obj.contains_key("order") {
        let order = match uuid_array_member(obj, "order", 50) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        let catalogs = match reorder_catalogs(&state.pg, &order).await {
            Ok(c) => c,
            Err(e) => return Ok(refused(e)),
        };
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "app.catalog-reorder",
                target_type: "app_catalog",
                target_id: None,
                target_label: None,
                before: None,
                after: Some(json!({ "order": order })),
            },
        )
        .await;
        return Ok(Json(json!({ "ok": true, "catalogs": catalogs })).into_response());
    }
    let id = match uuid_member(obj, "id") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let name = match optional_max_string_member(obj, "name", 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let url = match optional_max_string_member(obj, "url", 4000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // false: Discover stops reading it, and the row stays in the list.
    let enabled = match optional_boolean_member(obj, "enabled") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if name.is_none() && url.is_none() && enabled.is_none() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "the body must change name, url or enabled",
        ));
    }
    let patch = CatalogPatch { name, url, enabled };
    let catalog = match update_catalog(&state.pg, &id, &patch).await {
        Ok(c) => c,
        Err(e) => return Ok(refused(e)),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "app.catalog-edit",
            target_type: "app_catalog",
            target_id: Some(&catalog.id),
            target_label: Some(&catalog.name),
            before: None,
            after: Some(json!({
                "name": catalog.name,
                "url": catalog.url,
                "enabled": catalog.enabled,
            })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true, "catalog": catalog })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let id = match uuid_member(obj, "id") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let removed = match delete_catalog(&state.pg, &id).await {
        Ok(c) => c,
        Err(e) => return Ok(refused(e)),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "app.catalog-remove",
            target_type: "app_catalog",
            target_id: Some(&removed.id),
            target_label: Some(&removed.name),
            before: Some(json!({ "name": removed.name, "url": removed.url })),
            after: None,
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}
