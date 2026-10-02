// /api/workbench/coding/services. The services a coding account can be signed
// in to, straight out of omp's own roster.
//
// The roster is NOT a list Talaria keeps: it comes from the omp auth bridge,
// which reads it from the harness's own compiled auth rules. A provider
// upstream adds appears here without a Talaria change, and one it drops stops
// being offered — which is the only way a list of 23 third-party sign-in flows
// stays true.
//
// Each entry carries its flow shape so the UI can say what is about to be
// asked for (a browser round trip, a device code, a question), and
// `permitted`, so an admin sees everything while a member sees what the org
// allowed. `GET ?models=<provider>` answers that provider's model ids for the
// role pickers.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::collections::HashMap;

use talaria_coding_accounts as ca;
use talaria_error::house_error;
use talaria_state::AppState;

use super::coding_gate::require_feature;

// doc: The coding-account sign-in roster from omp's own auth rules, each entry
// doc: with its flow shape and whether the org permits it. `?models=<provider>`
// doc: instead answers that provider's model ids, for the role pickers.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, Response> {
    require_feature(&state, &headers).await?;

    if let Some(provider) = q
        .get("models")
        .map(String::as_str)
        .filter(|p| !p.is_empty())
    {
        // The gateway's models are the org's own catalog, not omp's — asking
        // the bridge for them would answer with whatever omp thinks `talaria`
        // serves, which is only ever the roles we rendered.
        if provider == ca::GATEWAY_PROVIDER {
            return Ok(
                axum::Json(json!({ "models": gateway_models(&state).await })).into_response(),
            );
        }
        let models = talaria_omp_auth::models(provider)
            .await
            .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
        return Ok(axum::Json(json!({ "models": models })).into_response());
    }

    let permitted = ca::permitted_services(&state.pg).await;
    let roster = talaria_omp_auth::providers()
        .await
        .map_err(|e| house_error(StatusCode::BAD_GATEWAY, &e))?;
    let services: Vec<Value> = roster
        .into_iter()
        .map(|mut entry| {
            let store_as = entry
                .get("storeAs")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(obj) = entry.as_object_mut() {
                obj.insert("permitted".into(), json!(permitted.contains(&store_as)));
            }
            entry
        })
        .collect();
    Ok(axum::Json(json!({ "services": services, "permitted": permitted })).into_response())
}

/// The org's Workbench model roles, as the gateway plan's pickable models.
/// The same models the harness would get with no coding account at all, so
/// choosing the gateway explicitly and configuring nothing agree.
async fn gateway_models(state: &AppState) -> Vec<Value> {
    let mut out = Vec::new();
    for weight in ["light", "standard", "heavy"] {
        if let Ok(Some(model)) =
            talaria_model_roles::resolve_role_model(&state.pg, &format!("code-{weight}")).await
            && !out.iter().any(|m: &Value| m["id"] == json!(model))
        {
            out.push(json!({ "id": model, "name": format!("{model} (code-{weight})") }));
        }
    }
    out
}
