- **The port's end state corrected: Rust is the backend, not the UI host.**
  The roadmap's cutover batch said Rust would serve the SPA and `bun run
  start` would become the Rust binary — that was never the ask. SvelteKit
  keeps serving the SPA, its pages, and routing (TanStack stays the client
  layer, untouched). Cutover now deletes the TS API — `ui/src/routes/api`
  and the backend of `ui/src/server` — while the SvelteKit server remains
  the one public origin, handing every `/api/*` request to Rust on loopback
  (same-origin cookies, so the proxy hop graduates from scaffolding to the
  permanent architecture). The musl build stage stays: it builds the Rust
  backend process the prod image runs alongside the frontend server.
