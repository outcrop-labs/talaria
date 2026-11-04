// GET /api/runs/{id}/checkpoints. The run's append-only ledger in the order
// the work happened: each turn's send row, its steps, then its record row.
// `seq` is identity, not order.
//
// Same authority as the transcript beside it, and the same refusal: missing
// and out-of-audience both answer 403, so a guessable id is not an existence
// oracle. A run older than the ledger answers an empty list.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_error::{house_error, internal};
use talaria_realtime_watch::{RunWatchVerdict, may_watch_run, real_watch_deps};
use talaria_runs_store::run_checkpoints;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&run_id) {
        return Ok(gate);
    }
    let verdict = match may_watch_run(&user.id, &run_id, &real_watch_deps(state.pg.clone())).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(internal(
                &format!("[runs/checkpoints] watch gate failed for {run_id}"),
                e,
            ));
        }
    };
    if verdict != RunWatchVerdict::Ok {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    match run_checkpoints(&state.pg, &run_id).await {
        Ok(checkpoints) => Ok(Json(json!({ "checkpoints": checkpoints })).into_response()),
        Err(e) => Ok(internal(
            &format!("[runs/checkpoints] read failed for {run_id}"),
            e,
        )),
    }
}
