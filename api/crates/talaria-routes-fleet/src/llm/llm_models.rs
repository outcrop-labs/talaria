// GET /api/llm/v1/models. OpenAI-compatible model list for external tools
// pointing a tlk_ key at base_url http://<talaria>/api/llm/v1. Byte-stability
// matters: clients diff this list, so field order and the 401 envelope are
// pinned (error.rs tests).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use talaria_api_facades::gateway::agent_keys::{agent_key_scope, agent_slug_of_key};
use talaria_api_facades::gateway::models::{EndpointModels, catalog_of};
use talaria_auth::{authenticate_key, bearer_secret};
use talaria_error::{internal, openai_error};
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct ModelEntry {
    id: String,
    object: &'static str,
    owned_by: String,
}

#[derive(serde::Serialize)]
struct ModelsBody {
    object: &'static str,
    data: Vec<ModelEntry>,
}

/// The LLM-key stamp: a key stamped less than four minutes ago is not
/// matched, so it is not written (talaria-agent-auth's last-used stamps say
/// why that is enough for every reader, and why the guard is a minute inside
/// `STAMP_WINDOW`). `$1::uuid` against the uuid column keeps it on the
/// primary key.
const STAMP_LLM_KEY: &str = "update llm_api_keys set last_used_at = now() \
     where id = $1::uuid \
       and (last_used_at is null or last_used_at < now() - interval '4 minutes')";

static KEY_STAMPS: std::sync::LazyLock<talaria_agent_auth::StampCoalescer> =
    std::sync::LazyLock::new(|| {
        talaria_agent_auth::StampCoalescer::new(talaria_agent_auth::STAMP_WINDOW)
    });

/// Stamp `llm_api_keys.last_used_at` — the database half alone,
/// asynchronously committed. Rows written: 0 when the stamp is still inside
/// the window.
pub async fn stamp_key(pg: &sqlx::PgPool, key_id: &str) -> Result<u64, sqlx::Error> {
    talaria_agent_auth::stamp_async(pg, "llm_api_keys", STAMP_LLM_KEY, key_id).await
}

/// Note that an LLM key just authenticated, for this route and the chat
/// relay alike: at most once a window, a detached stamp — a bookkeeping
/// write must never fail or hold up a request that already authenticated.
/// `what` names the route in the log line. None when this process stamped
/// the key within the window. The handle is for a test to wait on; the
/// routes drop it.
pub fn note_key_used(
    pg: &sqlx::PgPool,
    key_id: &str,
    what: &'static str,
) -> Option<tokio::task::JoinHandle<()>> {
    note_key_used_at(pg, key_id, what, std::time::Instant::now())
}

/// [`note_key_used`] at a given instant: the seam a test drives its clock
/// through.
pub fn note_key_used_at(
    pg: &sqlx::PgPool,
    key_id: &str,
    what: &'static str,
    now: std::time::Instant,
) -> Option<tokio::task::JoinHandle<()>> {
    if !KEY_STAMPS.due_at(key_id, now) {
        return None;
    }
    let pg = pg.clone();
    let key_id = key_id.to_string();
    Some(tokio::spawn(async move {
        if let Err(e) = stamp_key(&pg, &key_id).await {
            KEY_STAMPS.forget(&key_id);
            tracing::warn!("{what} last_used_at update failed for {key_id}: {e}");
        }
    }))
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let identity = match bearer_secret(auth) {
        Some(secret) => authenticate_key(&state.pg, secret).await,
        None => Ok(None), // no header: not even a lookup
    };
    let identity = match identity {
        Ok(Some(id)) => id,
        Ok(None) => return openai_error(StatusCode::UNAUTHORIZED, "invalid API key"),
        Err(e) => {
            // No envelope here — status only, no driver text past the
            // boundary.
            return internal("[llm/v1/models] key lookup failed", e);
        }
    };

    // Detached last_used_at: a bookkeeping write must never fail a request
    // that already authenticated.
    let _ = note_key_used(&state.pg, &identity.key_id, "[llm/v1/models]");

    // Local endpoints first, then name asc — first-seen order is what
    // owned_by's endpoint list preserves. `models` is a jsonb column: sqlx
    // maps Vec<String> to text[], so the decode goes through
    // Json<Vec<String>>.
    let eps = match sqlx::query_as::<_, (String, sqlx::types::Json<Vec<String>>)>(
        "select name, models from llm_endpoints order by (class = 'local') desc, name asc",
    )
    .fetch_all(&state.pg)
    .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|(name, models)| EndpointModels {
                name,
                models: models.0,
            })
            .collect::<Vec<_>>(),
        Err(e) => return internal("[llm/v1/models] catalog query failed", e),
    };

    // An agent's key lists what the chat relay would let it ask for — the
    // ids its rendered config routes to — so the list and the 403 agree.
    let scope = match agent_slug_of_key(&identity.key_name) {
        Some(slug) => agent_key_scope(&state.pg, slug).await,
        None => None,
    };
    let data = catalog_of(&eps)
        .into_iter()
        .filter(|m| scope.as_ref().is_none_or(|s| s.models.contains(&m.id)))
        .map(|m| ModelEntry {
            id: m.id,
            object: "model",
            owned_by: format!("talaria:{}", m.endpoints.join(",")),
        })
        .collect();
    Json(ModelsBody {
        object: "list",
        data,
    })
    .into_response()
}
