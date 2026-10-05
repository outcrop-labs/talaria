// The workbench surface: flows, github, jobs, repos, and coding accounts.
//
// Coding accounts come in two halves. The `workbench_coding_*` routes are the
// human side (sign in, choose a plan, pin a ticket); the `workbench_auth_*`
// routes are omp's auth-broker protocol as the agents' harnesses speak it,
// with `coding_broker` and `coding_gate` holding what both halves share.
pub mod coding_broker;
pub mod coding_gate;
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
pub mod workbench_github;
pub mod workbench_jobs;
pub mod workbench_repo_requests;
pub mod workbench_repos_agent_id;
