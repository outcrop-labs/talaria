// The agent fleet: defs, versions, crons, secrets, containers, hires, render/reconcile.
pub mod fleet;
pub mod fleet_agents_id_control;
pub mod fleet_agents_id_crons;
pub mod fleet_agents_id_crons_jobid;
pub mod fleet_agents_id_secrets;
pub mod fleet_containers;
pub mod fleet_create;
pub mod fleet_crons;
pub mod fleet_defs;
pub mod fleet_defs_id;
pub mod fleet_defs_id_edit;
pub mod fleet_defs_id_google;
pub mod fleet_defs_id_mcp;
pub mod fleet_defs_id_versions;
pub mod fleet_endpoints;
pub mod fleet_endpoints_id;
pub mod fleet_endpoints_id_available;
pub mod fleet_federate;
pub mod fleet_hires;
pub mod fleet_reconcile;
pub mod fleet_render;
pub mod fleet_resources;

use talaria_permissions::has_perm;
use talaria_personal_agent::owns_agent;
use talaria_state::AppState;

/// `agents.manage`, or the owner of a personal assistant — the identical
/// question the two cron route files each asked in its own copy.
pub(crate) async fn can_manage_agent(
    state: &AppState,
    user_id: &str,
    role: &str,
    id: &str,
) -> bool {
    match has_perm(&state.pg, user_id, role, "agents.manage").await {
        Ok(true) => true,
        Ok(false) => owns_agent(&state.pg, user_id, None, Some(id)).await,
        Err(_) => false,
    }
}
