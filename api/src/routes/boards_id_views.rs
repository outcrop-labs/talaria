// /api/boards/{id}/views — port of ui/src/routes/api/boards.$id.views.ts.
// Saved board views: named filter/layout presets shared with the board.
// GET → the board's views (any member); POST → create; PUT → rename/update
// config; DELETE → remove (owner/editor). Config is the board URL's search
// state verbatim (view/group/q/facets) — the client owns its meaning, and
// unknown keys STRIP on the way in (zod's default), so the stored jsonb is
// only ever the eight known shapes.

use crate::boards::{board_role, can_edit};
use crate::body::{
    as_object, optional_enum_member, optional_max_string_member, optional_string_member, parse,
    string_member, uuid_member,
};
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

const VIEW_KINDS: &[&str] = &["board", "list", "gantt"];

/// The saved view row (TS's ROW select): id, boardId, name, config, createdBy,
/// position, createdAt, updatedAt.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BoardView {
    id: String,
    board_id: String,
    name: String,
    config: Value,
    created_by: String,
    position: i32,
    created_at: String,
    updated_at: String,
}

type ViewRow = (String, String, String, Value, String, i32, i64, i64);

fn view_of(
    (id, board_id, name, config, created_by, position, created_ms, updated_ms): ViewRow,
) -> BoardView {
    BoardView {
        id,
        board_id,
        name,
        config,
        created_by,
        position,
        created_at: crate::agent_auth::epoch_ms_to_iso(created_ms),
        updated_at: crate::agent_auth::epoch_ms_to_iso(updated_ms),
    }
}

/// views.ts's Config — eight optional members, no minimums (the empty string
/// is a legal "clear" on every one of them), zipped back into a JSON object
/// of only the PRESENT keys in schema order: what z.object's strip-and-keep
/// leaves TS to hand postgres.
fn validate_config(obj: &Map<String, Value>) -> Result<Value, String> {
    let mut clean = Map::new();
    if let Some(v) = optional_enum_member(obj, "view", VIEW_KINDS)? {
        clean.insert("view".into(), Value::String(v));
    }
    for (key, max) in [
        ("group", 20),
        ("q", 200),
        ("status", 300),
        ("assignee", 2000),
        ("priority", 100),
        ("label", 1000),
        ("due", 20),
    ] {
        if let Some(v) = optional_max_string_member(obj, key, max)? {
            clean.insert(key.into(), Value::String(v));
        }
    }
    Ok(Value::Object(clean))
}

/// Config arrives as an OBJECT or not at all — zod's type message before any
/// member check, with absent (undefined) and null spelled apart exactly as
/// zod answers them.
fn config_member(obj: &Map<String, Value>) -> Result<Value, String> {
    match obj.get("config") {
        None => Err("Invalid input: expected object, received undefined".into()),
        Some(v) => {
            let m = v.as_object().ok_or_else(|| {
                format!(
                    "Invalid input: expected object, received {}",
                    crate::body::zod_type_name(v)
                )
            })?;
            validate_config(m)
        }
    }
}

/// The optional spelling PUT uses (`config: Config.optional()`).
fn optional_config_member(obj: &Map<String, Value>) -> Result<Option<Value>, String> {
    match obj.get("config") {
        None => Ok(None),
        Some(_) => config_member(obj).map(Some),
    }
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "GET views", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on views failed: {e}");
            return thrown_internal_error();
        }
    }
    let rows: Vec<ViewRow> = match sqlx::query_as(
        "select id::text, board_id::text, name, config, created_by, position, \
                (trunc(extract(epoch from created_at) * 1000))::bigint, \
                (trunc(extract(epoch from updated_at) * 1000))::bigint \
         from board_views where board_id = $1::uuid order by position, created_at",
    )
    .bind(&id)
    .fetch_all(&state.pg)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] view list failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "views": rows.into_iter().map(view_of).collect::<Vec<_>>() })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "POST views", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(role) if can_edit(role.as_deref()) => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on view post failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 60) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let config = match config_member(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // createdBy is the human-readable attribution: email, else name, else
    // 'user' — the same actor ladder the audit log climbs.
    let created_by = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    let row: ViewRow = match sqlx::query_as(
        "insert into board_views (board_id, name, config, created_by, position) \
         values ($1::uuid, $2, $3, $4, \
                 coalesce((select max(position) + 1 from board_views where board_id = $1::uuid), 0)) \
         returning id::text, board_id::text, name, config, created_by, position, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint, \
                   (trunc(extract(epoch from updated_at) * 1000))::bigint",
    )
    .bind(&id)
    .bind(&name)
    .bind(&config)
    .bind(&created_by)
    .fetch_one(&state.pg)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[boards] view create failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "view": view_of(row) })).into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "PUT views", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(role) if can_edit(role.as_deref()) => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on view put failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let view_id = match uuid_member(obj, "viewId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match optional_string_member(obj, "name", 60) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let config = match optional_config_member(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Two separate statements, TS's shape: a name-only PUT does not touch
    // the config, and vice versa.
    if let Some(name) = &name
        && let Err(e) = sqlx::query(
            "update board_views set name = $1, updated_at = now() \
             where id = $2::uuid and board_id = $3::uuid",
        )
        .bind(name)
        .bind(&view_id)
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[boards] view rename failed: {e}");
        return thrown_internal_error();
    }
    if let Some(config) = &config
        && let Err(e) = sqlx::query(
            "update board_views set config = $1, updated_at = now() \
             where id = $2::uuid and board_id = $3::uuid",
        )
        .bind(config)
        .bind(&view_id)
        .bind(&id)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[boards] view config write failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "DELETE views", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(role) if can_edit(role.as_deref()) => {}
        Ok(_) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on view delete failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let view_id = match uuid_member(obj, "viewId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) =
        sqlx::query("delete from board_views where id = $1::uuid and board_id = $2::uuid")
            .bind(&view_id)
            .bind(&id)
            .execute(&state.pg)
            .await
    {
        tracing::error!("[boards] view delete failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(v: Value) -> Result<Value, String> {
        config_member(json!({ "config": v }).as_object().unwrap())
    }

    #[test]
    fn config_strips_unknowns_and_keeps_present_keys_in_schema_order() {
        let v = config(json!({
            "q": "opsoncall",
            "view": "gantt",
            "junk": "stripped",
            "due": ""
        }))
        .unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        assert_eq!(keys, ["view", "q", "due"]);
        // The empty string SURVIVES — it means "cleared", not "absent".
        assert_eq!(v["due"], json!(""));
        // Unknown keys never reach the stored jsonb.
        assert!(v.get("junk").is_none());
    }

    #[test]
    fn config_validates_the_enum_and_bounds() {
        assert_eq!(
            config(json!({ "view": "calendar" })).unwrap_err(),
            format!("Invalid option: expected one of \"board\"|\"list\"|\"gantt\"")
        );
        assert_eq!(
            config(json!({ "view": 5 })).unwrap_err(),
            "Invalid option: expected one of \"board\"|\"list\"|\"gantt\""
        );
        assert_eq!(
            config(json!({ "q": "x".repeat(201) })).unwrap_err(),
            "Too big: expected string to have <=200 characters"
        );
        // Type check before members, zod's order.
        assert_eq!(
            config(json!([])).unwrap_err(),
            "Invalid input: expected object, received array"
        );
        // config is REQUIRED on POST — absent answers the object type message.
        let body = json!({ "name": "Mine" });
        assert_eq!(
            config_member(body.as_object().unwrap()).unwrap_err(),
            "Invalid input: expected object, received undefined"
        );
    }
}
