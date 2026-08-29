# The Rust port

The API is moving from TypeScript to Rust: `ui/src/server` + `ui/src/routes/api`
(~80k LOC, 219 routes) port in dependency order to the `api/` crate (axum, sqlx,
redis-rs, hand-rolled reqwest clients), until the TS server is deleted. The
frontend stays Svelte/TS — the port ends with Rust serving the SPA, the API, SSE
and the scheduler, not with a rewrite of the product.

This is a **wholesale port, not a strangler-fig forever**: nothing is live and
there are no clients, which is the one window where the whole backend can change
shape. The coexistence proxy below exists so the app keeps working while the port
grinds — it is scaffolding, and its end state is deletion along with the rest of
the TS server.

When this page and the source disagree, the source wins.

## The rules of the port

These hold for every batch. Each one exists because breaking it breaks both
runtimes at once, not just the one being edited.

1. **TS owns the schema until batch 4.** The `MIGRATIONS` array in
   `ui/src/server/db/pg.ts` is append-only with sha256 checksums under an
   advisory lock; the Rust side **never migrates, never emits DDL** — sqlx reads
   and writes existing tables only, with runtime queries (no `query!` macros:
   the schema moves with the TS array, and compile-time checking against a
   per-devbox DB would be brittle). Batch 4 hands schema ownership to sqlx; until
   then a new column means a TS migration first, a Rust reader second.
2. **One route group, one runtime.** A migrated prefix serves from Rust or from
   TS, never both. The proxy has **no fallback on purpose**: if the Rust api is
   down, a migrated prefix answers 502 `upstream unreachable`, because silent
   fallback to the TS path is exactly the both-runtimes-serve-one-group failure
   mode the design refuses.
3. **Byte-parity with TS is the bar while both exist.** The UI is the client —
   shapes stay faithful because the SPA consumes them, not because the shapes are
   sacred. Verified by diffing responses from `:5273` (TS) against `:5274`
   (Rust) on the same dev DB before a prefix flips. serde structs are declared
   in wire order (serde_json `preserve_order` on round-trips) — `JSON.stringify`
   key order is observable behavior. Recorded divergences live at the bottom of
   this page.
4. **secretbox speaks both languages.** `api/src/secretbox.rs` is byte-compatible
   with `ui/src/server/secretbox.ts` (same KEK derivation, same token grammar),
   pinned by one committed fixture asserted from both sides
   (`api/tests/secretbox.rs`, `ui/src/server/secretbox.fixtures.test.ts`).
   Regenerate with `bun run api:vectors`; `api:check` fails on a stale fixture.
5. **The scheduler runs in exactly one runtime.** TS arms it today; Rust arms
   nothing. Batch 4 does the handoff (stop-TS → arm-Rust via the Redis lease, a
   handoff not a race) — until then any Rust code that could enqueue or reclaim
   work stays unwritten.
6. **`mcp/` stays TS**, and `GET /api/users` remains the fleet-wide auth oracle
   with its shape frozen — the MCP server authenticates every agent through it
   (`agent-auth.ts`), so the port must not move it until the session plane
   (batch 2) is served from Rust and proven.
7. **Same origin, no CORS.** Cookies are HttpOnly SameSite=Lax against one
   origin; the proxy is the only thing that ever hops between runtimes, on
   loopback, stripping hop-by-hop headers.
8. **Upstream error text dies at the boundary.** The gateway relays status codes
   and fixed sentences, never a provider's prose — the TS rule
   (`server/upstream-error.ts`), ported and byte-tested in `api/src/error.rs`.
9. **The unit suites are pure.** No test in `api/` or `ui/` needs a service in
   CI; the `#[ignore]`d integration tests that do touch the dev DB run locally
   with `cargo test -- --ignored`, never in CI. `bun run verify` never scans
   `api/`; the crate's gates are `bun run api:check` (fmt + clippy `-D warnings`
   + test) and the `api` CI job.

## How coexistence works

`ui/src/server/rust-proxy.ts` is the switch. A compiled `PREFIXES` list, checked
at the top of `app.ts`'s handler before the route table: a request under a
migrated prefix is forwarded to `TALARIA_RUST_API_URL` (loopback `:5274` in dev),
response bodies streamed through. Unset env — the default — forwards nothing:
every route is served by TS, byte-identical to before the port existed. An
`EXACT` list sits beside the prefixes for whole-path migrations — a route whose
sub-paths still belong to TS (`/api/agents` next to `register`/`heartbeat`,
`/api/apps` next to the app-server gateway) migrates by exact pathname, or the
prefix would strand those sub-routes on a Rust 404.

Flipping a group is two edits: the prefix joins `PREFIXES`, and `ui/.env` sets
`TALARIA_RUST_API_URL` (see `ui/.env.example` for the block). In dev,
`TALARIA_API=on` makes `talaria dev` start the Rust api as a sidecar (`cargo
run` in `api/`, adopting an instance already on the port, signals forwarded,
readiness polled without blocking the app); a devbox carries the whole toolchain
(`docker/devbox.Dockerfile` — pinned 1.97.1, mirroring `api/rust-toolchain.toml`).

## The batches

Dependency-ordered; each batch's prefixes flip only when its exit criteria hold
against the live dev stack.

| # | Scope | Exit criteria | Status |
|---|---|---|---|
| 1 | **LLM gateway** — `/api/llm/v1/*`: models, chat/completions (streaming + not), `tlk_` key auth with per-key caps and throttling, rate limits, the usage ledger, secretbox unseal, the confab guard (off/observe/annotate/strict, streaming caveat chunk, strict redaction) | byte-diff vs TS on models; live streams through the real provider hop with ledger rows either end of the stream; guard modes verified row-by-row in `guard_findings`; key caps enforced | **served from Rust** |
| 2 | **Sessions and auth** — Redis `sess:<sid>` sessions, Google OAuth (hand-rolled, no SDK), password credentials, invites, the users routes (oracle shape frozen until this lands) | a browser session held across a runtime hop; OAuth round-trip; the SPA fully usable with the session plane on Rust | **served from Rust** — the session store, `/api/auth/session`, `/api/auth/logout`, `/api/users` (oracle), `/api/auth/password`, `/api/auth/providers`, `/api/auth/claim`, `/api/auth/google` + `/callback`, all byte-diffed against TS on the same sessions and the same login counters (a Rust login verifies a node-hashed scrypt entry; either runtime's logout kills the other's session). The OAuth consent URL byte-matches TS's own serializer (same client record, same state token) and a live token-exchange failure bounces `exchange_failed` with the provider's prose dead in the log; the full happy path needs a human at Google's consent screen, so claim-via-google and the org-domain/invite doors are verified to their last reachable hop, not past it. The admin invites console lands with batch 3's admin surfaces |
| 3 | **Product reads in bulk** — boards, orgs, settings, agent reads, the read side of every surface the SPA lists | byte-diff per route family; the SPA's boards/orgs/agents surfaces on Rust reads | **in flight** — `/api/agents`, `/api/apps`, `/api/activity`, `/api/cost` serve from Rust (byte-diffed on the same sessions for admin/member/anon callers, including the audit-kind filter and the member Observability 403). Deferred with their planes: `/api/alerts` + admin `/api/home` (computeAlerts probes scheduler/mail/docker/MCP state that lives in the TS process until batch 5), `/api/history` (kb + artifacts read models), `/api/inference` (docker socket + the gateway pulse). The boards family (tasks/statuses intertwined + the retrieval write corner) is the next slice |
| 4 | **Runs engine + scheduler** — durable research/plan runs, per-step leases, the registered jobs; **schema ownership hands to sqlx here** (TS `MIGRATIONS` freezes, future migrations are sqlx's) | a run resumed across a restart; the scheduler armed in Rust with TS disarmed via the lease handoff; a migration issued from Rust applied once under the same advisory-lock discipline | — |
| 5 | **The tail** — retrieval (Qdrant + TEI + hand-rolled SearXNG client mirroring `search.ts`), SSE fanout (Redis pub/sub, id-shaped payloads), chat/channels, uploads, email, fleet rendering | every remaining `/api/*` prefix served from Rust; `PREFIXES` reduced to `/api/` | — |
| ∎ | **Cutover** — the TS server deleted; Rust serves the SPA, SSE and the scheduler; the prod image gains a Rust build stage (musl static — devboxes are glibc, prod is alpine) | `ui/src/server` gone; `bun run start` is the Rust binary; proxy and `TALARIA_RUST_API_URL` deleted with it | — |

## Recorded divergences

Everything found while porting where Rust cannot (or should not) match TS
byte-for-byte. This list is the contract: a divergence not here is a bug.

- **Model-list ordering.** TS sorts `localeCompare`, Rust sorts bytes. They agree
  on the ASCII lowercase ids the registry generates; models order isn't
  contractual. (`gateway/models.rs`)
- **App discovery reads disk.** TS discovers apps via build-time
  `import.meta.glob` and sorts `localeCompare`; the Rust port reads
  `apps/<slug>/talaria.json` from disk and sorts bytes (`api/src/users.rs`,
  `appViewRoutes`). Same directory in dev; the difference needs a build that
  compiled an app in and then lost its source tree — not a reachable state.
  The same entry covers the `mcp` flag (`api/src/users.rs`, `enabledApps`):
  TS asks its build-time glob whether `apps/<slug>/mcp.ts` was compiled in;
  Rust checks the file exists on disk.
- **Corrupt scrypt rows fail closed here, 500 there.** TS parses hash-entry
  numbers with `Number()` (admitting `"1e4"`, hex) and Node's base64 decoder
  decodes past junk, so a mangled `scrypt$…` row can reach node:crypto and
  throw; the Rust parse rejects the same row and the login is a plain 401
  (`api/src/password.rs`). An entry that malformed was never a credential
  anyone could present.
- **`/api/auth/providers` answers `configured: true` unconditionally.** TS
  computes it from env presence (`DATABASE_URL && REDIS_URL`); the Rust process
  refuses to boot without both, so the "not configured" warning the flag exists
  to surface has no state to describe here (`api/src/routes/auth_providers.rs`).
- Nothing else yet.

## Layout of the crate

`api/src/` — `main.rs` (axum serve, graceful shutdown), `config.rs` (env, the
secret-root precedence `TALARIA_SECRET_KEY` → `_FILE` → `AUTH_SECRET`),
`error.rs` (the two envelopes, byte-tested), `db.rs` (pool; the schema-ownership
comment), `state.rs`, `auth.rs`, `ratelimit.rs`, `secretbox.rs`;
`routes/` (one file per prefix, mirroring `ui/src/routes/api` shapes);
`gateway/` (the LLM gateway's internals: registry, vault, provider, params,
usage, budget, guard). `Cargo.lock` is committed, as `bun.lock` is.
