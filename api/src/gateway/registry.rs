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
    /// `llm_endpoints.context_length` — the default context window an
    /// endpoint declares; the agent editor's platform-defaults hire carries
    /// it into the fresh config's main target.
    pub context_length: Option<i64>,
    pub models: Vec<String>,
    /// `llm_endpoints.price_in_per_mtok` / `price_out_per_mtok` — the flat
    /// $/MTok fallback when neither override names the model. Null when the
    /// endpoint was never priced; the fitness estimate reads them through
    /// `price_of` exactly as the TS surface did.
    pub price_in_per_mtok: Option<f64>,
    pub price_out_per_mtok: Option<f64>,
    /// `llm_endpoints.model_prices` — an admin's per-model overrides, keyed by
    /// upstream model id. Raw jsonb: `{ "deepseek-v4": {"in": 0.27, "out": …} }`.
    pub model_prices: serde_json::Value,
    /// `llm_endpoints.auto_prices` — the price oracle's fills, same shape.
    /// Read-only; `model_prices` always wins.
    pub auto_prices: serde_json::Value,
    pub request_defaults: serde_json::Value,
    /// `llm_endpoints.model_efforts` — an admin's declared effort ladder per
    /// upstream model id (`{"deepseek-v4": ["low","high"]}`), the second voice
    /// that can vouch where a provider's catalog is silent. Raw jsonb kept
    /// unparsed: the column is admin-typed and outlives the build that wrote
    /// it, and validation happens per-model at use (model_efforts.rs), where a
    /// malformed entry degrades to the catalog's answer. Null when the column
    /// is null or predates the feature.
    pub model_efforts: serde_json::Value,
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
            Option<i64>,
            sqlx::types::Json<Vec<String>>,
            Option<f64>,
            Option<f64>,
            Option<serde_json::Value>,
            Option<serde_json::Value>,
            Option<serde_json::Value>,
            Option<serde_json::Value>,
        ),
    >(
        "select id::text, name, provider, base_url, class, api_key_env, context_length, models, \
         price_in_per_mtok::float8, price_out_per_mtok::float8, model_prices, auto_prices, request_defaults, model_efforts \
         from llm_endpoints order by (class = 'local') desc, name asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                name,
                provider,
                base_url,
                class,
                api_key_env,
                context_length,
                models,
                price_in_per_mtok,
                price_out_per_mtok,
                model_prices,
                auto_prices,
                request_defaults,
                model_efforts,
            )| {
                LlmEndpoint {
                    id,
                    name,
                    provider,
                    base_url,
                    class,
                    api_key_env,
                    context_length,
                    models: models.0,
                    price_in_per_mtok,
                    price_out_per_mtok,
                    // TS reads `ep.modelPrices ?? {}` at use — same here.
                    model_prices: model_prices.unwrap_or(serde_json::Value::Null),
                    auto_prices: auto_prices.unwrap_or(serde_json::Value::Null),
                    // TS reads `ep.requestDefaults ?? {}` at use — same here.
                    request_defaults: request_defaults.unwrap_or(serde_json::Value::Null),
                    model_efforts: model_efforts.unwrap_or(serde_json::Value::Null),
                }
            },
        )
        .collect())
}

// ── The registry's own control plane (agent-defs.ts endpoints CRUD) ─────────

/// The wire shape of listEndpoints(): every column in the TS SELECT's order,
/// jsonb passthroughs riding raw Values (the stored canonical order IS the
/// wire order), and prices as STRINGS — postgres.js hands numerics through
/// unparsed (arbitrary precision is the column's promise), so a stored 3
/// reads "3" on the wire, not 3. ::text prints the stored digits verbatim,
/// which is exactly that.
pub async fn list_endpoints_wire(pg: &PgPool) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    let rows: Vec<(
        String,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        bool,
    )> = sqlx::query_as(
        "select id::text, name, provider, base_url, class, api_key_env, context_length, \
         price_in_per_mtok::text, price_out_per_mtok::text, models, model_prices, \
         model_efforts, auto_prices, request_defaults, (api_key_cipher is not null) \
         from llm_endpoints order by (class = 'local') desc, name asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, provider, base_url, class, api_key_env, context_length,
              price_in, price_out, models, model_prices, model_efforts, auto_prices,
              request_defaults, has_key)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "provider": provider,
                    "baseUrl": base_url,
                    "class": class,
                    "apiKeyEnv": api_key_env,
                    "contextLength": context_length,
                    "priceInPerMtok": price_in,
                    "priceOutPerMtok": price_out,
                    "models": models,
                    "modelPrices": model_prices,
                    "modelEfforts": model_efforts,
                    "autoPrices": auto_prices,
                    "requestDefaults": request_defaults,
                    "hasKey": has_key,
                })
            },
        )
        .collect())
}

/// What updateEndpoint accepts — every field Option-al, the parse having
/// already happened (body.rs tri-state: None = absent from the patch).
pub struct EndpointPatch {
    pub class: Option<String>,
    pub price_in_per_mtok: Option<Option<f64>>,
    pub price_out_per_mtok: Option<Option<f64>>,
    pub models: Option<Vec<String>>,
    pub model_prices: Option<serde_json::Value>,
    pub model_efforts: Option<serde_json::Value>,
    pub request_defaults: Option<serde_json::Value>,
    /// Raw provider API key: a non-empty string seals + stores it; '' or null
    /// clears it; None (absent) leaves the stored key untouched. Never
    /// round-tripped to clients.
    pub api_key: Option<String>,
}

/// updateEndpoint — one independent UPDATE per PRESENT field, exactly as TS
/// sequences them. Two JS-truthiness traps are load-bearing: `if
/// (patch.models)` skips an EMPTY array (clearing the catalog is not
/// possible through this door), while the jsonb objects update even when
/// empty ({}) because a JS object is always truthy.
pub async fn update_endpoint(
    pg: &PgPool,
    sb: &crate::secretbox::SecretBox,
    id: &str,
    patch: &EndpointPatch,
) -> Result<(), sqlx::Error> {
    if let Some(api_key) = &patch.api_key {
        // patch.apiKey ? seal(trim) : null — '' and null both clear.
        let cipher = if api_key.trim().is_empty() {
            None
        } else {
            sb.seal(api_key.trim()).ok()
        };
        sqlx::query("update llm_endpoints set api_key_cipher = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(cipher)
            .execute(pg)
            .await?;
    }
    if let Some(class) = &patch.class {
        sqlx::query("update llm_endpoints set class = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(class)
            .execute(pg)
            .await?;
    }
    if let Some(request_defaults) = &patch.request_defaults {
        sqlx::query("update llm_endpoints set request_defaults = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(request_defaults)
            .execute(pg)
            .await?;
    }
    if let Some(price_in) = &patch.price_in_per_mtok {
        sqlx::query("update llm_endpoints set price_in_per_mtok = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(price_in)
            .execute(pg)
            .await?;
    }
    if let Some(price_out) = &patch.price_out_per_mtok {
        sqlx::query("update llm_endpoints set price_out_per_mtok = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(price_out)
            .execute(pg)
            .await?;
    }
    if let Some(models) = &patch.models
        && !models.is_empty()
    {
        sqlx::query("update llm_endpoints set models = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(serde_json::json!(models))
            .execute(pg)
            .await?;
    }
    if let Some(model_prices) = &patch.model_prices {
        sqlx::query("update llm_endpoints set model_prices = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(model_prices)
            .execute(pg)
            .await?;
    }
    if let Some(model_efforts) = &patch.model_efforts {
        sqlx::query("update llm_endpoints set model_efforts = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(model_efforts)
            .execute(pg)
            .await?;
    }
    Ok(())
}

/// createEndpoint — a user-defined endpoint (Models tab). Name must be fresh;
/// the caller maps a unique-violation to the friendly sentence.
pub async fn create_endpoint(
    pg: &PgPool,
    sb: &crate::secretbox::SecretBox,
    name: &str,
    provider: &str,
    base_url: Option<&str>,
    class: &str,
    api_key_env: Option<&str>,
    api_key: Option<&str>,
    models: &[String],
    model_prices: &serde_json::Value,
) -> Result<String, sqlx::Error> {
    let cipher = api_key
        .map(|k| sb.seal(k.trim()).ok())
        .flatten();
    let row: (String,) = sqlx::query_as(
        "insert into llm_endpoints \
           (name, provider, base_url, class, api_key_env, api_key_cipher, models, model_prices) \
         values ($1, $2, $3, $4, $5, $6, $7::jsonb, $8::jsonb) returning id::text",
    )
    .bind(name)
    .bind(provider)
    .bind(base_url)
    .bind(class)
    .bind(api_key_env)
    .bind(cipher)
    .bind(serde_json::json!(models))
    .bind(model_prices)
    .fetch_one(pg)
    .await?;
    Ok(row.0)
}

/// ensureEndpoint — insert-if-absent (on conflict only provider/base_url
/// refresh; class/key-env/context stay as first written). Federation and the
/// import path use it to map outside model targets into the registry.
pub async fn ensure_endpoint(
    pg: &PgPool,
    name: &str,
    provider: &str,
    base_url: Option<&str>,
    class: &str,
    api_key_env: Option<&str>,
    context_length: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into llm_endpoints (name, provider, base_url, class, api_key_env, context_length) \
         values ($1, $2, $3, $4, $5, $6) \
         on conflict (name) do update set \
           provider = excluded.provider, \
           base_url = excluded.base_url, \
           updated_at = now()",
    )
    .bind(name)
    .bind(provider)
    .bind(base_url)
    .bind(class)
    .bind(api_key_env)
    .bind(context_length)
    .execute(pg)
    .await?;
    Ok(())
}

/// addEndpointModels — union new ids into the endpoint's model list, first
/// occurrence order preserved-ish (jsonb_agg(distinct) inside one call).
pub async fn add_endpoint_models(
    pg: &PgPool,
    name: &str,
    models: &[String],
) -> Result<(), sqlx::Error> {
    if models.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "update llm_endpoints \
         set models = ( \
           select coalesce(jsonb_agg(distinct m), '[]'::jsonb) \
           from jsonb_array_elements_text(models || $2::jsonb) as m \
         ), updated_at = now() \
         where name = $1",
    )
    .bind(name)
    .bind(serde_json::json!(models))
    .execute(pg)
    .await?;
    Ok(())
}

/// deleteEndpoint — refused while any ENABLED agent's CURRENT version targets
/// it (retired agents don't run; their history must not block).
pub async fn delete_endpoint(
    pg: &PgPool,
    id: &str,
) -> Result<(bool, Vec<String>), sqlx::Error> {
    let name: Option<(String,)> =
        sqlx::query_as("select name from llm_endpoints where id = $1::uuid")
            .bind(id)
            .fetch_optional(pg)
            .await?;
    let Some((name,)) = name else {
        return Ok((true, Vec::new()));
    };
    let users: Vec<(String,)> = sqlx::query_as(
        "select d.slug from agent_defs d \
         join agent_versions v on v.agent_id = d.id and v.version = d.current_version \
         where d.enabled \
           and (v.config->'main'->>'endpoint' = $1 \
             or exists (select 1 from jsonb_array_elements(coalesce(v.config->'aliases','[]'::jsonb)) a where a->>'endpoint' = $1) \
             or exists (select 1 from jsonb_array_elements(coalesce(v.config->'fallbacks','[]'::jsonb)) f where f->>'endpoint' = $1))",
    )
    .bind(&name)
    .fetch_all(pg)
    .await?;
    if !users.is_empty() {
        return Ok((false, users.into_iter().map(|(s,)| s).collect()));
    }
    sqlx::query("delete from llm_endpoints where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await?;
    Ok((true, Vec::new()))
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
