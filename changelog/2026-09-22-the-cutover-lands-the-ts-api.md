- **The cutover lands — the TS API is deleted; the Rust api is the only
  runtime.** The `cutover/remove-ts-api` branch takes the coexistence
  posture the port left and finishes it, in waves: the proxy flip deletes
  every migrated TS route file and their server halves (SvelteKit keeps the
  SPA and the four permanent TS residents — healthz, `admin/update`, the
  rule-10 `/api/apps/` dispatch, the app-MCP gateway); the server prune and
  the scheduler crossing dissolve the flip predicate (`TALARIA_SCHEDULER`
  shrinks to a kill switch: unset arms, `off` stands down, `rust` is the
  retired handoff value that reads as unset — the api is the only thing left
  to arm); `server-entry.js` owns the api binary in production, adopting an
  instance already on the port or spawning one (`TALARIA_API_BIN`), wiring
  the proxy hop itself, and dying with its child so the supervisor restarts
  the pair; and the api ships **pre-built** — `api/package.Dockerfile`
  compiles the musl-static binary in CI only, published as
  `ghcr.io/outcrop-labs/talaria-api` (tag `main` on every api-touching
  push — the same ref Dokploy's checkout builds build — plus immutable
  `sha-<sha12>` provenance tags and channel tags mirroring the app image's),
  and the app image consumes it with `COPY --from`, so no GitHub runner,
  operator machine, or customer VM ever compiles Rust to build the app
  image. healthz grew the third check (`rustApi`) and `talaria dev` now
  spawns the api by default (`TALARIA_API=off` opts out). The generated API
  reference froze at the cutover: its Source links point at the Rust modules
  (`api/src/routes/**`), `gen-docs` skips `docs/api/**` until its extractor is
  ported to the Rust router table (#293), and the CLI reference generates as
  before.
