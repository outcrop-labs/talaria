// doc: The app host asks the api for an enabled app's gateway key: one `tlk_`
// doc: key named `app:<slug>`, minted on the first ask and sealed into
// doc: `app_llm_keys`, then the same key on every later ask while it still opens
// doc: and is live — a key that was revoked, replaced or can no longer be opened
// doc: is rotated. The host makes every model call it runs on the app's behalf
// doc: (a harness run from the app's server or tools, a fitness replay) on this
// doc: key, through `POST /api/llm/v1/chat/completions`, and never hands it to
// doc: app code. Disabling or uninstalling the app revokes it. Answers `{ key }`;
// doc: 404 when the app is not enabled. Callable only with the root-derived
// doc: service credential (`x-talaria-internal`); the host's proxy never forwards
// doc: `/api/internal/*`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_apps::{AppKeyError, ensure_app_llm_key};
use talaria_error::{house_error, internal};
use talaria_session::{require_internal, secretbox_or_500};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Response, Response> {
    require_internal(&state, &headers)?;
    let sb = secretbox_or_500(&state, "[apps] gateway key: secretbox").await?;
    // Spawned, so a host that gives up waiting cannot cut a mint between the
    // key row and its record: the ask completes, and the host's next one
    // finds the key it made.
    let (pg, wanted) = (state.pg.clone(), slug.clone());
    let ensured = tokio::spawn(async move { ensure_app_llm_key(&pg, &sb, &wanted).await })
        .await
        .unwrap_or_else(|e| Err(AppKeyError::Store(format!("the key task failed: {e}"))));
    match ensured {
        Ok(key) => Ok(Json(serde_json::json!({ "key": key })).into_response()),
        Err(AppKeyError::NotEnabled) => Ok(house_error(
            StatusCode::NOT_FOUND,
            &format!("app \"{slug}\" is not enabled, so it has no gateway key"),
        )),
        Err(AppKeyError::NoOwner) => Ok(house_error(
            StatusCode::CONFLICT,
            "no user exists yet to own the app's gateway key",
        )),
        Err(e @ AppKeyError::Store(_)) => Ok(internal("[apps] gateway key failed", e)),
    }
}
