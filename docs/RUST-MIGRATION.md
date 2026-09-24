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

Local cargo is capped at 2 jobs — `api/.cargo/config.toml`, and the same number
in `desktop/src-tauri/.cargo/config.toml`. That cap is the CPU cap and the RAM
cap: each rustc is a multi-GB process on cargo's jobserver, and an uncapped
workspace `cargo test` pins the host. Agents run `bun run gate`, which compiles
only the packages the diff touches, and do not lift the cap. CI lifts it
(`CARGO_BUILD_JOBS` from `setup-runtime`, and the same build-arg on the package
image). `bun run api:check` remains the CI api job's local spelling, not the
command an agent runs before a pull request.

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
   one committed fixture asserted from both sides (`api/tests/it/secretbox.rs`,
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
   `#[ignore]`d integration tests that touch a live DB run in
   `api-integration.yml` against scratch containers (`cargo test -- --ignored`
   still works locally against `talaria dev` infra — that is where
   `typed_binds` and `update_live`, which need more than Postgres+Redis, stay).
   `bun run verify` never scans `api/`; the CI gate is `bun run
   api:check` (fmt + clippy `-D warnings` + test) and the `api` CI job, with
   `api-integration.yml` the depth gate behind them. Locally that command is not the one to run — `bun run gate` compiles the packages the diff touches, under the job cap above.
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

## Layout of the workspace

`api/` is a Cargo workspace: `talaria-api` is a thin binary (`main.rs` plus an
alias-only `src/lib.rs`) and every engine lives in its own crate under
`api/crates/talaria-*` (206 at the time of this writing). `talaria-api-routes`
holds the handler modules and the router table; `talaria-jobs` is the
composition root that wires the cross-crate seams.

### Where new code goes

1. **A new domain gets a new crate** — `api/crates/talaria-<domain>/`, one
   concern, deps declared as `{ workspace = true }` from
   `[workspace.dependencies]` (membership is the `crates/*` glob, so the
   directory IS the registration). Crate-per-domain is the point of the split;
   do not grow a grab-bag crate.
2. **Dependencies point DOWN only** — leaf crates (config, db, error, body,
   secretbox, state, realtime…) at the bottom; platform crates (gateway);
   engines above them; `talaria-api-routes` and the binary at the top. If a
   lower crate needs something above it, the seam becomes a `OnceLock`
   injected at boot in `talaria-jobs` (see `CALL_MCP_TOOL`,
   `BUILD_DISPATCH`, `GET_TASK` for the pattern) — never an upward import.
3. **Types outrank engines** — shared row/patch/actor shapes live in a
   `-types` crate (`talaria-tasks-types`, `talaria-inbox-focus-types`) so two
   engines can share them without either depending on the other.
4. **A run kind lives next to its driver** — `talaria-research-def`,
   `talaria-runs-plan-draft`, `talaria-runs-work-session`… registered in
   `talaria-jobs`' arming census so a kind that falls out fails boot.

### The crate families

| Family | Crates (pattern) |
| :--- | :--- |
| binary | `talaria-api` (main.rs + aliases), `talaria-api-routes` (handlers, router) |
| composition | `talaria-jobs` (scheduler jobs + every OnceLock injection) |
| engines | `talaria-{harness,fitness,retrieval,kb,tasks,runs-*,fleet-*,google-*,mcp*,inbox-focus*,daily-brief*,workbench*,approvals,digest,research*,alerts,…}` |
| defs | `talaria-harness-defs` (every harness definition + registry), the `-def` run kinds |
| platform | `talaria-gateway`, `talaria-scheduler`, `talaria-notify`, `talaria-audit` |
| leaves | `talaria-{config,db,error,body,secretbox,state,auth,session,realtime,tz,yaml,…}` |

Names follow the module they replaced: `src/tasks.rs` → `talaria-tasks`,
`src/google/drive.rs` → `talaria-google-drive`. When you extract the next
one, keep the moves mechanical — rewrites to crate paths, the OnceLock only
where a cycle forces it — and run the full gate (`clippy -D warnings`,
`cargo test --workspace`, `bun run check`) before committing.

`Cargo.lock` is committed, as `bun.lock` is. The shared target dir stays
`api/target` so `.dockerignore` and the package Dockerfile's context keep
working unchanged.
