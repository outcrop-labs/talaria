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
SearXNG-compatible endpoint (Admin → Search / `SEARXNG_URL`). Which Hermes provider the agents
use is one org setting, **Agent web search** on Admin → Search (`talaria-agent-web-search`): the
render writes `web.search_backend` / `web.extract_backend` into every agent's config, delivers the
vendor key from its sealed row through `fleet/.env` (`TALARIA_WEB_<VAR>`, interpolated as the
variable the Hermes provider reads), and switches Hermes's keyless public tiers and their one-shot
rescue off (`web.keyless_fallback` / `web.keyless_rescue: false`) once a provider is picked or the
search URL is rendered to the agents as `SEARXNG_URL` (rendered, not probed), unless the operator
allowed them. Unset, nothing is rendered. A search-only backend (Brave, SearXNG) left to read pages
is a render warning, not a refusal. Research does not read it.

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
| Agent | `tak_…` per-agent key (`x-api-key`) | `agent-auth.ts` `resolve()` — the key *proves* identity; `x-agent-name` can narrow, never grant. `agentCaller` (requires the name), `requireAgent` |
| Legacy shared key | org-wide `TALARIA_AGENT_KEY` | resolves `legacy: true` — identified but untrusted; anything granting privilege refuses it. Close with `TALARIA_AGENT_KEY_LEGACY=off` |
| LLM gateway | `tlk_…` per-user bearer key | sha256-stored, mint gated by the `models.mint-keys` permission |
| An app, on the LLM gateway | `tlk_…` key named `app:<slug>`, held by the app host | minted by the api on the host's internal ask, sealed in `app_llm_keys`, revoked on disable and uninstall; the relay trusts it only while that record holds its id ([Apps](#apps)) |

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
- **Hermes' own controls are rendered, not re-implemented.** Each `config.yaml` arms Hermes'
  tool-loop guard (`tool_loop_guardrails.hard_stop_enabled` — `api_server` counts as attended, so
  the hard stops are off unless rendered), routes each persona tier
  (`platforms.api_server.extra.model_routes`: `<persona>-<tier>` → the tier's upstream id),
  carries each model's configured effort (`agent.reasoning_overrides`, and
  `model_overrides.<provider>.<id>.supports_reasoning: false` — the provider id the model's
  routed spec names, `custom` today — for a model nobody vouches levels for, so Hermes does not
  fill in `medium`), and turns off Hermes' model-written session titles
  (`auxiliary.title_generation.model_upgrade_enabled: false`). A turn's own effort pick travels
  as `model_options.reasoning_effort`, the one spelling Hermes' API server reads. A pinned
  session runs the agent's main model whatever tier a message picks (Hermes fixes a session's
  model at its first turn), so such a turn carries no tier effort and is booked as the main model.
- **How an agent reads an image** comes from Talaria's own capability facts (a fitness run's
  vision probe, the provider catalog, or a declaration), rendered in Hermes' keys
  (`fleet_vision` in talaria-fleet-render): each model the agent runs on whose sight Talaria
  knows gets `model_overrides.custom.<id>.supports_vision`. The describer is the org's
  `vision` role model, rendered as `auxiliary.vision` — routed and key-scoped like any other
  spec — whenever one of the agent's models is not known to see; assigning the role
  re-renders the fleet, and a new fact reaches agents at the next render. Hermes keeps one
  image mode per agent, and the render picks it from the main model: `native` (pixels) for a
  main model known to see, `text` (the describer first) for one that is not. A blind agent
  reads a chat image through the describer automatically, and a tool's image
  (`fetch_attachment` hands one back as a `MEDIA:` path) with Hermes' `vision_analyze`.
  Under a seeing main model, a turn on a tier or fallback that cannot see is sent pixels
  too and does not read an attached image; that model still has `vision_analyze`. Talaria
  serves fleet agents no image tool of its own (the Apps SDK keeps its in-process
  describer).
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
  restart; the persona door (`talaria-gateway` `fleet_chat.rs`) re-reads the manifest per
  attempt and holds a turn up to two minutes across it. An agent an old blue/green roll left in the `agent-<dept>-b` service
  (`agent_defs.active_slot = 'b'`) moves back to `agent-<dept>` on its next roll: the render
  names the home service, its image is pulled while `-b` still serves (a failed pull puts the
  agent back on `-b`), and only then is `-b` drained and removed. A stop, restart or recreate
  that fails before it reaches the container brings the drained gateway back up
  (`s6-svc -u`), so a failed step never leaves an agent running without one.
- **A compose verb's exit is not the container's state** (`talaria-fleet-docker`). A CLI
  killed at its deadline leaves the daemon finishing what it asked for — a stop lands at the
  end of its grace period whether anyone waits or not — and compose recreates by creating the
  new container as `<first 12 of the old id>_<name>`, stopping and removing the old one, then
  renaming. A recreate cut off between the create and the rename strands that container
  `created`, and the next recreate of the service then stops the running agent and fails on the
  name. So every `up` and stop-shaped verb first removes such a stranded container (this fleet's
  compose project and service by label, never started, that exact name; one compose operation
  per service at a time in the process, so nothing in flight is taken), and a failed `up` that
  was replacing a running container waits out a stop it may have started, then answers by what
  is there: the old container still serving (the change did not land), a new one running (it
  did), or nothing — and then it starts the service on the current render. The control route
  and the hire answer a failed verb the same way: compose's error is logged, the container is
  re-read (`settle`, `up_landed`), and an agent that reached what was asked answers as the
  success it is. `TALARIA_FLEET_COMPOSE_TIMEOUT_SECS` replaces every agent compose verb's
  deadline — longer for a slow disk, or tiny to drive these paths on purpose.
- **Config edits reach running agents without a roll.** A render rewrites each agent's
  `config.yaml` *in place* (same inode, under an `flock`, unchanged bytes skipped), because
  the container sees it through a single-file `:ro` bind that pins whichever inode existed
  at container start — a temp-file rename never reaches it. Hermes re-reads the file when its
  `(mtime, size, inode, ctime)` signature changes, and the writer bumps the mtime last so the
  final signature is always new. A reader that races the write sees either a parse error
  (Hermes keeps serving its last good config; the rendered file ends in the YAML `...`
  document-end marker so a stale tail after a shrink cannot parse) or a mix of two renders
  for one load. SOUL.md goes through the same writer. Because a written config is live at
  once, the render widens each agent's gateway scope to its old models plus its new ones
  before it writes any `config.yaml`, and narrows it to the new set after the last one, so a
  model change never meets a relay that refuses the id Hermes just switched to. Re-reading the
  file is not acting on every key in it, though: see **Applying a change** below for what a
  running agent takes from the rewrite and what still restarts it. Agents created before
  in-place writes still hold
  the inode of an orphaned `config.yaml` and need one roll: image installs get it from the
  post-deploy fleet roll; checkout and dev installs never roll the fleet on upgrade, so run
  **Roll all** (`POST /api/fleet/roll`) once.
- **Applying a change** (`apply_agent_change` in `talaria-fleet-reconcile`) is how every edit
  reaches a running agent: a toolset or delegation-cap switch, a def edit's apply, an MCP
  grant, assignment, team or user access, a connected account, a server or app enabled,
  disabled, removed or re-cataloged, the org profile, and a model or endpoint removed in Models
  (`talaria-fleet-cascade`: the stripped tiers are start-time config, so a running agent rolls;
  a stopped one is never started by it). It renders, then asks whether the
  running container still matches the render (`container_fit` in `talaria-fleet-docker`):
  `docker compose config --hash` for the agent's service against the
  `com.docker.compose.config-hash` label the container was created with — compose's own
  digest of image, environment (with every `${VAR}` interpolated and every `env_file` read
  in), mounts, limits and labels. Only a container that no longer matches is rolled; a stopped
  agent stays stopped and reads the render when it starts. Most of `config.yaml` is read only
  at start (`model_routes` is parsed once, in the API server adapter's constructor; plugins
  load at boot), so each service carries a `talaria.config-start-hash` label: a digest of the
  rendered config minus the paths a running Hermes v0.22.4 picks up from the rewrite —
  `agent.disabled_toolsets` (read on every API turn), `delegation.max_concurrent_children`
  (read on every delegate call) and `mcp_servers`, whose names the housekeeping tick
  reconciles (`LIVE_CONFIG_PATHS`, `talaria-fleet-render`). A change to the rest moves the
  label, so it moves compose's hash, so it rolls. SOUL.md is read when a session's prompt is
  built: an identity or soul edit reaches new sessions without a restart, and a session with
  history keeps the prompt it stored either way. A change lands on the agent's next turn,
  never the one in flight.

  Two MCP changes keep a name an agent already holds, and reach it through the gateway rather
  than a restart. A tool list that changes under the name — a subset widened or narrowed, a
  team or user grant, an app's new catalog, a connected account — is counted
  (`bump_tool_lists`, the `mcp.tool_lists_epoch` setting); the gateway remembers, per agent
  and server, the count it last saw, and answers the agent's next keepalive `ping` (every 180s
  by default) with `notifications/tools/list_changed` ahead of the result, as SSE, so Hermes
  lists the tools again (the Rust gateway, and the TS app gateway for `app-*` servers). A
  `tools/list` from an agent behind the count carries it too: Hermes stops pinging an upstream
  that answered ping with -32601 or let it time out, and keeps that connection alive with
  `tools/list` instead (the Rust relay only; the TS app gateway answers ping itself). A
  server connected after the agent first saw it not connected: the in-house not-connected
  handshake issues a `talaria-nc-` session id, and once the account is connected a request on
  that session is answered HTTP 404, which the MCP client reads as "Session terminated" and
  Hermes reconnects on. What still rolls: the Developer switch (env, image, mounts), a key
  rotation (env), an MCP server's timeout or auth-mode edit (Hermes does not reconnect a server
  whose entry changed under the same name, and a session minted under the old credentials is
  the old session), the operator's own roll and restart, and the update job's fleet roll.

  **Hermes's managed scope (`HERMES_MANAGED_DIR`) is not used, by decision (2026-10-27).** It
  deep-merges a managed `config.yaml` over the user file leaf by leaf, which cannot close a
  map: an `mcp_servers.<x>` or `providers.<x>` sibling the agent or its terminal wrote into the
  user layer — on its own state volume — would survive the overlay and reach a raw endpoint,
  the invariant the whole-file `:ro` render exists to hold. It would add no liveness (the
  managed file reloads on the same stat signature as the user one, and its `.env` is read at
  launch) and remove no roll. Revisit only if Hermes grows a closed-map managed semantic.
- **The Hermes image is pinned** to a tag and digest (`scripts/chassis.template.yml`, and the
  last entry of `HERMES_IMAGE_DEFAULTS` in `talaria-fleet-render`, which is
  `HERMES_IMAGE_PIN`). A host's `fleet/chassis.yml` is copied from the template once and never
  rewritten, so it keeps the default it was seeded with. The render therefore reads a chassis
  image of `${HERMES_IMAGE:-<x>}`, where `<x>` is any default a stock template ever shipped (the
  pre-pin `:latest` or any past pin), as never chosen, and renders the current pin. Any other
  image an operator wrote stays theirs, and `HERMES_IMAGE` in `fleet/.env` overrides both (set
  it to hold agents on an older stock pin). A host pulls a pin it does not have on the next
  roll or start. On an image install the post-deploy fleet roll moves running agents onto it;
  on a checkout or dev install, run **Roll all** after upgrading. To bump the pin:
  1. **Append** the new `nousresearch/hermes-agent:<tag>@sha256:<digest>` to
     `HERMES_IMAGE_DEFAULTS`. Never edit or drop an entry, or hosts seeded with it stay on it.
  2. Set the same string as the template default and as the `FROM` default in
     `scripts/workbench.Dockerfile` and `scripts/build-workbench-image.sh`. Tests fail if they
     disagree.
  3. Check the new version's config keys against the render, and set `HERMES_CONFIG_VERSION`
     (beside the pin) to the new image's `DEFAULT_CONFIG["_config_version"]`: every render
     stamps it as `_config_version`, so Hermes reads the config as current and never tries to
     migrate the read-only file. A test pairs the two. Re-check `LIVE_CONFIG_PATHS` against the
     new version too: a path stays there only while Hermes still re-reads it live.
  4. Check what the drain relies on in the new image: the gateway's s6 service is still
     `/run/service/gateway-default`, `s6-svc` and `s6-svwait` are still under `/command`, and a
     container drained with `s6-svc -d` and then recreated starts its gateway on its own (Hermes
     still records the gateway as running). If Hermes ever records that stop as deliberate,
     every roll and restart yields a container whose gateway never starts.
- **Rolling the whole fleet** (`POST /api/fleet/roll`, admin): every running managed agent
  is queued through the same in-place roll, `TALARIA_FLEET_ROLL_CONCURRENCY` (default 2)
  departments at a time so twenty agents never double the host's load at once; departments
  already mid-roll are skipped and named. The request answers at once; the live picture
  (`GET /api/fleet/roll`: rolling / idle / failed / done per agent, with each failure's
  sentence) comes from the roll registry `roll_agent` writes at start and verdict — in
  process, mirrored into the Redis hash `talaria:fleet-roll:v1` so every api instance
  answers the same, outcomes kept an hour. A single agent's roll lands on the same page.
- **Preflight probes from where the agent stands**: a throwaway container on the fleet
  network probes the gateway/MCP base URLs (`talaria-fleet-preflight`) — the host's view proves
  nothing.
- **Crons live in the agent**, not in Talaria: Hermes' own scheduler ticks inside the
  container; Talaria reads and changes the jobs through Hermes's cron REST on the agent's own
  API server (`/api/jobs`, the chat door and bearer —
  [`talaria-agent-crons`](../api/crates/talaria-agent-crons/src/lib.rs)). Run-now only marks a
  job due for the next tick (≤60s), and is refused on a paused job, which Hermes's `run`
  would resume for good. The admin frequency floor judges the schedule Hermes parsed — a job
  is written paused, checked, then started — and refuses any shape it cannot read (a 6-field
  cron, an unreadable field).
  Their spend is metered at the relay like every agent call — see **Agent spend** below; a
  fire's calls carry `cron_<job id>` and land as `unattended`, per job.
- **Agent spend is metered at the relay, one row per upstream call.** Every agent's
  config.yaml carries a `providers.talaria` entry on the same gateway URL its model specs use,
  with `session_affinity_header: x-talaria-session`; Hermes matches it by base URL and stamps
  the conversation's affinity key on every call it makes to the gateway — the main loop,
  compression, titles, vision, a delegated subagent (its parent's key) and a cron
  (`cron_<job id>`). On a turn Talaria drives, Hermes v0.22's key is its own digest of the
  session's route (`gwk_…`), so two records meet in the middle: the chat transport
  (`talaria-gateway` `fleet_chat.rs`) records the conversation it drove in
  `usage_attributions` with the flow's source, conversation or run, and ticket — a pinned
  session's id is the one Talaria created, the completions arm's is the one Hermes answers in
  `X-Hermes-Session-Id` — and the agent's talaria-events plugin reports, from inside the turn,
  which conversation its key names (`POST /api/agents/affinity` → `usage_affinity_keys`). The
  relay copies the flow onto every row it books for that conversation; anything unclaimed stays
  `unattended` under the agent. A turn an app's harness drives is recorded the same way, as
  `app` spend, so the agent's calls are that turn's only meter. Budgets read the same rows, so a long turn counts while it runs.
  No flow writes an agent's spend itself any more
  ([`usage.rs`](../api/crates/talaria-gateway/src/usage.rs), "Attribution"). So a fleet an
  operator pointed at a raw upstream (a non-gateway `LLM_BASE_URL` in the fleet `.env`, which
  skips the gateway brain) has no agent spend in the ledger at all: its calls never reach the
  relay, and nothing else books them.
- **Hermes sessions** (api/crates/talaria-gateway/src/fleet_chat.rs, fleet_lost_turns.rs) carry
  every work session (keyed by run id) and the conversations of an agent with **pinned
  sessions** on (keyed by conversation id). Hermes holds the transcript, and Talaria sends one
  message per turn. Three rules keep that continuous across agent restarts:
  - *A session that carried turns before is looked up, never re-created.* If it is gone (a
    lost state volume, or a deleted session, whose id Hermes never reuses), a chat turn
    replays the full history the completions way, and so does every later turn. No empty
    room is left for the next turn to fall into. A work session starts its session again
    instead, because its steps carry no history to replay.
  - *A turn lost in a non-graceful restart is repaired in place.* Hermes v0.22 marks such a
    turn `unknown` and refuses every later turn on that session (`unknown_execution`), leaving
    each refused turn queued. The session arm withdraws the refused turn
    (`/v1/runs/{id}/stop`), acknowledges the lost one with `hermes sessions discard` in the
    container (`/v1/runs/{id}/resolve-unknown` answers 404 for session turns after a restart),
    and sends the turn again. Hermes' own boundary row tells the agent the earlier turn did not
    finish.
  - *A resumed turn the agent still holds is picked up, not re-sent.* A v0.22 session turn
    keeps running when its stream drops, so the auto-resume asks `GET /v1/runs/{id}` first and
    lands the run the way its live stream would have: the reply, a stopped turn, or, for a
    `failed` run, an error row that does not chain. Only a run the container does not know
    (404) or one Hermes fenced as `unknown` is sent again. The run door keeps only the reply's
    text: no tool calls or reasoning, `MEDIA:` tags unresolved, and no `approval.request`
    frames, so a run parked on an approval waits out the agent's approval timeout.

  Pinned sessions stay a per-agent switch, off by default. One thing blocks making them the
  default: on a session, a per-message tier is not honoured, and the session runs its main
  model (the relay books what ran). Conversations from before the switch stay on replay. The first turn after a crash can wait up to five minutes before it runs: the
  restarted container's gateway gets the same pid and pid-namespace id as the dead one, so
  Hermes treats the dead turn's transcript lease as live until it expires.

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

Inside the container, Hermes gates `execute_code` and any terminal command its classifier
calls dangerous behind an approval, and Talaria sets that policy whole on every render
(`fleet_approvals` in `talaria-fleet-render`). A **Developer Agent** gets `approvals.mode: off`
and `approve`: flagged commands run, with Hermes' hardline floor and any `approvals.deny` list
still in force. **Every other agent** gets `mode: manual` and `deny`: no LLM guardian decides,
and a flagged call is refused. Crons and unattended turns resolve that on the spot. A
*streamed* turn (chat, channel reply, work session) is treated by Hermes as attended, so it
emits an `approval.request` frame and blocks. The fleet chat door taps every turn's stream and
answers each request `deny` through the agent's own `POST /v1/runs/{id}/approval`
(`talaria-gateway/src/fleet_approvals.rs`), and writes an `agent.approval_denied` row to the
audit log. It never answers `once`, not even for a Developer Agent. What a Developer Agent still
asks under `mode: off` comes from the gates Hermes never lets anything auto-approve: a write to a
project's AGENTS.md / CLAUDE.md / SOUL.md, an untrusted MCP server's write tool, a saved card
fill. Those want a person, and v0.22 strips the `pattern_key` that would tell them apart from a
command. A request nobody answers is refused by Hermes after `approvals.timeout` (60 s). The
same render pins Nous-hosted connectors (`tools.connectors.enabled`) and the Nous guest identity
(`nous.guest`) off: SaaS goes through the MCP gateway, and models through the LLM gateway.

Work delivery is **push-side**: the app dispatches durable work sessions to agents
(`talaria-work-dispatch`): on the task write that hands an agent work (the ticket enters an
agent-start column, or gains an agent assignee), on a workchain hand-off, and from a sixty-second
sweep that re-offers anything a push missed. A workchain step whose predecessor is still open is
not dispatched — the gate asks `talaria_tasks::workchain_blocked` before any session starts, and
the session asks it again on every turn. The agent cannot start it by hand either: moving it into a
working column or review, or binding a workbench job to it, is refused the same way. Agents do not
poll for work.

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

**An app's model calls go through the one gateway.** A harness an app runs — from its
server, an MCP tool, or a fitness replay of its fixtures — is executed by the host's TS
runner (`ui/src/server/harness/run.ts`), and every call that runner makes is a POST to the
Rust relay (`/api/llm/v1/chat/completions`) on the app's own gateway key, `app:<slug>`
(`harness/transport.ts`). The host asks the api for that key over the internal route
(`POST /api/internal/apps/<slug>/llm-key`) and keeps it in memory; app code never sees it.
Which app is calling is the host's answer, not the app's: the host marks every boundary where
it runs app code (`isolateApp`, an MCP handler, a replay — `ui/src/server/app-scope.ts`), and a
model call outside every such scope is refused. The relay treats the key as the app: its
budget subject is `app:<slug>` (set it in Admin → Settings → LLM budgets), every row it books
lands under the run's caller stamp inside that namespace with ledger source `app`, and the
run's ref and ticket ride along when they are uuids. A model that is a live fleet agent is
driven for the turn through the persona door with the turn recorded as `app` spend. The relay
runs no guard pass of its own for an app's key; the runner's, with the harness's own rule
set, is the one of record. Nothing in the UI process holds a provider key or reaches a model
any other way ([`app_keys.rs`](../api/crates/talaria-gateway/src/app_keys.rs),
[`llm_key.rs`](../api/crates/talaria-apps/src/llm_key.rs)).



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
