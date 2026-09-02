// /api/search — port of ui/src/routes/api/search.ts.
//
// LIVE WEB SEARCH — the endpoint behind the `web_search` MCP tool, and the one
// place an agent or a signed-in user reaches this deployment's search.
//
// IT ASKS THE RESOLVER, NOT SEARXNG. This used to call the SearXNG client
// directly, which meant every fleet agent was pinned to Talaria's own instance
// while the research harness and the fitness sweep resolved search properly —
// registry first, our engine as the floor. An org that had registered Exa got
// it everywhere EXCEPT in the agents doing their actual work. One resolver now
// answers for all of them (web_search.rs).
//
// THE SAME DOOR AS `search_knowledge`, deliberately: a fleet agent authenticates
// with its agent key and a person with their session, and neither gets a
// different search. What differs from /api/rag/search is that this one has no
// PRINCIPAL-SCOPED CORPUS to speak of — the public web is the same web for
// everybody, so there is nothing here to scope and nothing that could leak
// between tenants. The auth check is still required: search costs upstream
// requests against engines that rate-limit by IP, and an unauthenticated
// endpoint is a free proxy for whoever finds it.

use crate::agent_auth::agent_caller;
use crate::body::{as_object, parse, string_member};
use crate::error::house_error;
use crate::session::require_user;
use crate::state::AppState;
use crate::web_search::search_the_web;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    if caller.is_none()
        && let Err(gate) = require_user(&state, &headers).await
    {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let query = match string_member(obj, "query", 2, 400) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let limit = match obj.get("limit") {
        None => None,
        Some(v) => match v.as_i64() {
            // z.number().int().min(1).max(25)
            Some(n) if (1..=25).contains(&n) => Some(n as f64),
            _ => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    "limit must be an integer from 1 to 25",
                );
            }
        },
    };

    match search_the_web(&state, &query, limit).await {
        Ok(found) => Json(json!({
            "query": query,
            "results": found.results,
            // `via` rides along so a caller can say WHICH engine answered.
            // Null is Talaria's own; a named supplier is the org's
            // registered provider.
            "via": found.via,
        }))
        .into_response(),
        // 503, NOT 500: search being unreachable is a fact about this
        // deployment's infrastructure rather than a bug in the request, and
        // the sentence goes straight into an agent's transcript — the search
        // tool writes these for a model to act on, so it is relayed rather
        // than replaced.
        Err(err) => {
            let mut resp = Json(json!({ "error": err })).into_response();
            *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            resp
        }
    }
}
