// GET /api/fleet/containers. Container reality per agent (the managed
// service). Admins and `agents.manage` see the whole fleet; a manager sees
// the departments their OWN agents run in — without it their tiles would
// render every agent as stopped, which is the one thing this read exists to
// prevent (docs/PERMISSIONS.md, "Agent managers").

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_managers::reads_whole_fleet;
use talaria_api_facades::fleet::docker::container_status;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let fleet_wide = match reads_whole_fleet(&state.pg, &user.id, &user.role).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[fleet] permission read failed", e)),
    };
    let rows: Result<Vec<(String,)>, sqlx::Error> = if fleet_wide {
        sqlx::query_as("select department from agent_defs where enabled order by slug")
            .fetch_all(&state.pg)
            .await
    } else {
        sqlx::query_as(
            "select d.department from agent_defs d \
             where d.enabled \
               and (d.owner_user_id = $1::uuid \
                    or exists (select 1 from agent_managers m \
                               where m.agent_id = d.id and m.user_id = $1::uuid)) \
             order by d.slug",
        )
        .bind(&user.id)
        .fetch_all(&state.pg)
        .await
    };
    let departments = match rows {
        Ok(r) => r.into_iter().map(|(d,)| d).collect::<Vec<_>>(),
        Err(e) => return Ok(internal("[fleet] fleet_containers failed", e)),
    };
    Ok(
        // container_status errors on a docker failure — the whole route 500s
        // the house way (no json body).
        match container_status(&departments).await {
            Ok(containers) => Json(json!({ "containers": containers })).into_response(),
            Err(e) => internal("[fleet] container_status failed", e),
        },
    )
}
