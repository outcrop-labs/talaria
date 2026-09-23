- **The API is moving to Rust, and the first slice is already serving**: a new `api/`
  crate (axum + sqlx) ports the backend batch by batch while the app keeps working — a
  loopback proxy in the TS server forwards migrated prefixes to it
  (`TALARIA_RUST_API_URL`; unset is the byte-identical TS-serves-everything default, and
  there is deliberately no fallback when it is set). Batch 1, the LLM gateway
  (`/api/llm/v1/*`), is done: models and chat/completions — streaming included, with the
  relay metering tokens and booking the ledger whichever end the stream comes to, so a
  client that hangs up mid-stream is still recorded because the provider still bills —
  per-key caps and throttling, rate limits, secretbox unseal pinned by two-way
  cross-language vectors, and the confab guard riding the relay in all four postures
  (observe records, annotate warns in-stream, strict scrubs). `talaria dev` gains
  `TALARIA_API=on` to raise the api as a sidecar (adopting one already on the port,
  degrading to a warn on a box without cargo), the devbox image carries the pinned
  toolchain, and CI gates the crate (fmt + clippy + the pure unit suite —
  service-dependent tests stay local, by the house rule). Roadmap and coexistence rules:
  `docs/RUST-MIGRATION.md`. Verified: models byte-diffed against TS on the same dev DB;
  live streaming and non-streaming completions through a real provider with
  `usage_events` and `guard_findings` rows checked after each guard posture, including a
  strict-mode redaction of a model-invented credential; the sidecar's spawn, adopt and
  no-cargo paths; the devbox image building the whole crate, C toolchain and all.
