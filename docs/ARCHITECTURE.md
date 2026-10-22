# Architecture

How Talaria works, in one place: the processes, the request lifecycle, the auth stack, the
data layer, realtime, the agent fleet, and the app platform. Each section names the source
files that hold the truth — when this page and the source disagree, the source wins.

## The two planes

Talaria is two server processes behind one origin, plus the fleet they render:

```
  browser ──► Talaria app (ui/, one process)
              • the SPA on the origin (Vite + Svelte 5), and the app host
                    │ /api/* on loopback
                    ▼
              Rust api (api/, :5274)
              • every /api/* route: Postgres + Redis, the LLM gateway,
                the MCP gateway, the fleet renderer/orchestrator, the scheduler
                    │
     ┌──────────────┼───────────────────────────┐
     ▼              ▼                           ▼
  fleet of Hermes  talaria-mcp (mcp/,            providers you register
  agent containers a stateless proxy on :5280)   (cloud + self-hosted)
  (rendered from   — the agent-facing MCP
  one chassis)      server; no DB of its own
```

**The app (`ui/`)** serves the client from one origin and forwards `/api/*` to the Rust
api. In dev, Vite's middleware loads the real server handler (`src/server/app.ts`)
in-process with HMR (`vite.config.ts`). In prod, `server-entry.ts` wraps the SSR-bundled
handler in a Node server, serves the built client, spawns or adopts the api, and owns
graceful shutdown.

**The MCP server (`mcp/`)** is the agent-facing protocol plane: a pure proxy on `:5280`
whose every tool call becomes an HTTP call to the app with the agent's own credential. It
holds no database and no identity — it authenticates connecting agents by asking the app
(`GET /api/users` is the fleet-wide auth oracle; `agent-auth.ts` and `mcp/README.md` carry
the warning). Two transports: stdio for one agent, streamable-HTTP for the whole fleet.

**The Rust api (`api/`)** is the backend of record — every `/api/*` route except the
four permanent TS residents serves from it: the app's handler forwards `/api` to it on
loopback (`server/rust-proxy.ts` on `TALARIA_RUST_API_URL` — assumed, not opted into:
unset hops to the loopback default `http://127.0.0.1:5274`, so a fresh checkout proxies
the moment an api is listening; the literal `off` stands the hop down for tests and the
one deliberately-unproxied posture). Production server-entry spawns or adopts the api on
exactly that port; dev runs it as a
sidecar by default (`talaria dev`; `TALARIA_API=off` opts out). The four residents are
permanent (`admin/update`, app dispatch, the app-MCP gateway, and `healthz`); the rules,
the hop, and the recorded divergences are
[`docs/RUST-MIGRATION.md`](./RUST-MIGRATION.md) — the port's own story is
[`docs/history/rust-port.md`](./history/rust-port.md).

| Surface | Port | Notes |
| :--- | :--- | :--- |
| The app (dev and container) | 5273 | `PORT` in bare prod |
| The Rust api (the backend; axum) | 5274 | host loopback; container mode also listens on the fleet network (not published) so agent LLM and toolkit calls skip the UI |
| talaria-mcp | 5280 | spawned by the app as a child process |
| Postgres / Redis (dev) | 5544 / 6399 | dev infra, `--restart unless-stopped` |
| Qdrant / TEI / storage (dev) | 6333 / 8055 / 9010 | retrieval, embeddings, the built-in bucket |
| Each agent's persona gateway | 8770 up | stable per-agent port, persisted in `agent_defs.gateway_port` |

Dev infra is **five services**, not two (`docker/dev-compose.yml`): Postgres, Redis, Qdrant,
TEI embeddings, object storage. Web search is not a sidecar: fleet agents search with Hermes's
own providers, and Talaria's own pipelines use a registered provider or an operator-pointed
SearXNG-compatible endpoint (Admin → Search / `SEARXNG_URL`).

## Request lifecycle

1. **Entry.** Node request → `Request` → `server/app.ts`'s fetch handler (prod), or the same
   via `ssrLoadModule` (dev). The handler's first act is the proxy switch: a request under
   `/api` forwards to the Rust api on loopback (`rust-proxy.ts`, streamed through, no
   fallback by design), and everything below this line describes only what stays in the
   app — the SPA shell and the four permanent residents.
2. **Route table.** `app.ts` globs `routes/api/**/*.ts`, requires a `Route` export, sorts by
   static-segment count (`/api/boards/mine` beats `/api/boards/$id`). 405 with an `allow`
   header on the wrong method; unknown paths fall through to the SPA shell.
3. **Routing is string-based.** A route is the `defineApi('…')` literal in the file —
   `$param`, trailing `$` for splats. The file *name* is convention only, not mechanics.
4. **Guards** (`api-guard.ts`): `requireUser`, `requireAdmin`, `requireView`, `requirePerm` —
   each returns the user or a ready `Response`; the house idiom is
   `if (gate instanceof Response) return gate`.
5. **Body.** `parseBody(request, zodSchema)` — validated data or a 400 carrying the first
   zod issue.
6. **Resource ACL inline after the guard** — board membership, KB editors, ownership.
7. **DB + respond.** `json(data, init)` is the only HTTP helper.

**The envelope** (`docs/API-CONVENTIONS.md` has the detail): errors are
`{ error: string }` with machine strings for switchable cases; reads return a named wrapper
(`{ doc }`, never a bare array); mutations return `{ ok: true }` or the created object. The
LLM wire (`/api/llm/v1/*`) is the one exception — it speaks the OpenAI error shape.

**CSRF, honestly:** there is no CSRF middleware. The cross-site controls are the session
cookie's `SameSite=Lax` plus the constant-time-compared OAuth state cookie. If you add a
state-changing GET you have broken the model, not found a shortcut.

**Streaming, the richest lifecycle:** `/api/chat` gates the user, the conversation, the
agent, and the tier — then tees the agent's SSE stream: one branch to the client, one
detached branch persisted server-side, so a reply survives a reload mid-stream (`chat.ts`).

## Auth stack

| Caller | Credential | Resolution |
| :--- | :--- | :--- |
| Browser | opaque `sid` cookie → Redis `sess:<sid>` | sessions patchable in place; a role change lands without re-login |
| Google OAuth | redirect `…/api/auth/google/callback` | tokens hand-rolled (no SDK); hosted-domain and Workspace gates; the first identity through a claimable instance claims it |
| Password | email + scrypt hash in `user_password_credentials` | accounts created/removed by admins (Admin → People); timing-equalized misses |
| Agent | `tak_…` per-agent key (`x-api-key`) | `agent-auth.ts` `resolve()` — the key *proves* identity; `x-agent-name` can narrow, never grant. `agentCaller` (requires the name), `fleetCaller` (URL-subject plane), `requireAgent` |
| Legacy shared key | org-wide `TALARIA_AGENT_KEY` | resolves `legacy: true` — identified but untrusted; anything granting privilege refuses it. Close with `TALARIA_AGENT_KEY_LEGACY=off` |
| LLM gateway | `tlk_…` per-user bearer key | sha256-stored, mint gated by the `models.mint-keys` permission |

There are no users in a fresh install: the first visit offers `/claim`, and the identity
created there (password or Google) becomes the admin — the race is serialized on an advisory
lock, first claim wins. Roles are granted and revoked in Admin → People; a sign-in never
changes a role, and the last admin cannot be demoted. Fine-grained permissions
are a 13-entry catalog resolved user-override → org default → shipped default
(`permissions.ts`, `docs/PERMISSIONS.md`); admins hold everything unconditionally.

## Data

- **Postgres — no ORM.** The api issues sqlx runtime queries; the TS residents' slice uses
  `postgres.js` tagged-template `sql` fragments.
- **Schema migrations, append-only by contract.** One channel: the TS `MIGRATIONS` array in
  `db/pg.ts` owns every schema change AND every one-time data operation — schema changes
  and one-shot repairs alike ship as appended statements and apply themselves at the next
  boot (eagerly in server-entry, before the api spawns; lazily on first query as backstop),
  once per statement ever, under advisory lock `8_314_207`, each statement committing with
  its own sha256 bookkeeping row. An entry's index is its identity; inserting mid-array or
  editing an applied statement trips a checksum and the boot refuses. The Rust api owns
  zero DDL by law (`api/src/db.rs`). CI replays the whole array against a scratch
  postgres:16 and diffs the result against the committed schema snapshot
  (`migrations.yml`; `ui/src/server/db/schema.snapshot.sql`), and the invariant check
  demands a `-- deliberate:` marker on any statement that can destroy or rewrite data.
  Running psql by hand on a customer database is never part of a release. Bring the
  containers up before the app.
- **Wire numerics arrive as strings** (`numeric`, `int8`) — read them through `PgNumeric` /
  `pgNum`, don't `Number()` blind.
- **Redis** holds sessions, the realtime bus, scheduler leases, KB presence.
- **Secrets are envelope-encrypted**: a KEK derived from `TALARIA_SECRET_KEY` wraps a data
  key in `secret_keys` (all versions kept); ciphertext is versioned
  (`secretbox.ts`, `docs/ENCRYPTION.md`). A config file never holds a live credential.
- Load-bearing tables: `agent_defs` (the fleet registry of record), `agent_versions`
  (immutable, append-only), `internal_versions` (uniform snapshots for skills, memory,
  docs, artifacts), `app_data` (the whole app-platform store), `usage_events` (the priced
  ledger), `audit_log`.

## Realtime

Redis pub/sub → SSE. No websockets anywhere (`realtime.ts` in TS, `api/src/realtime.rs` in
the Rust api — the same bus and the same rule on both sides). One dedicated Redis
subscriber per connected client; 25-second pings; four topics: `board:<id>`,
`channel:<id>`, `run:<id>`, and `user:<id>` — the per-person firehose that makes runs
multi-device.

**The load-bearing rule: an event says *what changed*, never *what it says*** — the client
re-fetches through the ordinary ACL'd route, so realtime can never widen a read. Payloads
are serialized field-by-field so a wider object can't leak.

Chat streaming is *not* on the bus: `/api/chat` tees the agent's own SSE stream (above).

## The fleet

Agents are **rendered, not hand-written**. The fleet render (api/src/fleet/render.rs) materializes gitignored
`fleet/` from one Talaria-owned chassis: per-agent `config.yaml` + `SOUL.md`, a
`docker-compose.yml` under the `talaria-fleet` project, and `fleet.json` — the manifest the
app reads to dial each agent's persona gateway directly on its published port. No bridge, no
multiplexer.

- Every model spec in a rendered config routes through the app's LLM gateway, so agents have
  exactly one upstream — guarded, metered, attributed. Each agent reaches it on **its own**
  gateway key (`agent:<slug>`, stamped as `LLM_API_KEY_<SLUG>` and interpolated into that
  service alone as `LLM_API_KEY`): the relay budgets it as the agent (`llm_budgets.perAgent`
  / `agents{<model>}`, on the same `agent_model` window its chat turns land in) and refuses
  any model outside the set its rendered config routes to (`gateway_agent_models`, written
  by the render). No container holds a fleet-wide gateway secret.
- Each agent presents its own `tak_` key; the renderer rewrites `fleet/.env` from the DB
  every render (0600 in a 0700 dir).
- **Rolls recreate in place** (`talaria-fleet-reconcile`): re-render, pull the agent's image
  if the host lacks it (`compose pull --policy missing`; a failed pull leaves the running
  container alone), then `compose up -d --force-recreate --no-deps agent-<dept>` and wait up
  to 180s for `docker inspect` health. The old container's gateway is drained first, through
  its own supervisor (`s6-svc -d` then `s6-svwait -D` inside the container): a plain `docker
  stop` never drains Hermes, because s6-overlay revives the SIGTERMed gateway and KILLs
  everything 3s later. Down through s6, it holds cron and `/v1` runs up to its 30s drain floor
  and exits; every stop, restart and remove Talaria does goes the same way. The render also
  gives every service `stop_grace_period: 45s` (the chassis may override it) as the outer
  bound. There is no second container: every Hermes since
  v2026.4.30 holds an `flock` on `/opt/data/gateway.lock` in the state volume, so a second
  container on the same volume exits on start and never serves — which is why the blue/green
  roll this replaced never completed for a running agent. The agent is offline for the
  restart; `proxyChat` re-reads the manifest per attempt and holds a turn up to two minutes
  across it. An agent an old blue/green roll left in the `agent-<dept>-b` service
  (`agent_defs.active_slot = 'b'`) moves back to `agent-<dept>` on its next roll.
- **Config edits reach running agents without a roll.** A render rewrites each agent's
  `config.yaml` *in place* (same inode, under an `flock`, unchanged bytes skipped), because
  the container sees it through a single-file `:ro` bind that pins whichever inode existed
  at container start — a temp-file rename never reaches it. Hermes re-reads the file when its
  `(mtime, size, inode, ctime)` signature changes, and the writer bumps the mtime last so the
  final signature is always new. A reader that races the write sees either a parse error
  (Hermes keeps serving its last good config; the rendered file ends in the YAML `...`
  document-end marker so a stale tail after a shrink cannot parse) or a mix of two renders
  for one load. SOUL.md goes through the same writer. Environment, image, limits and plugin
  changes still need a roll: rotate-key, the Developer switch and MCP timeout or URL edits
  roll as before.
- **The Hermes image is pinned** to a tag and digest (`scripts/chassis.template.yml`, mirrored
  as `HERMES_IMAGE_PIN` in `talaria-fleet-render`). A chassis that still carries the old
  floating default `${HERMES_IMAGE:-nousresearch/hermes-agent:latest}` renders the pin
  instead; any other image an operator set stays theirs, and `HERMES_IMAGE` in `fleet/.env`
  overrides both. A host pulls a pin it does not have on the next roll or start, and the
  post-deploy fleet roll is what moves running agents onto it. To bump it, change the
  template default, `HERMES_IMAGE_PIN`, and the `FROM` default in
  `scripts/workbench.Dockerfile` and `scripts/build-workbench-image.sh` to the new
  `nousresearch/hermes-agent:<tag>@sha256:<digest>` (tests fail if they disagree), then
  check the new version's config keys against the render.
- **Rolling the whole fleet** (`POST /api/fleet/roll`, admin): every running managed agent
  is queued through the same in-place roll, `TALARIA_FLEET_ROLL_CONCURRENCY` (default 2)
  departments at a time so twenty agents never double the host's load at once; departments
  already mid-roll are skipped and named. The request answers at once; the live picture
  (`GET /api/fleet/roll`: rolling / idle / failed / done per agent, with each failure's
  sentence) comes from the roll registry `roll_agent` writes at start and verdict — in
  process, mirrored into the Redis hash `talaria:fleet-roll:v1` so every api instance
  answers the same, outcomes kept an hour. A single agent's roll lands on the same page.
- **Preflight probes from where the agent stands**: a throwaway container on the fleet
  network probes the gateway/MCP base URLs (api/src/fleet/preflight.rs) — the host's view proves
  nothing.
- **Crons live in the agent**, not in Talaria: Hermes' own scheduler ticks inside the
  container; Talaria reads it for truth and mutates through `docker exec`
  (api/src/agent_crons.rs). Their spend is metered at the relay: a turn Talaria drives is
  ledgered once by the flow that drove it, so the chat transport counts each such turn in
  Redis (`talaria:turn:v1:<agent model>`) while it is open, and a call on an agent's key with
  no turn counted — a cron — is booked by the relay under the agent as `unattended`, into the
  same window its budget is judged on.

## Agent work and the guardrails

The MCP toolkit (`mcp/src/index.ts`) is the fleet's registered tools — tickets, documents, KB,
channels, Google calendar/mail, research, board membership — and no document states the count:
it changes with every registration, and `bun run check` (`toolkit-tool-count-drift`) fails any doc
that writes a number the source does not have; `mcp/README.md` lists every tool by name. The guardrail that
actually holds is server-side and narrow: **agents cannot write assignees and cannot make a
terminal status move** — the final sign-off is structurally a human's. The refusal predicate
lives once in `server/tasks.ts` (`agentSafePatch`); a second definition anywhere fails the
invariant check, and the tool descriptions must mirror it (a startup auditor shouts on
drift).

Work delivery is **push-side**: the app dispatches durable work sessions to agents
(`work-dispatch.ts`). The Python plugin in `plugin/talaria/` is vestigial — nothing mounts
it; its heartbeat reference is the legacy mission-control contract.

## The MCP gateway

Agents never see an upstream URL or credential. Every external tool call goes through
`/api/mcp/gw/<server>` (`mcp.gw.$server.ts`), which enforces, in order: agent identity →
assignment ∩ per-person allowance → **tool allowlist before the upstream hears anything** →
credential injection (org headers, OAuth token, or the per-user connected account).
`tools/list` responses are rewritten down to the allowed set. `«secret:…»` handles are spent
at this one choke point and resolved to real values only here — unresolved handles are
reported to the operator, never to the model. Non-builtin upstream URLs pass the SSRF guard
before any hop.

Two server kinds never leave the process: `talaria-workbench://` and app-published servers
(`talaria-app://<slug>`) dispatch in-process. The built-in toolkit is seeded into the same
registry and cannot be removed.

## Apps

Apps are TypeScript codebases this instance compiles — they do not freeze into
the host bundle. Four former `import.meta.glob`s are gone; loaders
`import()` the app's current build (`/app-builds/<slug>/<key>/`) or, in dev,
the source through Vite (`/@app/<slug>/`). Shared Svelte/SDK instances come
from host `runtime/rt-*.js` entries. Each app gets its own Postgres
(compose project, password sealed in `app_settings`); `ctx.store` is the
document API, never a connection string. A throw in an app is isolated: the
pane shows the crash, the host stays up. Enablement, install, and uninstall
stay Rust; the UI process owns the compiler and the database spawn. Building
them: `docs/APPS.md` + `docs/sdk/`.



## Jobs and durable runs

One scheduler, one runtime, one switch: `TALARIA_SCHEDULER` — unset arms the
Rust api's (`api/src/jobs.rs`), `off` arms nobody (the kill switch; `rust` is
the retired coexistence-era value and reads as unset). Registered jobs with
named intervals, **exactly-once via a Redis lease**
— one run per interval fleet-wide; unreachable Redis skips the tick rather than running
unguarded. The first nine jobs are required: a missing registration refuses to arm the schedule, and the arm loop says which job is missing on every retry (boot never reports the schedule armed). The api
arms in every posture that serves — `talaria dev`'s child included, so background jobs
fire locally exactly as in prod; a bare `vite dev` with no api is the one posture that
never arms them, which is why "my background job never fires locally" usually means the
api isn't running. Boot arms independently of the stores (the arm loop retries until
Postgres and Redis answer — the startup log says which), and server-entry probes the
api before reporting the app ready, so jobs run only on an instance that can serve.

The scheduled family: comms decay (distill idle threads), the daily digest and approval
escalation, the daily brief (opens per person, per timezone), notification mail, outreach
sweeps, run reclaim (re-enters a run whose driver died), price refresh, update checks.
Research/plan runs are durable on the same plane: derived run ids + per-step leases, so two
instances cannot double-run and a restart resumes at the last persisted turn.
