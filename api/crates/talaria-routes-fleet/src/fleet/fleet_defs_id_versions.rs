// /api/fleet/defs/{id}/versions. GET → an agent definition's full version
// history (admin). POST { revertTo } → re-publish an old version's payload
// as a NEW version (history is append-only; a revert is itself a tracked
// change).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_defs::{
    AgentVersionRow, NewVersion, add_version_if_changed, get_agent_def_wire, list_versions,
};
use talaria_body::{NumKind, number_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_perm;
use talaria_state::AppState;

fn version_wire(v: &AgentVersionRow) -> Value {
    json!({
        "id": v.id,
        "agentId": v.agent_id,
        "version": v.version,
        "soul": v.soul,
        "config": v.config,
        "note": v.note,
        "createdBy": v.created_by,
        "createdAt": v.created_at,
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let def = match get_agent_def_wire(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => return Ok(internal("[fleet/defs/versions] def read failed", e)),
    };
    let Some(def) = def else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let versions = match list_versions(&state.pg, &id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[fleet/defs/versions] versions read failed", e)),
    };
    let mut body = json!({
        "def": def,
        "versions": versions.iter().map(version_wire).collect::<Vec<_>>(),
    });
    // Config payloads carry provider numbers (a price, a context length) —
    // print them the JS way (`3`, never `3.0`).
    talaria_body::js_numberify(&mut body);
    Ok(Json(body).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // exclusive lower bound, so the folded helper's >= min cannot say it.
    let revert_to = match number_member(obj, "revertTo", NumKind::Int, f64::MIN, f64::INFINITY) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if revert_to <= 0.0 {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "Too small: expected number to be >0",
        ));
    }
    let revert_to = revert_to as i64;
    let def: Option<(String,)> =
        match sqlx::query_as("select id::text from agent_defs where id = $1::uuid")
            .bind(&id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(row) => row,
            Err(e) => return Ok(internal("[fleet/defs/versions] def read failed", e)),
        };
    let Some((def_id,)) = def else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let versions = match list_versions(&state.pg, &def_id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[fleet/defs/versions] versions read failed", e)),
    };
    let Some(target) = versions.iter().find(|v| v.version as i64 == revert_to) else {
        return Ok(house_error(StatusCode::NOT_FOUND, "version not found"));
    };
    let created_by = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let note = format!("revert to v{}", target.version);
    Ok(
        match add_version_if_changed(
            &state.pg,
            &def_id,
            &NewVersion {
                soul: &target.soul,
                config: &target.config,
                note: Some(&note),
                created_by: Some(&created_by),
            },
        )
        .await
        {
            Ok((version, created)) => {
                Json(json!({ "ok": true, "version": version, "created": created })).into_response()
            }
            Err(e) => internal("[fleet/defs/versions] revert write failed", e),
        },
    )
}
