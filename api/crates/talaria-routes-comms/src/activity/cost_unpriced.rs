// doc: GET /api/cost/unpriced. Every cloud model whose usage in the last 30
// doc: days has no price — its endpoint, model, token counts and the reason
// doc: (no published price found, the price source didn't answer, not looked
// doc: up yet, the endpoint is gone) — and every admin pricing decision with
// doc: what it prices now, plus (`fuzzy`) every model priced by a fuzzy
// doc: catalog match — the entry it was priced as — which a manual rate can
// doc: override. Admins only: the fix is theirs to make.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_error::internal;
use talaria_price_oracle::unpriced::{list_fuzzy, list_settings, list_unpriced};
use talaria_session::require_admin;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let (unpriced, prices, fuzzy) = tokio::join!(
        list_unpriced(&state.pg),
        list_settings(&state.pg),
        list_fuzzy(&state.pg)
    );
    let unpriced = match unpriced {
        Ok(u) => u,
        Err(e) => return Ok(internal("[cost] unpriced list failed", e)),
    };
    let prices = match prices {
        Ok(p) => p,
        Err(e) => return Ok(internal("[cost] price settings failed", e)),
    };
    let fuzzy = match fuzzy {
        Ok(f) => f,
        Err(e) => return Ok(internal("[cost] fuzzy-match list failed", e)),
    };
    Ok(Json(json!({ "unpriced": unpriced, "prices": prices, "fuzzy": fuzzy })).into_response())
}
