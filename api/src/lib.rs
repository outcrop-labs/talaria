// talaria-api library — everything the binary mounts, importable so
// integration tests (tests/) can drive the router and the secretbox against
// committed cross-language fixtures without going through a socket.

pub mod activity;
pub mod agent_auth;
pub mod agent_role_templates;
pub mod audit;
pub mod auth;
pub mod auth_config;
pub mod boards;
pub mod body;
pub mod claim;
pub mod config;
pub mod db;
pub mod error;
pub mod fleet;
pub mod gateway;
pub mod google_client;
pub mod google_connections;
pub mod google_oauth;
pub mod instance;
pub mod invites;
pub mod llm_keys;
pub mod notify;
pub mod org_domains;
pub mod params;
pub mod password;
pub mod password_accounts;
pub mod permissions;
pub mod ratelimit;
pub mod routes;
pub mod secretbox;
pub mod session;
pub mod state;
pub mod teams;
pub mod users;
pub mod workflows;
