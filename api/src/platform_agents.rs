// Platform sub-agents — Talaria's OWN workers, separate from the Hermes
// fleet. Which model powers each agent is configured GRANULARLY on
// Models → Platform; unset = the job's auto chain keeps working untouched.
// Port of the resolution half of platform-agents.ts: the catalog of agents
// (labels, jobs, skills — the admin panel's rows) ports with that panel's
// route; what the model-identity plane needs is the assignment lookup.
//
// Resolution contract mirrors model roles: an assignment only wins while it
// still ROUTES on the gateway — a deleted model can never silently break a
// subsystem, the job just falls back to its own chain.

use crate::gateway::registry::resolve_route;
use crate::gateway::settings::get_setting;
use sqlx::PgPool;

const KEY: &str = "platform_agent_models";

/// Current raw assignments (id → model), unvalidated.
pub async fn get_platform_agent_models(pg: &PgPool) -> serde_json::Value {
    get_setting(pg, KEY, serde_json::json!({})).await
}

/// The admin-assigned model for a platform agent — but only while it still
/// routes on the gateway. None = unassigned or stale: use the job's auto
/// chain. A database failure propagates (TS throws to the same 500).
///
/// NOTE: this resolves (and so ADVANCES the round-robin cursor), exactly as
/// the TS `resolveRoute(assigned)` does — intentional parity with live
/// traffic's routing behavior.
pub async fn platform_agent_model(pg: &PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
    let Some(assigned) = get_platform_agent_models(pg)
        .await
        .get(id)
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        return Ok(None);
    };
    if resolve_route(pg, &assigned).await?.is_none() {
        return Ok(None);
    }
    Ok(Some(assigned))
}
