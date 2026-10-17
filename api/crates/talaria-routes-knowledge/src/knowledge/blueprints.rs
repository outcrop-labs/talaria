// /api/blueprints. The asks somebody sends again — a named piece of text a
// person pastes into the composer. GET → the caller's own plus the shared
// ones (any member). POST → create: a personal blueprint needs only a
// session; a SHARED one (owner null, the org's) needs templates.manage.
//
// That permission is the one already governing org-wide content — the ticket
// and plan skeletons everyone starts from — so a shared blueprint is gated
// by the same people who curate those, and nothing new had to be invented.
//
// The store lives here rather than in a crate of its own because there is one
// table, four verbs and no second consumer: `blueprints_id.rs` reads the
// handful of `pub(crate)` helpers below and nothing else does. The day an
// agent tool or the brief reads blueprints, this moves to `talaria-blueprints`
// the way `talaria-templates` did.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_body::{
    optional_boolean_member, optional_max_string_member, parse, trimmed_string_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{require_perm, require_user};
use talaria_state::AppState;

/// The gate on the SHARED path only. A personal blueprint needs a session and
/// nothing more; making one org-wide, or changing or removing one that is,
/// needs the same permission templates take. Named so the generated
/// reference reads the conditional gate as what it is — `session` with a
/// permission on one branch — instead of overstating every verb as perm-gated.
pub(crate) async fn require_share_rights(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), Response> {
    require_perm(state, headers, "templates.manage")
        .await
        .map(|_| ())
}

/// A blueprint on the wire. `shared` is the column `owner_user_id` read as a
/// boolean — the client's question is "is this everyone's?", and making it
/// derive the answer from a null id would put the access model in the
/// browser. `ownerUserId` rides along anyway: a shared row is the only one
/// whose owner the viewer is not.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blueprint {
    pub id: String,
    pub name: String,
    pub body: String,
    pub shared: bool,
    pub owner_user_id: Option<String>,
    pub surface: String,
    pub sort: i32,
    pub created_at: String,
    pub updated_at: String,
}

/// The column list every read shares, and the only place the epoch-ms
/// conversion is spelled — `created_at`/`updated_at` go out as ISO strings,
/// which is what every other row in this api answers.
const COLS: &str = "id::text, name, body, owner_user_id::text, surface, sort, \
                    (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
                    (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms";

type Row = (
    String,
    String,
    String,
    Option<String>,
    String,
    i32,
    i64,
    i64,
);

impl From<Row> for Blueprint {
    fn from(r: Row) -> Self {
        let (id, name, body, owner_user_id, surface, sort, created_ms, updated_ms) = r;
        Blueprint {
            shared: owner_user_id.is_none(),
            id,
            name,
            body,
            owner_user_id,
            surface,
            sort,
            created_at: epoch_ms_to_iso(created_ms),
            updated_at: epoch_ms_to_iso(updated_ms),
        }
    }
}

/// One row by id, whoever owns it — the CALLER's right to see it is decided
/// by `owner_user_id` at the door above (a personal row that is not yours
/// reads as absent), so this read is deliberately unfiltered: the handler
/// needs the owner to make that decision.
pub(crate) async fn get_blueprint(pg: &PgPool, id: &str) -> Result<Option<Blueprint>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this module's own COLS list.
    let sql = format!("select {COLS} from blueprints where id = $1::uuid");
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(Blueprint::from))
}

/// The one read there is: this person's own, plus the org's. Ordered by the
/// hand-order then the name, which is the order the picker shows and never
/// varies.
async fn list_blueprints(pg: &PgPool, user_id: &str) -> Result<Vec<Blueprint>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this module's own COLS list.
    let sql = format!(
        "select {COLS} from blueprints \
         where owner_user_id = $1::uuid or owner_user_id is null \
         order by sort asc, name asc"
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(Blueprint::from).collect())
}

/// Insert. `owner` is None for a shared blueprint — the caller has already
/// been held to templates.manage for that.
async fn create_blueprint(
    pg: &PgPool,
    name: &str,
    body: &str,
    owner: Option<&str>,
    created_by: &str,
) -> Result<Blueprint, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this module's own COLS list.
    let sql = format!(
        "insert into blueprints (name, body, owner_user_id, created_by) \
         values ($1, $2, $3::uuid, $4) returning {COLS}"
    );
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(name)
        .bind(body)
        .bind(owner)
        .bind(created_by)
        .fetch_one(pg)
        .await?;
    Ok(Blueprint::from(row))
}

/// Who a write names itself as: the email, the display name, or "user" — the
/// same fallback chain the templates library uses. `created_by` is text and
/// not a user reference on purpose: it survives the person leaving.
pub(crate) fn author(user: &talaria_session::SessionUser) -> &str {
    user.email
        .as_deref()
        .or(user.name.as_deref())
        .unwrap_or("user")
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match list_blueprints(&state.pg, &user.id).await {
        Ok(blueprints) => Json(json!({ "blueprints": blueprints })).into_response(),
        Err(e) => internal("[blueprints] list failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // `shared` is read FIRST and the permission settled before the rest is
    // validated: a member who may not publish an org-wide blueprint should
    // learn that from the answer, not be told their name is too long first
    // and discover the 403 on the retry.
    let shared = match optional_boolean_member(obj, "shared") {
        Ok(v) => v.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if shared {
        require_share_rights(&state, &headers).await?;
    }
    let name = match trimmed_string_member(obj, "name", 1, 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Max-only: the empty body is legal — a blueprint can start as a name and
    // get its text in the editor.
    let text = match optional_max_string_member(obj, "body", 20_000) {
        Ok(v) => v.unwrap_or_default(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let owner = if shared { None } else { Some(user.id.as_str()) };
    Ok(
        match create_blueprint(&state.pg, &name, &text, owner, author(&user)).await {
            Ok(blueprint) => Json(json!({ "blueprint": blueprint })).into_response(),
            Err(e) => internal("[blueprints] create failed", e),
        },
    )
}
