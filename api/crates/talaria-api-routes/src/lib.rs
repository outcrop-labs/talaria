// The router crate — the table in routes/mod.rs names the system; the
// handler groups live in their own crates (talaria-routes-*), compiled in
// parallel; the engine facades live in talaria-api-facades.

pub mod routes;
