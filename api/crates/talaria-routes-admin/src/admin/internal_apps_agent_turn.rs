// doc: The app host sends one agent turn an app's code asked for
// doc: (`ctx.agents.turn` in `@talaria/sdk/server`): `{ app, actor, agent,
// doc: message, system?, session?, effort? }`, where `actor` is what started
// doc: the app's code as the host authenticated it — `{ kind: "request",
// doc: userId }`, `{ kind: "mcp", agent, proven }` or `{ kind: "trigger",
// doc: trigger }`. Who may drive whom is decided here: from a request, only an
// doc: agent that person may use (never someone else's personal assistant);
// doc: from an MCP tool, only the calling agent, proven by its own key; from a
// doc: trigger, no agent. A refusal is a 403 with the sentence, before any turn
// doc: starts. The app must be enabled with a ready build and under its LLM
// doc: budget (`app:<slug>`, the subject its relay calls are held to). The turn goes
// doc: through the fleet's own chat door — on a Hermes session keyed per app,
// doc: per person or calling agent, per `session` key, when one is given — and
// doc: its spend is booked to the agent under ledger source `app`. The answer
// doc: streams as newline-delimited JSON: `{type:"text",text}` and
// doc: `{type:"tool",name,status,label}` as they land, then one
// doc: `{type:"done",text,tools,session}` or `{type:"error",error}`. A turn
// doc: silent for ten minutes is abandoned; a caller that hangs up stops the
// doc: agent's run. Callable only with the root-derived service credential
// doc: (`x-talaria-internal`); the host's proxy never forwards `/api/internal/*`.

use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use talaria_error::{house_error, internal};
use talaria_session::require_internal;
use talaria_state::AppState;
use tokio::sync::mpsc;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    require_internal(&state, &headers)?;
    let req = match talaria_app_agents::parse_request(&talaria_body::parse(&body)) {
        Ok(req) => req,
        Err(e) => return Ok(house_error(StatusCode::BAD_REQUEST, &e)),
    };
    match talaria_app_agents::refusal(&state.pg, &req).await {
        Ok(Some(why)) => return Ok(house_error(StatusCode::FORBIDDEN, &why)),
        Ok(None) => {}
        Err(e) => return Ok(internal("[apps] agent turn: the access check failed", e)),
    }
    let (tx, rx) = mpsc::channel::<String>(64);
    let pg = state.pg.clone();
    tokio::spawn(async move { talaria_app_agents::drive(&pg, &req, tx).await });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|line| (Ok::<_, Infallible>(Bytes::from(line)), rx))
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, "no-cache, no-transform")
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(stream))
        .expect("static headers build"))
}
