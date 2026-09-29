// Agent Google Slides, read only. There is no write half: see the crate
// header in talaria-google-slides for why editing a deck is a different
// problem than editing a doc.
//
// GET /slides/{id}   the deck's text, slide by slide

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::resolve_agent_google;
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::slides::read_deck_with_token;
use talaria_error::house_error_msg;
use talaria_state::AppState;

const NOT_CONNECTED: &str =
    "No Google account is connected for this agent (its owner, or the org account).";

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Slides") {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(house_error_msg(
            StatusCode::CONFLICT,
            "not_connected",
            NOT_CONNECTED,
        ));
    };
    match read_deck_with_token(&google.token, &id).await {
        Ok(deck) => Ok(Json(deck).into_response()),
        Err(e) => Ok(google_fail(e, "Slides")),
    }
}
