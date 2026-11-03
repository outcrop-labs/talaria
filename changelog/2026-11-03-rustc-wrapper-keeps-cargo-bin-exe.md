- **A local build of the api's integration tests compiles again: the rustc wrapper runs on bash, so `CARGO_BIN_EXE_talaria-api` reaches rustc.**
  `scripts/rustc-wrapper.sh`, the `RUSTC_WRAPPER` the root `mise.toml` sets, was a
  `#!/bin/sh` script. On Debian and Ubuntu that is dash, which drops every
  environment variable whose name is not a shell identifier before it execs.
  Cargo names `CARGO_BIN_EXE_<bin>` after the bin, hyphen included, so
  `env!("CARGO_BIN_EXE_talaria-api")` in `api/tests/it/code_probe_child.rs` failed
  to compile in every mise-activated checkout, and `bun run gate` went red on
  untouched `rc` for any diff that selects `talaria-api`. CI sets no wrapper, so
  its api job stayed green. The wrapper now runs on bash, which passes such names
  through. `scripts/rustc-wrapper.test.mjs` runs the wrapper by its own shebang
  and checks that a hyphenated `CARGO_BIN_EXE_*` reaches rustc. Verified: that
  check failed before the change and passes after it, `cargo test -p talaria-api
  --test it --no-run` exited 101 on `rc` at `1759fe9e` and exits 0 with the fix,
  and `bun run gate` passes with a whitespace edit in `talaria-conversations`.
