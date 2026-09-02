// /api/rag/search — port of ui/src/routes/api/rag.search.ts. Ranked
// retrieval across the caller's accessible collections, for EITHER caller
// shape: a signed-in user (their bindings) or a fleet agent (agent key +
// x-agent-name → that agent's bindings). This is the endpoint the
// search_knowledge MCP tool calls — retrieval as function-calling.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use serde_json::json;

use crate::agent_auth::agent_caller;
use crate::body::{
    NumKind, as_object, optional_number_member, optional_uuid_array_member, parse, string_member,
};
use crate::error::house_error;
use crate::retrieval::index::{self, Principal, SearchOpts};
use crate::retrieval::{embed, qdrant};
use crate::session::require_user;
use crate::state::AppState;

fn hit_json(h: &index::RetrievalHit) -> Value {
    json!({
        "collection": h.collection,
        "score": h.score,
        "sourceType": h.source_type,
        "sourceId": h.source_id,
        "title": h.title,
        "snippet": h.snippet,
        "href": h.href,
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Agent-first: a fleet key + x-agent-name searches AS that agent, and a
    // REJECTED agent credential answers its own 401/403 here — it never
    // falls through to the session, because a bad key is not "no key".
    let (user_id, agent_model) = match agent_caller(&state.pg, &headers).await {
        Ok(Some(caller)) => (None, Some(caller.model)),
        Ok(None) => match require_user(&state, &headers).await {
            Ok(u) => (Some(u.id), None),
            Err(gate) => return gate,
        },
        Err(resp) => return resp,
    };
    let principal = Principal {
        user_id: user_id.as_deref(),
        agent_model: agent_model.as_deref(),
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let query = match string_member(obj, "query", 1, 2000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let limit = match optional_number_member(obj, "limit", NumKind::Int, 1.0, 20.0) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let collection_ids = match optional_uuid_array_member(obj, "collectionIds", 20) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let opts = SearchOpts {
        limit: limit.map(|l| l as usize),
        collection_ids: collection_ids.as_deref(),
    };
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    let http = crate::retrieval::real_http();
    match index::search_for_principal(&state, &qd, &ed, &http, principal, &query, opts).await {
        Ok(hits) => {
            Json(json!({ "hits": hits.iter().map(hit_json).collect::<Vec<_>>() })).into_response()
        }
        // TS's catch carries the failure's own sentence at 502: retrieval is
        // a gateway to someone else's index, and "unreachable" is the honest
        // status for a search that never ran.
        Err(msg) => house_error(StatusCode::BAD_GATEWAY, &msg),
    }
}
