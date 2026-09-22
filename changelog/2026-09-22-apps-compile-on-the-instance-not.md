- **Apps compile on the instance, not into the host.** Installing an app no
  longer requires rebuilding Talaria. The host emits stable `/runtime/rt-*.js`
  entries; each app is a standalone Vite build against those, with its own
  Postgres (a compose project, spawned on install, destroyed on uninstall).
  The DB password is sealed in `app_settings` and written to a 0600 `db.env`,
  never `docker run -e` and never HMAC of `TALARIA_SECRET_KEY`. `talaria backup`
  dumps each app DB into `app-data.tar.gz`. A throw in an app's UI, server,
  or MCP stays in that pane and shows the crash — the rest of the cockpit keeps
  running. Authors write TypeScript against `@talaria/sdk`; they never write
  Rust. Core ships no example app — scaffold with `bun talaria app new`.
  Verified: `bun run verify` (svelte-check 0 errors, mcp `tsc --noEmit`,
  1139 ui tests); `cargo fmt` + clippy `-D warnings` + 2112 api tests from
  `api/` (the crate's rust-toolchain). Specifier derivation includes
  `svelte/internal/client` and excludes the compiler; `sourceKey` is stable
  then changes with a source edit; `isolateApp` swallows a throw into
  `{ ok: false }`; `composeYaml` has no password; a compose down/up kept a
  row; restore of `all` extracts `app-data.tar.gz`.
