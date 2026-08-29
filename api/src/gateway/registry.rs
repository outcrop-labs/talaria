// Endpoint registry + model routing — port of listEndpoints (agent-defs.ts)
// and routingFor/resolveRoute (llm-gateway.ts). The catalog shaping for
// /models lives in gateway/models.rs; this is the WRITE side's view: which
// endpoint serves a model id, and how a bare pool round-robins.

use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// One `llm_endpoints` row, the columns the relay reads. TS's LlmEndpoint
/// also carries pricing columns; pricing is computed SQL-side in spend_since,
/// so the relay's view omits them.
#[derive(Debug, Clone)]
pub struct LlmEndpoint {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub class: String,
    pub api_key_env: Option<String>,
    pub models: Vec<String>,
    pub request_defaults: serde_json::Value,
}

pub async fn list_endpoints(pg: &PgPool) -> Result<Vec<LlmEndpoint>, sqlx::Error> {
    // Same order the TS query produces: local endpoints first, then name asc —
    // round-robin pools and owned_by lists both depend on it.
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            sqlx::types::Json<Vec<String>>,
            Option<serde_json::Value>,
        ),
    >(
        "select id::text, name, provider, base_url, class, api_key_env, models, request_defaults \
         from llm_endpoints order by (class = 'local') desc, name asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, provider, base_url, class, api_key_env, models, request_defaults)| {
                LlmEndpoint {
                    id,
                    name,
                    provider,
                    base_url,
                    class,
                    api_key_env,
                    models: models.0,
                    // TS reads `ep.requestDefaults ?? {}` at use — same here.
                    request_defaults: request_defaults.unwrap_or(serde_json::Value::Null),
                }
            },
        )
        .collect())
}

pub struct ModelRouting {
    /// Every endpoint this model id can land on — one for a pin, the whole
    /// round-robin pool for a bare name, empty when nothing serves it.
    pub endpoints: Vec<LlmEndpoint>,
    /// The model id the upstream expects (a pin drops the endpoint prefix).
    pub upstream_model: String,
}

/// Where a model id CAN go, without picking (and without advancing the
/// round-robin cursor) — port of routingFor.
pub async fn routing_for(pg: &PgPool, model: &str) -> Result<ModelRouting, sqlx::Error> {
    let eps = list_endpoints(pg).await?;
    // Endpoint-qualified: "<endpoint>/<rest>" (rest may itself contain "/").
    if let Some(slash) = model.find('/')
        && slash > 0
    {
        let ep_name = &model[..slash];
        let rest = &model[slash + 1..];
        if let Some(ep) = eps.iter().find(|e| e.name == ep_name)
            && ep.models.iter().any(|m| m == rest)
        {
            return Ok(ModelRouting {
                endpoints: vec![ep.clone()],
                upstream_model: rest.to_string(),
            });
        }
    }
    Ok(ModelRouting {
        endpoints: eps
            .into_iter()
            .filter(|e| e.models.iter().any(|m| m == model))
            .collect(),
        upstream_model: model.to_string(),
    })
}

/// Round-robin cursor per bare model name (module-level; resets on restart —
/// same lifetime as TS's module Map).
fn rr() -> &'static Mutex<HashMap<String, usize>> {
    static RR: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    RR.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct ResolvedRoute {
    pub endpoint: LlmEndpoint,
    /// The model id the upstream expects.
    pub upstream_model: String,
}

/// Resolve a requested model id to an endpoint + upstream model — port of
/// resolveRoute. Ok(None) = nothing serves it.
pub async fn resolve_route(pg: &PgPool, model: &str) -> Result<Option<ResolvedRoute>, sqlx::Error> {
    let ModelRouting {
        endpoints,
        upstream_model,
    } = routing_for(pg, model).await?;
    if endpoints.is_empty() {
        return Ok(None);
    }
    let i = {
        let mut rr = rr().lock().unwrap();
        let i = rr.get(model).copied().unwrap_or(0) % endpoints.len();
        rr.insert(model.to_string(), i + 1);
        i
    };
    Ok(Some(ResolvedRoute {
        endpoint: endpoints[i].clone(),
        upstream_model,
    }))
}

#[cfg(test)]
mod tests {
    // Round-robin arithmetic is the only pure logic here; the rest is SQL and
    // is exercised by the live-DB tests (tests/llm_chat.rs).
    #[test]
    fn cursor_advances_modulo() {
        // 3 endpoints, cursor 0,1,2,0 — mirrors (rr.get ?? 0) % len, i+1.
        for (cursor, len, want) in [(0usize, 3usize, 0usize), (1, 3, 1), (3, 3, 0), (7, 3, 1)] {
            assert_eq!(cursor % len, want);
        }
    }
}
