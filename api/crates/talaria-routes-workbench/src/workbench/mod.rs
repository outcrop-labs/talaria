// The workbench surface: flows, git hosts, jobs, repos, and coding accounts.
//
// Coding accounts come in two halves. The `workbench_coding_*` routes are the
// human side (sign in, choose a plan, pin a ticket); the `workbench_auth_*`
// routes are omp's auth-broker protocol as the agents' harnesses speak it,
// with `coding_broker` and `coding_gate` holding what both halves share.
pub mod coding_broker;
pub mod coding_gate;
#[cfg(test)]
pub(crate) mod live_support;
pub mod workbench_auth_credential;
pub mod workbench_auth_credential_id_block;
pub mod workbench_auth_credential_id_blocks;
pub mod workbench_auth_credential_id_disable;
pub mod workbench_auth_credential_id_refresh;
pub mod workbench_auth_healthz;
pub mod workbench_auth_snapshot;
pub mod workbench_coding_accounts_agent_id;
pub mod workbench_coding_login;
pub mod workbench_coding_pin_task_id;
pub mod workbench_coding_services;
pub mod workbench_env_repo;
pub mod workbench_flow;
pub mod workbench_git_hosts;
pub mod workbench_git_hosts_id;
pub mod workbench_git_hosts_id_agents;
pub mod workbench_git_hosts_id_refresh;
pub mod workbench_git_hosts_id_test;
pub mod workbench_github_permissions_agent_id;
pub mod workbench_github_permissions_agent_id_check;
pub mod workbench_harnesses;
pub mod workbench_jobs;
pub mod workbench_permission_requests;
pub mod workbench_repo_requests;
pub mod workbench_repos_agent_id;

/// The git host `id` names, or the answer when there is none: a 404, or a
/// 500 when the registry cannot be read. The git host routes that act on one
/// host (`workbench_git_hosts_id*`) share it.
async fn git_host_or_404(
    state: &talaria_state::AppState,
    id: &str,
) -> Result<talaria_git_hosts::HostRow, axum::response::Response> {
    match talaria_git_hosts::registry::get_host(&state.pg, id).await {
        Ok(Some(row)) => Ok(row),
        Ok(None) => Err(talaria_error::house_error(
            axum::http::StatusCode::NOT_FOUND,
            "no such git host",
        )),
        Err(e) => Err(talaria_error::internal(
            "[workbench/git-hosts] host read failed",
            e,
        )),
    }
}
