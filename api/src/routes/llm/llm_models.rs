// GET /api/llm/v1/models. OpenAI-compatible model list for external tools
// pointing a tlk_ key at base_url http://<talaria>/api/llm/v1. Byte-stability
// matters: clients diff this list, so field order and the 401 envelope are
// pinned (error.rs tests).

use crate::auth::{authenticate_key, bearer_secret};
use crate::error::{openai_error, thrown_internal_error};
use crate::gateway::models::{EndpointModels, catalog_of};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

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
            tracing::error!("[llm/v1/models] key lookup failed: {e}");
            // No envelope here — status only, no driver text past the
            // boundary.
            return thrown_internal_error();
        }
    };

    // Detached last_used_at: a bookkeeping write must never fail a request
    // that already authenticated. `id::text = $1` because the bind is a
    // String — sqlx types it as text, which has no implicit uuid comparison.
    let pg = state.pg.clone();
    let key_id = identity.key_id;
    tokio::spawn(async move {
        if let Err(e) =
            sqlx::query("update llm_api_keys set last_used_at = now() where id::text = $1")
                .bind(&key_id)
                .execute(&pg)
                .await
        {
            tracing::warn!("[llm/v1/models] last_used_at update failed for {key_id}: {e}");
        }
    });

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
        Err(e) => {
            tracing::error!("[llm/v1/models] catalog query failed: {e}");
            return thrown_internal_error();
        }
    };

    let data = catalog_of(&eps)
        .into_iter()
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
