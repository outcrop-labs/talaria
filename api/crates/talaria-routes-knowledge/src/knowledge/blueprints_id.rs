// /api/blueprints/{id}. One blueprint: PUT → edit, DELETE → remove — your
// own with a session alone, a SHARED one (owner null) with templates.manage.
// Somebody else's personal blueprint answers 404 and never 403: a row you
// cannot see does not exist, and a 403 would confirm the id is somebody's.
//
// THE GATE IS THE READ, which is why the 404 comes out right. The handler
// resolves the id against the caller before it looks at the body at all, and
// "no such row" and "not yours" collapse into the same answer there — the
// distinction is never reachable from outside. The one refusal that IS a 403
// is the shared row: everybody can see it, so what they are missing to
// change it is a permission, and that is what the answer says.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::PgPool;
use talaria_body::{
    optional_boolean_member, optional_max_string_member, parse, trimmed_string_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

use super::blueprints::{author, get_blueprint};

/// What the caller is allowed to do with this id. `Missing` covers both "no
/// such row" and "somebody else's personal row": the handler answers them
/// identically and the distinction must not leak.
enum Reach {
    Missing,
    /// The caller's own personal blueprint.
    Mine,
    /// The org's — readable by all, writable with templates.manage.
    Shared,
}

/// Resolve the id against the caller. One read, used by both verbs.
async fn reach(pg: &PgPool, id: &str, user_id: &str) -> Result<Reach, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select owner_user_id::text from blueprints where id = $1::uuid")
            .bind(id)
            .fetch_optional(pg)
            .await?;
    Ok(match row {
        None => Reach::Missing,
        Some((None,)) => Reach::Shared,
        Some((Some(owner),)) if owner == user_id => Reach::Mine,
        Some(_) => Reach::Missing,
    })
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let user = require_user(&state, &headers).await?;
    let reach = match reach(&state.pg, &id, &user.id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[blueprints] read before update failed", e)),
    };
    if matches!(reach, Reach::Missing) {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let shared = match optional_boolean_member(obj, "shared") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The permission covers both directions across the line: editing a row
    // that IS shared (including un-sharing it) and sharing one that is not.
    // Setting `shared: false` on a row already personal names no org-wide
    // blueprint and needs nothing.
    if matches!(reach, Reach::Shared) || shared == Some(true) {
        super::blueprints::require_share_rights(&state, &headers).await?;
    }
    let name = match optional_trimmed(obj, "name", 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let text = match optional_max_string_member(obj, "body", 20_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(name) = name.as_deref()
        && let Err(e) = set_name(&state.pg, &id, name).await
    {
        return Ok(internal("[blueprints] name update failed", e));
    }
    if let Some(text) = text.as_deref()
        && let Err(e) = set_body(&state.pg, &id, text).await
    {
        return Ok(internal("[blueprints] body update failed", e));
    }
    // Sharing hands the row to the org (owner null); un-sharing hands it to
    // whoever did it — there is no previous owner to give it back to, and a
    // row with no owner that nobody can see would be unreachable.
    if let Some(shared) = shared {
        let owner = if shared { None } else { Some(user.id.as_str()) };
        if let Err(e) = set_owner(&state.pg, &id, owner, author(&user)).await {
            return Ok(internal("[blueprints] share update failed", e));
        }
    }
    Ok(match get_blueprint(&state.pg, &id).await {
        Ok(Some(blueprint)) => Json(json!({ "blueprint": blueprint })).into_response(),
        // Deleted between the write and the read-back — rare, and the honest
        // answer is the one the caller would get asking again.
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => internal("[blueprints] read after update failed", e),
    })
}

/// Optional trimmed string — the trim runs before the bounds, and only when
/// the key is present.
fn optional_trimmed(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(_) => trimmed_string_member(obj, key, 1, max).map(Some),
    }
}

/// Each field is its own statement, each restamping `updated_at` — the patch
/// only moves what the request actually named (the templates library's shape,
/// for the same reason: a merged UPDATE would have to re-send values the
/// caller never sent).
async fn set_name(pg: &PgPool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("update blueprints set name = $1, updated_at = now() where id = $2::uuid")
        .bind(name)
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

async fn set_body(pg: &PgPool, id: &str, body: &str) -> Result<(), sqlx::Error> {
    sqlx::query("update blueprints set body = $1, updated_at = now() where id = $2::uuid")
        .bind(body)
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

/// Crossing the line. `created_by` is restamped with whoever moved it: who
/// put a blueprint in front of the whole org is the useful attribution, not
/// who first typed it privately.
async fn set_owner(
    pg: &PgPool,
    id: &str,
    owner: Option<&str>,
    by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update blueprints set owner_user_id = $1::uuid, created_by = $2, updated_at = now() \
         where id = $3::uuid",
    )
    .bind(owner)
    .bind(by)
    .bind(id)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let user = require_user(&state, &headers).await?;
    let reach = match reach(&state.pg, &id, &user.id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[blueprints] read before delete failed", e)),
    };
    match reach {
        Reach::Missing => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Reach::Shared => {
            super::blueprints::require_share_rights(&state, &headers).await?;
        }
        Reach::Mine => {}
    }
    Ok(
        match sqlx::query("delete from blueprints where id = $1::uuid")
            .bind(&id)
            .execute(&state.pg)
            .await
        {
            Ok(_) => Json(json!({ "ok": true })).into_response(),
            Err(e) => internal("[blueprints] delete failed", e),
        },
    )
}
