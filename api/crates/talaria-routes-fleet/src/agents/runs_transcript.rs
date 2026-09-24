// GET /api/runs/{id}/transcript. The retained work history: every turn's
// prompt and the watch lines captured with it. The live terminal only
// replays the current turn's tail, and that tail expires with the session,
// so this is the record a finished session has.
//
// Same ACL as the watch stream. The artifact itself is private and owned by
// nobody — created_by is the agent model — so the generic artifact read
// filters it out and the Turns pane used to say the history was never
// captured. Whoever may see the run may read what it already did.
//
// 200 `{ body }` — null when capture never wrote a row (a session that
// predates it, or a capture that failed). Missing and out-of-audience both
// answer 403, same as the watch stream: a guessable id is not an existence
// oracle.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_error::{house_error, internal};
use talaria_realtime_watch::{RunWatchVerdict, may_watch_run, real_watch_deps};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let verdict = match may_watch_run(&user.id, &run_id, &real_watch_deps(state.pg.clone())).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(internal(
                &format!("[runs/transcript] watch gate failed for {run_id}"),
                e,
            ));
        }
    };
    if verdict != RunWatchVerdict::Ok {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    match talaria_artifacts::run_transcript(&state.pg, &run_id).await {
        Ok(body) => Ok(Json(json!({ "body": body })).into_response()),
        Err(e) => Ok(internal(
            &format!("[runs/transcript] read failed for {run_id}"),
            e,
        )),
    }
}
