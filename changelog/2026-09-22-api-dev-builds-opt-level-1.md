- **API dev builds: opt-level 1 + mold.** `api/Cargo.toml` `[profile.dev]`
  compiles our crate at `-C opt-level=1`, deps at 3, `debug =
  "line-tables-only"` (unwind stays — tests and catch-panic). gnu linux
  links with mold (`api/.cargo/config.toml`); CI installs the package, the
  devbox image does too. musl/package image unchanged until mold-on-musl is
  proven. Verified: `cargo build` after the profile flip (4m 34s, deps at
  opt-level 3); incremental `touch src/routes/mod.rs` 20.7s (was 23.7s);
  `cargo clippy --all-targets -- -D warnings` green; `cargo test` green;
  `bun run check`.
