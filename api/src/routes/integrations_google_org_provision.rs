// /api/integrations/google/org/provision — port of
// ui/src/routes/api/integrations/google.org.provision.ts. The org workspace
// provisioning surface (admin). GET → what the panel draws: scope readiness,
// the provisioned container ids, and every agent's effective send address.
// POST → run the requested provisions, per-item outcomes back.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::body::{as_object, optional_boolean_member, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::google_org::{get_org_connection_status, get_org_email};
use crate::google_pending_actions::agent_from_address;
use crate::google_provisioning::{provision_workspace, provisioning_readiness};
use crate::session::require_admin;
use crate::state::AppState;

// GET → readiness, container ids, and every ORG agent's effective address
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    // TS runs four reads concurrently (Promise.all); each fails the whole
    // route on error, and none writes, so sequential reads answer the same.
    let readiness = match provisioning_readiness(&state.pg).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[integrations/google/org] readiness read failed: {e}");
            return thrown_internal_error();
        }
    };
    let status = match get_org_connection_status(&state.pg).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[integrations/google/org] status read failed: {e}");
            return thrown_internal_error();
        }
    };
    let org_email = match get_org_email(&state.pg).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[integrations/google/org] org email read failed: {e}");
            return thrown_internal_error();
        }
    };
    // listAgentDefs' row, cut to the five columns this read uses, still
    // ordered by slug asc. Personal assistants are filtered out — they send
    // as their owner, where no org alias applies.
    type DefRow = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let defs: Vec<DefRow> = match sqlx::query_as(
        "select model, slug, display_name, email_alias, owner_user_id::text from agent_defs order by slug asc",
    )
    .fetch_all(&state.pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[integrations/google/org] agent defs read failed: {e}");
            return thrown_internal_error();
        }
    };
    let agents: Vec<Value> = defs
        .into_iter()
        // `.filter((d) => !d.ownerUserId)` — org agents only
        .filter(|(_, _, _, _, owner_user_id)| owner_user_id.is_none())
        .map(|(model, slug, display_name, email_alias, _owner)| {
            let effective = agent_from_address(&slug, email_alias.as_deref(), org_email.as_deref());
            json!({
                "model": model,
                "displayName": display_name,
                "slug": slug,
                "alias": email_alias,
                "effective": effective,
            })
        })
        .collect();
    Json(json!({
        "readiness": readiness,
        "orgEmail": org_email,
        "calendarId": status.targets.calendar_id,
        "sharedDriveId": status.targets.shared_drive_id,
        "agents": agents,
    }))
    .into_response()
}

// POST → run the requested provisions ({calendar?, drive?}, at least one)
pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let calendar = match optional_boolean_member(obj, "calendar") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let drive = match optional_boolean_member(obj, "drive") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let (calendar, drive) = (calendar.unwrap_or(false), drive.unwrap_or(false));
    if !calendar && !drive {
        return house_error(StatusCode::BAD_REQUEST, "bad request");
    }
    let sb = state.secretbox().await.unwrap_or_default();
    match provision_workspace(
        &state.pg,
        &sb,
        crate::google_provisioning::ProvisionRequest { calendar, drive },
        now_ms(),
    )
    .await
    {
        Ok(result) => {
            // Each key rides only when requested — TS's undefined members are
            // dropped by JSON.stringify, not written as null.
            let mut body = serde_json::Map::new();
            if let Some(c) = result.calendar {
                body.insert(
                    "calendar".into(),
                    serde_json::to_value(&c).unwrap_or(Value::Null),
                );
            }
            if let Some(d) = result.drive {
                body.insert(
                    "drive".into(),
                    serde_json::to_value(&d).unwrap_or(Value::Null),
                );
            }
            Json(Value::Object(body)).into_response()
        }
        // provision_workspace folds its throws into per-item outcomes; the
        // Err here is a DB write it could not fold — the route's throw.
        Err(e) => {
            tracing::error!("[integrations/google/org] provision failed: {e}");
            thrown_internal_error()
        }
    }
}

/// Date.now() — the one clock provisioning reads.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
