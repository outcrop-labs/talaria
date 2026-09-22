- **The port merges — the Rust API is the backend of record.** PR #290
  landed on main (2026-09-01): 216 of 219 TS route files serve from the Rust
  crate in any proxied environment, the parity battery closed at 370 route
  pairs byte-diffed against the TS oracle on the shared dev DB (every pair
  either byte-identical or a recorded divergence — that record is the
  contract, `docs/RUST-MIGRATION.md`), and the scheduler flip is armed in
  dev (`TALARIA_SCHEDULER=rust` — the whole job table from `api/src/jobs.rs`,
  TS's `startScheduler` stood down on the same value). CI gained the `api`
  job: rustfmt, clippy `-D warnings`, and the crate's tests on the pinned
  1.97.1 toolchain (`bun run api:check` runs the same gates locally). The
  three permanent residents stay TS (`admin/update`, the rule-10 app
  dispatch, and `healthz`); cutover — deleting the TS API behind the proxy —
  is the one remaining step.
