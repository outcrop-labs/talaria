# The Rust port

The API is moving from TypeScript to Rust: `ui/src/server` + `ui/src/routes/api`
(~80k LOC, 219 routes) port in dependency order to the `api/` crate (axum, sqlx,
redis-rs, hand-rolled reqwest clients), until the TS API is deleted. The scope is
the BACKEND and nothing else — the SvelteKit server keeps serving the SPA, its
pages and their routing exactly as it does today; the port ends with Rust owning
every `/api/*` route, SSE and the scheduler, not with a rewrite of the product
and not with Rust hosting the UI.

This is a **wholesale port, not a strangler-fig forever**: nothing is live and
there are no clients, which is the one window where the whole backend can change
shape. The coexistence proxy below exists so the app keeps working while the port
grinds — at cutover the TS API route table behind it is deleted, but the hop
itself is the end-state architecture, not scaffolding: the SvelteKit server stays
the one public origin and hands `/api/*` to Rust on loopback, which is what
keeps the cookies same-origin (rule 7).

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
5. **The scheduler runs in exactly one runtime.** `TALARIA_SCHEDULER` is the
   whole switch, read by both sides: unset = TS arms (every existing
   deployment), `rust` = the Rust api registers and arms the table while TS's
   `startScheduler` stands down, `off` = nobody arms. One value in one env
   file, so there is no window where both runtimes think a period is theirs —
   and the shared `sched` lease namespace backstops a botched value besides.
   The Rust arm (`api/src/jobs.rs`) refuses to arm until the census's six run
   kinds all have definitions (its boot log names the missing ones), because
   `run-reclaim` strands rows of kinds it cannot define once TS's sweep is
   down. Until a deployment sets `rust`, Rust arms nothing and TS's schedule
   is untouched.
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
10. **App modules are customer code, not port surface.** Building a microapp
   with the SDK stays a TS/node experience: an app's internal APIs are the
   app author's own code, talking to the host through the same `/api` and UI
   surfaces everyone else uses, and the port never reaches into
   `apps/<slug>/` to rewrite that. The app-server gateway (`/api/apps`,
   `apps.$app.$` dispatch) may cross to Rust as HOST plumbing, but the
   modules it dispatches into keep their TS/node runtime forever — that is
   the accessibility promise, not a migration debt. Rust API modules are the
   advanced tier: a self-contained option an app author can opt into, which
   coexists with (never replaces) the TS default. Rule 2's
   one-runtime-per-group governs the host's routes, never app-owned code.

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
| 3 | **Product reads in bulk** — boards, orgs, settings, agent reads, the read side of every surface the SPA lists | byte-diff per route family; the SPA's boards/orgs/agents surfaces on Rust reads | **served from Rust** — `/api/agents`, `/api/apps`, `/api/activity`, `/api/cost`, `/api/keys` (+ `/api/keys/{id}`), `/api/teams` (+ `/{id}` and `/{id}/members`), `/api/workflows` (+ `/{id}`), `/api/notifications`, `/api/me`, `/api/models` (+ `/api/models/efforts`), `/api/admin/model-roles`, and the admin console's wave-1 groups serve from Rust: `/api/agent-role-templates`, `/api/admin/password-accounts`, `/api/admin/google-client` (+ its `/login`), `/api/admin/instance`, `/api/admin/permissions`. The teams list is the port's first ACTING-user surface: a personal assistant's `tak_` key proxies to its owner (a general agent or a rejected credential resolves to nobody — the plain 401, never agent-auth's refusal). The workflows pair is the port's first zod-ARRAY surface — the 400 table behind match/skills/toolkits (element checks outrank the array-length check, the trim runs before the length bounds, unknown keys strip at every level) is probed and pinned in `body.rs` tests, and its two quiet corners held on both sides: no 404 anywhere (a missed id updates or deletes nothing and still answers ok), and an EMPTY PUT patch runs no SQL at all — so it answers ok even for a non-uuid `{id}` while a patch with one field present takes the recorded platform-500 divergence. All byte-diffed on the same sessions for anon (401), member (403), and admin callers — the reads literally, the writes through restore-safe cycles (template upsert/shadow/delete, account create/set/remove, org-default and per-user override set/clear, login toggle, domain set/verify, key mint/policy-set/policy-clear/revoke with the caps read back through each runtime, team create/rename/delete and the add/re-role/remove member cycles, workflow create/patch/delete with jsonb key order read back through each runtime), the 405s with their `allow` headers, and the full zod 400 table including the permissions union's one non-generic message (a string userId failing the uuid format check answers `Invalid UUID`; every other both-branches-fail shape answers `Invalid input` — probed against zod 4.3.6's sole-non-aborted-branch return, pinned in `admin_permissions.rs` tests) and the nullish z.number() table behind the key policy (probed and pinned in `body.rs` tests: the safe-int guard's own bounds, a max breach that says "number" even on an int field, and 0 echoing back as 0 while the row stores unlimited). The notifications route is the port's first zod-RECORD surface — the prefs patch's whole 400 table (the record's own type word, the generic `Invalid key in record` past 40 chars that is NOT the string-max message, the enum's message for a wrong-type value, the field's two refines outranking the root's) is probed and pinned in the route's tests — and its boundaries held on both sides: the mail fan-out behind the prefs (sendGatedMail, the outbox, the drain) is scheduler plane and stays TS (rule 5, batch 5), the brief-nudge lands without its SSE publish, ids absent AND ids empty both mean "mark all of mine", the member's delivery 403 fires after the 400s and before any write so one PATCH never half-applies, and the admin delivery flips are audited identically on both runtimes (the audit rows mirror). `/api/me` migrates by EXACT pathname — its me.* siblings (`me.mcp`, `me.assistant`, `me.events`) are the fleet/agent/SSE planes and move whole with batch 5 — and carries the port's first probed ICU behavior: `isValidTimeZone` asks Intl to resolve the name, so the acceptance grammar itself was probed as a 100+-spelling table on which node's and bun's ICUs agreed on every row (IANA names match case-insensitively, `±HH`/`±HH:MM`/`±HHMM` offset forms resolve up to 23:59, `Factory` is refused) and is mirrored with chrono-tz's tzdb plus a hand-rolled offset parser, the table pinned in `api/src/me.rs` tests. The route's own corners held byte-for-byte: the zod bounds run on the RAW string and the handler's trim comes after (a spaces-only name is legal and stores `""`), the PUT applies fields in me.ts's sequence with no transaction (a name lands in the row and the live session — the KEEPTTL patch — before a refused model or zone answers the same request), the member model gate reads the allowlist and the catalog exactly as model-access.ts does (a qualified id is judged by its pinned bare model; an id no endpoint serves is refused), and null is a legal VALUE for the nullable trio, distinct from absent, via `present_nullable_string_member`. The boards family moves whole to batch 4, not slice-wise here: its enqueue/reclaim corner is rule-5 territory, and a method-split boards (reads here, writes there) breaks one-runtime-per-group. Deferred to their owning batches: `/api/alerts` + admin `/api/home` (computeAlerts probes scheduler/mail/docker/MCP state that lives in the TS process until batch 5), `/api/history` (kb + artifacts read models), `/api/inference` (docker socket + the gateway pulse), `admin/invites` (createInvite sends email — batch 5), `admin/settings` (the fleet-docker import), `memory/{id}` (readMemory/writeMemory are `docker exec` into the running agent container — the same plane as `/api/inference`), and the model-identity plane crossed as the batch's closing slice — `models`, `models/efforts`, and `admin/model-roles` all stand on the harness persona engine, so they ported as one unit. The engine came with them: the persona index's load-bearing two-pass claim (base ids first, then alias tiers — the tier pass has NO main-configured gate, an alias with a valid target claims its tier id even when main is unset or malformed, while a base id without a main claims nothing; every claimed pool carries the agent's fallbacks, because the runner can land on them), the effort ladder (provider catalog plus the admin declaration, which REPLACES and never merges, `EFFORT_ORDER` with unknowns appended, the once-only backfill when a stored catalog predates the efforts extraction), the model-info matcher (OpenRouter's public catalog — full id for slashed, unique suffix for bare, dots↔dashes, 6h stale-on-failure cache — and the `model_blurbs` override read whose error PROPAGATES to the route's 500, TS `Promise.all` parity), the resolution chains (`pin→role→utility→env→first-routable` and the muse chain, the member allowlist gating role/utility/preferred/first-routable but never pin/env, `routes()` advancing the rr cursor on purpose), and the ISO-8601 pair in `agent_auth.rs` — the house has no clock crate, so the formatter gained its strict inverse `iso_to_epoch_ms` and capability's expiry reads parse without a dependency. Verified by a 37-case byte-diff on the same sessions: the three routes × anon/member/admin, the whole zod 400 table behind the PUT (role enum first, then model bounds, then effort bounds — zod's schema order), restore-safe write cycles run through BOTH runtimes (assign, effort set against the published ladder, effort clear, model null and `""` clears, effort on an unassigned role refused, an unpublished level refused with the publish sentence, the audit rows mirroring), the 405s with their `allow` headers, the effort feed's query corners (missing/empty/whitespace model, percent-encoded ids, and a DUPLICATE `?model=` — `URLSearchParams.get` is first-wins, and both sides must show it), and the persona-configured default through a live config mutation read only after both runtimes' 60-second persona caches expired. Three cases differ in array ORDER alone — the recorded localeCompare divergence, invisible until an endpoint name with a capital letter joined the registry: TS collates `Z.ai/glm-5.3` last, byte order puts it first, and both lists carry the same rows |
| 4 | **Runs engine + scheduler** — the engine core (`runs/lease`, `store`, `define`, `run`, `decide`, `reclaim`, ~3.5k lines: Redis leases with CAS renew/release Lua and per-attempt tokens, every `runs` write a CAS on `(id, lease_owner, state)`, `attempt` incremented only on a reclaim so a healthy 400-step run can never exhaust `maxAttempts`, the step-budget lease TTL, persist-then-publish ordering, and the `awaiting` guard that parks a decision rather than failing it — `run.ts` also documents two deliberately deleted exports, a second in-file reclaim sweep and an ungated `answerRun`; a Rust port that re-derives either re-creates the bug), the six run kinds behind `runs/boot.ts` (agent-hire, plan-draft, rag-backfill/rag-reindex, research, work-session — work-session drags the harness capability layer, which is where the ticket/board writes actually happen), the boards family (held out of batch 3 for exactly this: task create → `maybeDispatchTicket` → a work-session enqueue, and status moves re-enter `updateTask`'s re-dispatch branch), the `run_decision` half of the approvals census, the runs/me/boards SSE streams (one dedicated Redis subscriber per client), and the whole registered job table — comms-decay, outreach-sweep, price-refresh, daily-digest, approval-escalation, notification-mail (the module's outbox and drain, the one already-ported route whose module still holds a hand inside the scheduler), run-reclaim, daily-brief, mcp-library-refresh, update-check — plus `maybeRewriteBlurbs`, which becomes a REAL job here: its only TS trigger was the `/api/models` handler, which the coexistence proxy shadows, so the org-voice sweep is dark in any proxied environment until this batch ports it. The flip is one slice — stop-TS → arm-Rust, never both — so everything the ten jobs touch must be in Rust first, which pulls the mail transport and the enqueue-calling routes (boards writes, research decide, plan-drafts, admin.rag, fleet create/hires) into this batch's blast radius even where batch 5 had them; the engine's dependency arrow is runs → gateway (already ported), never the reverse. THE FLIP MECHANISM HAS LANDED AND IS ARMABLE: `TALARIA_SCHEDULER=rust` hands the whole schedule over (`api/src/jobs.rs` registers and arms the table, TS's `startScheduler` stands down on the same value, the models route's blurb kick retires for the blurb job, `dispatch_deps` swaps the coexistence stub for the real driver edges, and the five enqueue sites drive inline exactly as TS did); the arm still holds itself to the census's six run kinds, and that table is WHOLE — the retrieval session's reindex pair (rag-backfill/rag-reindex) crossed with the retrieval plane's own halves (artifact-routing and the health probe landed beside it), so the pair's defs, real deps, arms, and start verbs are all Rust, and the guard's empty-list assertion is now a regression tripwire rather than a checklist (agent-hire crossed with the fleet write plane: the def, its real deps, the arm, and the fleet create/hires routes). The reindex pair's last seam closed with the admin.rag route: the retrieval console crossed whole (GET health + both run projections + the 60s upgrade status, PUT the reranker config and space↔brain bindings, POST the kicks that make the pair's start verbs live enqueue sites and the live rerank model catalog), and with the upgrade-status cache on the Rust side the reindex run's `invalidate` edge got its real body — a rebuild drops the cache its own panel reads, in the process that rebuilds. The bare `/api/rag/*` family crossed with the console (the collection registry — GET list with the member picker's blanked binding matrix vs the admin's full rows, POST create with its 400-carried create sentences; the `{id}` bindings replace and delete, where the auto-collection refusal is the one 400 and every other failure is TS's platform 500; and the POST search the MCP `search_knowledge` tool rides, agent-key or session caller alike) — the registry is what the search resolves principals against, so the family crosses as one unit. `update-check` is a deliberate HOLD past the flip: its apply half pulls, rebuilds ui/dist and restarts the TS server — choreography that cannot rebuild or restart the Rust binary, so an update driven from Rust would leave the two artifacts diverged with a green checkmark; its state/check/reconcile halves stay reachable through the unproxied admin routes, manual apply keeps working there, and the auto half returns when batch 7 settles the two-artifact restart topology. **Schema ownership hands to sqlx here** (TS `MIGRATIONS` freezes, future migrations are sqlx's) | a run resumed across a restart; the scheduler armed in Rust with TS disarmed — the lease handoff; a migration issued from Rust applied once under the same discipline: the same `schema_migrations` shape (integer id + sha256 of the whitespace-collapsed statement — the applied set is per-STATEMENT, not per-file, and sqlx's own `_sqlx_migrations` table cannot replace it), the same advisory lock `8_314_207` (a mixed TS/Rust boot must serialize on the SAME key), a dedicated single-connection migration pool that is never reaped, per-statement transactions committing the statement and its bookkeeping row together, `initSecretbox` inside the pass, and growth-only re-arm — a different checksum at an applied id still refuses to boot, and that refusal is the guard working, not a bug to soften | — |
| 5 | **The tail** — retrieval (Qdrant + TEI + hand-rolled SearXNG client mirroring `search.ts`), SSE fanout (Redis pub/sub, id-shaped payloads), chat/channels, uploads, email, fleet rendering | every remaining `/api/*` prefix served from Rust; `PREFIXES` reduced to `/api/` | — |
| ∎ | **Cutover** — the TS API deleted (`ui/src/routes/api` and the backend of `ui/src/server`); the SvelteKit server keeps serving the SPA and proxies every `/api/*` request to Rust (SSE and scheduler included); the prod image gains a Rust build stage (musl static — devboxes are glibc, prod is alpine) | `ui/src/routes/api` gone and `ui/src/server` reduced to what the frontend server itself needs (the handler and the proxy hop); `PREFIXES = ['/api/']`; `bun run start` still boots the SvelteKit server, now with the Rust api as its backend process; the SPA fully usable end to end with no TS API behind it | — |

## Recorded divergences

Everything found while porting where Rust cannot (or should not) match TS
byte-for-byte. This list is the contract: a divergence not here is a bug.

- **Model-list ordering.** TS sorts `localeCompare`, Rust sorts bytes. They agree
  on the ASCII lowercase ids the registry generates, and diverge the moment an
  endpoint name carries a capital: `Z.ai/glm-5.3` collates LAST under
  `localeCompare` (z after o) and FIRST under byte order (0x5A before 0x6F) —
  observed, not hypothetical, in the `/api/models` and `/api/admin/model-roles`
  diffs, where the two lists carry the same rows in different order. Models
  order isn't contractual (a picker's display order). (`gateway/models.rs`,
  `model_access.rs`)
- **App discovery reads disk.** TS discovers apps via build-time
  `import.meta.glob` and sorts `localeCompare`; the Rust port reads
  `apps/<slug>/talaria.json` from disk and sorts bytes (`api/src/users.rs`,
  `appViewRoutes`). Same directory in dev; the difference needs a build that
  compiled an app in and then lost its source tree — not a reachable state.
  The same entry covers the `mcp` flag (`api/src/users.rs`, `enabledApps`):
  TS asks its build-time glob whether `apps/<slug>/mcp.ts` was compiled in;
  Rust checks the file exists on disk.
- **Uncaught TS route errors answer the platform's plain-text 500; the port
  answers the house envelope.** Same status, different body: TS's server has
  no catch-all boundary, so a route throw the handler didn't catch (a non-uuid
  `{id}` reaching a raw SQL bind on `/api/keys/{id}`, a client-id that trims
  to nothing on `PUT /api/admin/google-client`) surfaces as bare
  `Internal Server Error` text. The port logs the cause and answers
  `{"error":"internal error"}` — rule 8's discipline applied to our own
  failures (`api/src/routes/keys_id.rs`, `admin_google_client.rs`).
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
- **Clearing the instance domain works here and cannot on TS.** postgres.js
  turns `sql.json(null)` into SQL NULL, so TS's `PUT /api/admin/instance
  {"domain":null}` violates `app_settings.value`'s NOT NULL constraint and the
  route's validation catch leaks the raw Postgres sentence as a 400 — the
  domain can never be cleared at all. sqlx encodes a real jsonb null, so the
  upsert lands: 200, `{instance: null}`, and both runtimes' readers see null
  (`api/src/instance.rs`). The TS catch was built for the validation throw;
  the DB-failure leak is the bug this port does not reproduce.
- **The org-voice blurb sweep does not run from Rust — and coexistence
  already silenced it in TS.** `maybeRewriteBlurbs` is not a scheduler job:
  its only trigger is the TS `/api/models` handler, a 10-minute-throttled
  detached kick. The moment `TALARIA_RUST_API_URL` is set, the proxy answers
  `/api/models` from Rust and that handler never runs — so the sweep is dark
  in every proxied environment (including dev-with-env), not just after
  cutover. The Rust route reads `model_blurbs` rows but never writes them;
  the sweep ports as a real registered job with the scheduler (batch 4).
  Until then the degradation is cosmetic and self-healing (the rows are read
  the moment anyone writes them), and unproxied TS installs keep firing it.
- **The blurb clamp cuts at a char boundary, not a surrogate boundary.** TS
  `slice(0, 157)` counts UTF-16 units and can split a surrogate pair in half
  (a lone surrogate in the response); a Rust string cannot hold half a pair,
  so the clamp stops at the last whole character that fits. Reachable only
  with a >160-unit first sentence whose 157th unit falls inside an astral
  character. Cosmetic. (`api/src/model_info.rs`)
- Nothing else yet.

## Layout of the crate

`api/src/` — `main.rs` (axum serve, graceful shutdown), `config.rs` (env, the
secret-root precedence `TALARIA_SECRET_KEY` → `_FILE` → `AUTH_SECRET`),
`error.rs` (the two envelopes, byte-tested), `db.rs` (pool; the schema-ownership
comment), `state.rs`, `auth.rs`, `ratelimit.rs`, `secretbox.rs`;
`routes/` (one file per prefix, mirroring `ui/src/routes/api` shapes);
`gateway/` (the LLM gateway's internals: registry, vault, provider, params,
usage, budget, guard). `Cargo.lock` is committed, as `bun.lock` is.
