// talaria-api library — everything the binary mounts, importable so
// integration tests (tests/) can drive the router and the secretbox against
// committed cross-language fixtures without going through a socket.

pub mod agent_auth;
pub mod audit;
pub mod auth;
pub mod body;
pub mod claim;
pub mod config;
pub mod db;
pub mod error;
pub mod gateway;
pub mod google_client;
pub mod password;
pub mod password_accounts;
pub mod ratelimit;
pub mod routes;
pub mod secretbox;
pub mod session;
pub mod state;
pub mod users;
