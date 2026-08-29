// talaria-api library — everything the binary mounts, importable so
// integration tests (tests/) can drive the router and the secretbox against
// committed cross-language fixtures without going through a socket.

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod gateway;
pub mod ratelimit;
pub mod routes;
pub mod secretbox;
pub mod state;
