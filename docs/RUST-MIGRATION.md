# The Rust api

The backend is the `api/` crate — axum, sqlx, redis-rs, hand-rolled reqwest
clients. It owns every `/api/*` route, the SSE streams, and the scheduler. The
SvelteKit server (bun) stays the SPA host forever: it serves the app and hands
`/api/*` to the api on loopback, which is what keeps cookies same-origin
(rule 7). Three residents stay TS, permanent by rule: the app dispatch
(`/api/apps/…` subtree — bare `/api/apps` is the api's), the app-MCP gateway
branch (`/api/mcp/gw/app-*`), and `healthz`. The one-time fourth (`admin/update`,
the git-checkout updater that rebuilt `ui/dist` and restarted the very process
serving it) retired with its updater: the update surface is the api's
(`/api/admin/updates` rolls containers — see [`UPDATES.md`](./UPDATES.md)).
The api ships as a **pre-built
package image** (`ghcr.io/outcrop-labs/talaria-api`, musl static) compiled only
in CI by `api/package.Dockerfile` and consumed by the prod image with
`COPY --from` — no app-image build (GitHub runner, operator machine, or the
Dokploy checkout a customer VM builds on each push) ever compiles Rust.

How the api got here — the TS→Rust port, batch by batch, with the parity
battery that proved each slice — is the dated record in
[`history/rust-port.md`](./history/rust-port.md). This page is the living law:
the hop, the rules, the wire divergences that are contract. When this
page and the source disagree, the source wins.

## The hop

[`ui/src/server/rust-proxy.ts`](../ui/src/server/rust-proxy.ts) is the whole
boundary. `PREFIXES = ['/api/']` — everything under `/api/` hops — with the
TS residents above matched `STAY_TS` first. The target is
`TALARIA_RUST_API_URL`, defaulting to `http://127.0.0.1:5274` (the port
`talaria dev` and `server-entry` bind their api to): **the hop is assumed**, so
first spin-up — dev or prod — proxies the moment an api is listening. The
literal `off` stands the boundary down, for installs supervised some other way;
there is no third state. If the api is down, `/api/*` answers
502 `{"error":{"message":"upstream unreachable"}}` — no fallback, on purpose
(rule 2).

In dev, `talaria dev` starts the api as a sidecar by default (`cargo run` in
`api/`, adopting an instance already on the port, signals forwarded, readiness
polled without blocking the app; `TALARIA_API=off` opts out), and `healthz`
reports the effective api URL and probes it. A devbox carries the whole
toolchain (`docker/devbox.Dockerfile`, pinned like `api/rust-toolchain.toml`).

## The rules

1. **The schema is owned by the TS `MIGRATIONS` array — the api issues no DDL.**
   Migrations are append-only, sha256-checksummed per statement under advisory
   lock `8_314_207`, committed with their bookkeeping row in one per-statement
   transaction; the `schema_migrations` shape predates the api (integer id +
   checksum of the whitespace-collapsed statement, applied per-STATEMENT) and
   is kept so pre-port databases keep booting. Growth-only: a different
   checksum at an applied id refuses to boot, and that refusal is the guard
   working. (An earlier revision of this rule said the array was frozen and
   sqlx would own new migrations — that was never implemented; the array
   stayed live through the cutover and remains the single channel for schema
   changes and one-time data operations alike. See `api/src/db.rs`.) Runtime
   queries only — no `query!` macros: every devbox has its own database, and
   compile-time checking would couple the build to one schema.
2. **One origin, no fallback.** The api serves a prefix or nothing: if it is
   down, the hop answers 502 `upstream unreachable`. Silent fallback is the
   failure mode this shape refuses.
3. **Wire shapes are contract.** The SPA consumes them, and key order is
   observable — serde structs are declared in wire order, stored jsonb
   passthroughs ride the raw `serde_json::Value` (Postgres canonical order,
   not a typed struct's declaration order), numerics cross as strings, floats
   parse round-trip-exact (`float_roundtrip`). The divergences recorded below
   are the complete exceptions list: a divergence not there is a bug.
4. **secretbox speaks both languages.** `api/src/secretbox.rs` and
   `ui/src/server/secretbox.ts` seal and open the same rows — the TS side still
   spends workspace secrets through the app-MCP gateway — so the cipher is a
   live two-way contract: same KEK derivation, same token grammar, pinned by
   one committed fixture asserted from both sides (`api/tests/secretbox.rs`,
   `ui/src/server/secretbox.fixtures.test.ts`). Regenerate with
   `bun run api:vectors`; `api:check` fails on a stale fixture.
5. **The scheduler runs in exactly one process.** `TALARIA_SCHEDULER` is the
   kill switch and nothing else: unset (the default) arms the api's job table,
   `off` arms nobody. The shared `sched` lease namespace backstops a value
   gone wrong besides, and the arm refuses to go up until the census's six run
   kinds all have definitions (its boot log names the missing ones).
6. **`mcp/` stays TS**, and `GET /api/users` remains the fleet-wide auth oracle
   with its shape frozen — the MCP server authenticates every agent through it.
7. **Same origin, no CORS.** Cookies are HttpOnly SameSite=Lax against one
   origin; the proxy is the only thing that ever hops between the two servers,
   on loopback, stripping hop-by-hop headers.
8. **Upstream error text dies at the boundary.** The gateway relays status
   codes and fixed sentences, never a provider's prose (`api/src/error.rs`).
9. **The unit suites are pure.** No test in `api/` needs a service in CI; the
   `#[ignore]`d integration tests that touch the dev DB run locally with
   `cargo test -- --ignored`. `bun run verify` never scans `api/`; the crate's
   gates are `bun run api:check` (fmt + clippy `-D warnings` + test) and the
   `api` CI job.
10. **App modules are customer code, never port surface.** Building a microapp
    with the SDK stays a TS/node experience: an app's internal APIs are the
    author's own code, talking to the host through the same `/api` and UI
    surfaces everyone else uses. The app-server gateway is host plumbing and
    stays TS; Rust API modules are the opt-in advanced tier, which coexists
    with (never replaces) the TS default. Rule 2 governs host routes, never
    app-owned code.

## Recorded divergences

Found while porting, frozen here: each entry is behavior the api has **on
purpose**, decided against the TS route it replaced. This list is the contract
— a divergence not here is a bug.

- **The fitness plane counts what an enabled app shipped.** App harnesses
  (`apps/<slug>/harnesses/*.ts`) are customer code loaded as code, not data
  the api can read (rule 10) — the registry cannot see them. On an install
  with an enabled app (dev's leadworks ships 4 harnesses, 35 fixtures) the
  matrix/bare `registry` counts, the value view's workload keys and every
  aggregate they feed (`perDay`, `shares`, `usdPerReadyRun`, the
  `unmeasured` list), and the estimate's fixture-derived arithmetic plus the
  note branch the unmeasured count selects all exclude exactly that set.
  On an install with no enabled app harnesses, nothing is excluded.
  (`fitness/surface.rs`, `fitness/value.rs`)
- **Fitness `clear` works.** The TS-era route 500'd on every model — its
  report-clearing write put SQL NULL into a NOT NULL column — while the run
  rows and transcripts cleared fine. The api's `clear` deletes its rows and
  answers 200 with the counts, which is what the verb has always meant.
- **Model-list ordering sorts bytes, not locale.** The registry's ids are
  ASCII lowercase and the order agrees everywhere but one corner: an endpoint
  name with a capital (`Z.ai/glm-5.3`) collates last under locale-aware
  comparison and first under byte order. Models order isn't contractual (a
  picker's display order). (`gateway/models.rs`, `model_access.rs`)
- **App discovery reads disk.** Apps are discovered by reading
  `apps/<slug>/talaria.json` from disk (byte-sorted); the `mcp` flag checks
  whether `apps/<slug>/mcp.ts` exists on disk. The difference from a
  build-time glob needs a build that compiled an app in and then lost its
  source tree — not a reachable state. (`api/src/users.rs`)
- **Uncaught route errors answer the house envelope.** A route failure the
  handler doesn't catch (a non-uuid `{id}` reaching a raw SQL bind on
  `/api/keys/{id}`, a client-id that trims to nothing on
  `PUT /api/admin/google-client`, a fractional `?since=2.5` reaching the int4
  comparison on `/api/channels/{id}/messages`, a non-uuid `{id}` on
  `/api/conversations`) logs the cause and answers
  `{"error":"internal error"}` — never a bare text body, nor a provider's or
  Postgres's own sentence on the wire (rule 8's discipline applied to our own
  failures). (`routes/models/keys_id.rs`, `admin/admin_google_client.rs`,
  `comms/channels_id_messages.rs`, `comms/conversations_id.rs`)
- **Corrupt scrypt rows fail closed.** A mangled `scrypt$…` hash entry
  rejects and the login is a plain 401 rather than a thrown decode
  (`api/src/password.rs`). An entry that malformed was never a credential
  anyone could present.
- **`/api/auth/providers` answers `configured: true` unconditionally.** The
  process refuses to boot without `DATABASE_URL` and `REDIS_URL`, so the
  "not configured" warning the flag exists to surface has no state to
  describe (`routes/account/auth_providers.rs`).
- **The instance domain can be cleared.** `PUT /api/admin/instance
  {"domain":null}` encodes a real jsonb null, so the upsert lands: 200,
  `{instance: null}`. (The TS-era route leaked a raw Postgres 400 sentence
  here and could never clear the domain at all — a bug this api does not
  reproduce.) (`api/src/instance.rs`)
- **The blurb sweep is a registered job.** `maybeRewriteBlurbs` runs on the
  schedule (`api/src/jobs.rs`); it is not a route side effect, so no request
  path needs to touch it and the rows it rewrites are visible to every
  reader the moment they land.
- **The blurb clamp cuts at a char boundary, not a surrogate boundary.** A
  Rust string cannot hold half a surrogate pair, so the 157-unit clamp stops
  at the last whole character that fits. Reachable only with a >160-unit
  first sentence whose 157th unit falls inside an astral character.
  Cosmetic. (`api/src/model/info.rs`)
- **The MCP library serves an empty shelf when the registry never answers.**
  A refresh only replaces a cached shelf with a non-empty one, so a dead
  registry reads as `[]` rather than 502. (`routes/mcp/mcp_library.rs`)
- **A permissions-read failure at the fleet crons/secrets gate answers 403,
  not 500.** The gate folds a failing read into denial — a member is refused
  rather than shown a server error. Reachable only when the permissions read
  itself fails mid-request; the denial is the safer side of the same failure.
  (`routes/fleet/fleet_agents_id_crons_jobid.rs`,
  `routes/fleet/fleet_agents_id_secrets.rs`)
- Nothing else yet.

## Layout of the crate

`api/src/` — the scaffold first: `main.rs` (axum serve, graceful shutdown),
`config.rs` (env, the secret-root precedence `TALARIA_SECRET_KEY` → `_FILE` →
`AUTH_SECRET`), `error.rs` (the two envelopes, byte-tested), `db.rs` (pool +
the migration discipline), `state.rs`, `auth.rs`, `ratelimit.rs`,
`secretbox.rs` (the cross-language cipher, vector-pinned from both sides).
Then one dir per engine family, and the routes one dir per subsystem:

- `routes/` — 23 subsystem dirs mirroring the `docs/api` groups
  (`routes/boards/boards_id.rs`, `routes/fleet/fleet_defs.rs`, …); the router
  table in `routes/mod.rs` names the system each handler belongs to
- `runs/` + `scheduler.rs` + `jobs.rs` — the durable-run engine (leases, CAS,
  reclaim, decide), the sweep, and the registered job table
- `gateway/` — the LLM gateway: registry, vault, provider, params, usage,
  budget, guard
- `harness/` + `fitness/` — the persona/capability engine, and the
  model-fitness battery
- `fleet/` — render, reconcile, preflight, docker, cascade, federate: the
  fleet's whole write plane
- `google/` — OAuth, connections, calendar/drive/gmail, provisioning
- `mcp/`, `model/`, `kb/`, `inbox_focus/`, `workbench/`, `retrieval/`,
  `daily_brief/` — the other engine families
- the top level holds the singles that never grew a family — `body.rs` (the
  validation engine, every 400 sentence probed and pinned), `password.rs`,
  `session.rs`, `realtime.rs` (the SSE fanout), `yaml_string.rs` (the
  byte-identical `yaml` emitter, fixture-pinned), `workspace_secrets.rs`,
  and friends

`Cargo.lock` is committed, as `bun.lock` is.
