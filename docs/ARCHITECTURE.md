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
search URL is rendered to the agents as `SEARXNG_URL`, unless the operator allowed them. Rendered is
not reachable, so Admin → Search probes the URL twice: from the app, and from inside a running
agent against the URL as the agents dial it (`talaria-agent-web-search`'s `probe`), shown as the
"from agents" dot. Unset, nothing is rendered. A search-only backend (Brave, SearXNG) left to read pages
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

- **A department is one agent's key.** It names the agent's compose service (`agent-<department>`),
  its Hermes data volume (`<project>_hermes-<department>`), its roll lock and its model id
  (`<slug>-<department>`), so two agents in one department would be one container and one Hermes
  home. Every writer (hire, clone, personal assistant, federation) goes through
  `talaria_agent_defs::upsert_agent_def`, which refuses a department another agent holds — retired
  or not, since a retired agent keeps its volume — under a per-department advisory lock, and the
  doors answer 409. There is no unique index: an index build fails outright on an install that
  already holds such a pair, and roll-safe migrations only add ([UPDATES.md](./UPDATES.md)). A pair
  that predates the rule is never merged or renamed: the render runs the older agent in the
  department's container (`department_runners`) and leaves the other out of the compose file and
  the manifest with a warning. No door acts on the runner's container for the left-out agent
  (`talaria_agent_defs::runner_instead_of`): its lifecycle acts are refused (retire and delete
  excepted, and delete leaves the shared container and volume alone), a config or MCP save lands
  as a version without the roll, a personal assistant's edit skips the restart and its re-enable is
  refused, and its memory and media doors answer 409. The reconcile and the fleet roll act on each
  department's container once, as the runner's. The roster, the agent's Summary tab and the alerts
  name both. The render used to keep the slug that sorted last, so an install upgraded with a pair
  in place can find the other agent still in the container: a deploy's fleet roll
  (`roll_running_agents`) recreates it as the runner's, and on a checkout a **Roll** of the runner
  does.
- Every model spec in a rendered config routes through the app's LLM gateway, so agents have
  exactly one upstream — guarded, metered, attributed. Each agent reaches it on **its own**
  gateway key (`agent:<slug>`, stamped as `LLM_API_KEY_<SLUG>` and interpolated into that
  service alone as `LLM_API_KEY`): the relay budgets it as the agent (`llm_budgets.perAgent`
  / `agents{<model>}`, on the same `agent_model` window its chat turns land in) and refuses
  any model outside the set its rendered config routes to (`gateway_agent_models`, written
  by the render). No container holds a fleet-wide gateway secret. A budget refusal names
  when the rolling window falls back under the cap (`error.resets_at`, epoch seconds, and
  the same minute in its message), and an agent's key gets no `Retry-After` with it:
  Hermes honours that header with waits of up to 600 s, so it would hold the turn for it,
  where without it Hermes retries for a few seconds, ends the turn and names the reset
  from `resets_at`. Other keys keep the header. Hermes trusts that time for its fallback
  as well. An agent with a fallback model switches to it on the refusal. The fallback calls
  the same gateway on the same key, so the turn still ends. The conversation's cached
  Hermes agent (v0.22.4 keeps one per session and evicts it after about an hour idle) then
  stays on the fallback until `resets_at`, even when an admin raises the cap sooner. A new
  conversation, that hour idle, or a restart of the agent starts on the main model again.
  The credential Hermes marks exhausted in `auth.json` does not stop a fresh agent. A
  capped `resets_at` would shorten that, but Hermes' copy would then name a wrong reset
  again, so the true time is sent.
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
- **The browser is the image's own, and it comes with the terminal.** The Hermes image
  ships Chromium (`/opt/hermes/tools`), and its package manager's lock pins the
  `agent-browser` CLI that drives it, but the image does not stage that CLI, and Hermes
  offers no browser tool until the CLI is already there. Which agents get it is the
  terminal's call. Hermes's default browser is `browser_exec`, which it offers only alongside
  the `terminal` toolset (and its Browser Use mode withholds the built-in `browser_navigate`
  tools). Its built-in tools would work without a terminal, but v0.22.4 lets a local browser
  open private and internal addresses: it skips the check on the grounds that the terminal
  reaches those networks anyway, and it judges that by the terminal backend, not by whether
  the session has a terminal. So Talaria gives the browser only to an agent with both
  `browser` and `terminal` on, such as a Developer Agent, whose shell already has that reach.
  A terminal-less agent with `browser` ticked gets no engine and no browser tool, and its
  rendered config is what it was before the engine existed; the toolset grid says why. So
  before an agent with both on starts (hire, start, roll, reconcile), the start runs
  `hermes pm install agent-browser` in a one-shot `compose run --rm --no-deps` of the agent's
  own service, as the image's `hermes` user, onto its state volume, where Hermes's own lazy
  install would put it (talaria-fleet-docker's `browser_engine.rs`; the render lists those
  departments in `fleet/browser-engine.json`, `fleet_browser_engine`). It never execs into a
  running agent, runs before a roll's drain, and installs no browser. An installed CLI
  answers at once; a failed install is logged and the agent starts without browser tools. A
  start waits at most 90 s for it (a host whose egress drops packets would otherwise hold it
  for minutes in pm's own retries); a one-shot past that is removed, and the timeout makes
  every start skip the install for the next 5 minutes. The render writes no `browser:` block
  of its own: Hermes's default stands, and a def that authored its own backend keeps it.
  With the terminal closed, a backend Hermes would run as a local browser (the built-in
  tools on this Chromium, or Camofox) has the `browser` toolset closed by the render
  instead, so an engine left on the volume runs nothing; a CDP endpoint or a hosted provider
  keeps Hermes's check and is left alone (docs/AGENT-NETWORKING.md, "What an agent's browser
  can reach"). A Developer Agent's
  `PLAYWRIGHT_BROWSERS_PATH` (where a repo's own e2e suite installs its browsers, on the
  agent's state volume) changes neither: Hermes hands `agent-browser` the image's Chromium by
  path itself.
- Each agent presents its own `tak_` key; the renderer rewrites `fleet/.env` from the DB
  every render (0600 in a 0700 dir).
- **Rolls recreate in place** (`talaria-fleet-reconcile`): re-render, pull the agent's image
  if the host lacks it (`compose pull --policy missing`; a failed pull leaves the running
  container alone), then `compose up -d --force-recreate --no-deps agent-<dept>` and wait up
  to 180s for `docker inspect` health. That wait is what a caller waits on, so it stays bounded;
  a container still running when it passes (its healthcheck `starting`, or `unhealthy` because
  the boot outlasted the start period and retries) is not called failed: its roll-registry row
  stays `rolling`, noted `still starting`, and a task off every caller's path watches it up to
  twelve more minutes, then writes `done` (healthy) or `failed` with the reason — never over a
  newer roll's row. Either wait ends early on a container that is not running: at once, and
  `idle` rather than `failed`, when a stop or retire was asked of the agent since its roll
  started (`stop_mark`, `talaria-fleet-docker`); `failed` once it has read not running for 30s
  in a row with no stop asked (it exited, or keeps crashing), so a restart's or a crash's
  few seconds down do not end it. A `still starting` row's roll has released its locks: the
  roll picture's `rolling` flag leaves it out, and the agent can be rolled again. If the api
  restarts mid-wait, its row drops off the picture once the late wait could no longer be
  running. An apply that rolled such a container has applied; the control route's roll,
  the fleet roll batch and the MCP queue audit `roll_failed` only if that late wait fails. The old container's gateway is drained first, through
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
  agent back on `-b`), and only then is `-b` drained and removed. The move holds the
  department's move lock (`lock_move`, `talaria-fleet-docker`) from before it reads `-b` until
  the home container is up, and every control verb (`compose_verb`, `fleet_up`) takes it before
  it resolves the agent's service: a verb sent mid-move would otherwise address the home
  service while `-b` still served — a stop that stopped nothing, after which the move started
  the home container. A stop or a retire leaves a stop intent before it waits; the move reads
  it before the retire, and one that arrived puts the def back on `-b` and leaves the agent
  there, for the waiting stop to stop. The move lock and the stop intents (like the per-service
  verb locks) live in one api process: a stop sent through a second api instance against the
  same fleet is not seen by a move or a health wait running in the first. A stop, restart or recreate
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
  per service at a time in the process, so nothing in flight is taken). The removal is
  confirmed before compose runs: a `docker rm` that ran out its time goes on in the daemon,
  and a `compose up` beside the half-removed container removed the agent's own container and
  created nothing, with exit 0 — so it is waited out, an `up` or `restart` does not run beside
  one that will not go, and an `up` that reports success with no container is a failure. **A
  verb outlives its caller.** The control route runs each verb but `roll` whole on a task of
  its own (`run_detached` in `talaria-fleet-docker`) — the render ahead of an `up`, the compose
  verb, which takes the service's lock inside that task, and the re-read after a failure — and
  so do the hire's `up`, a personal assistant's bring-up, and the applies of the def and MCP
  edits and of an assistant owner's edit. A client that goes away (a curl that gave up, a closed tab, or the app
  proxy's own fetch, which gives up on a response with no headers after 300 s) drops only the
  wait: the compose CLI used to die with the request's future, halfway through a recreate. The
  route answers with the verb's own result when it settles within 210 s
  (`ANSWER_WITHIN`), and past that with 202 `{ok, pending, state}` — the container as it is so
  far; the roster shows the rest. A failed `up` that
  was replacing a running container waits out a stop it may have started, then answers by what
  is there: the old container still serving (the change did not land), a new one running (it
  did), or nothing — and then it starts the service on the current render. The control route
  and the hire answer a failed verb the same way: compose's error is logged, the container is
  re-read (`settle`, `up_landed`), and an agent that reached what was asked answers as the
  success it is. One that did not is answered by why compose failed: docker's own error (a
  port already bound) with the container in the wrong state is the route's 500; a CLI killed
  at its deadline, or a re-read docker will not answer, is a 503 "could not confirm", because
  the daemon may still be carrying the request out (a stop landed two minutes after its
  answer, well past the settle window). A retire whose `rm -sf` ran out its time after the
  stop issues the remove for the stopped container it left (an unforced `docker rm`, waited
  out when it is slow).
  `TALARIA_FLEET_COMPOSE_TIMEOUT_SECS` replaces every agent compose verb's deadline — longer
  for a slow disk, or tiny to drive these paths on purpose.
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
  agent stays stopped and reads the render when it starts. **An apply never starts an agent**:
  a recreate starts the container it makes, so only a container *seen* running is rolled. A
  fit docker does not answer within 10 s is asked again with 30 s of patience
  (`ContainerFit::Unknown`), and one it still will not describe is left alone, with a sentence
  for the caller that the change is rendered but not applied. The roll then reads the
  container once more under the service's lock (`fleet_recreate_running`), so a person who
  stops the agent while the roll renders and pulls is not overruled. **No roll starts a
  stopped agent either** (`roll_running_agent`): a roll asked for as a roll — the operator's,
  the update job's, an MCP timeout or auth-mode edit's, the Developer switch's, a key
  rotation's — recreates a running agent whatever its fit, and leaves one that is not running
  when the recreate holds the service's lock as it is; its next start reads the render
  (compose recreates a container whose service moved, and a plain start boots Hermes on the
  rewritten `config.yaml`). The control route refuses `roll` for an agent that is not running
  (409: start it instead). The one exception is the fleet roll a person asks to include stopped
  agents in (`POST /api/fleet/roll` with `all`). Most of `config.yaml` is read only
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
  team or user grant, an app's new catalog, a connected account — is counted for each server
  it touches (`bump_tool_lists`, one `mcp.tool_list_count:<server>` setting per server, so a
  change to one server re-lists that server alone, for the agents that hold it); the gateway
  remembers, per agent and server, the count it last saw, and answers the agent's next
  keepalive `ping` (every 180s
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
  the old session), the operator's own roll and restart, and the update job's fleet roll — each
  a running agent only; a stopped one takes the change when it next starts.

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
  (every managed enabled one with `all`, which is the one roll that starts a stopped agent) is
  queued through the same in-place roll, `TALARIA_FLEET_ROLL_CONCURRENCY` (default 2)
  departments at a time so twenty agents never double the host's load at once; departments
  already mid-roll are skipped and named. The request answers at once; the live picture
  (`GET /api/fleet/roll`: rolling / idle / failed / done per agent, with each failure's
  sentence) comes from the roll registry each roll writes at start and verdict — in
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
  A schedule made from a thread reports into it by two roads, both ending in
  [`talaria-cron-output`](../api/crates/talaria-cron-output/src/lib.rs)'s `land` (clamp, scrub,
  reply guard, responsible-user stamp): the talaria-events plugin's push to
  `/api/agents/cron-output` at the turn's end, which is fire-and-forget, and a five-minute pull of
  the agent's own `GET /api/sessions?source=cron` that lands a settled run whose report never
  arrived. The Hermes session id is stamped on the row and is what the two roads dedupe on, under
  one advisory lock per thread and session. The pull lands only a run Hermes booked
  `cron_complete`, and only its last assistant row when that row asks for no tools, so a run that
  died mid-loop never lands its narration as a report. The listing does not name the job, so the
  pull matches a session to a job by its title (Hermes titles a run with the job's name) against
  every job the agent has, listened or not, and checks the run was sent that job's prompt; when two
  of the agent's jobs share a name, the prompt decides. A run that answered Hermes's silence
  sentinel (`[SILENT]`, matched with Hermes's own `is_autonomous_silence_response` rule, ported in
  `talaria-cron-output`'s `silence`) lands nothing by either road, as Hermes delivers nothing for
  it: the run is recorded in `cron_silent_runs` as delivered silent under the same run key, and
  the pull reads that record as delivered. Each road judges the text it received, and the push
  carries only the plugin's first 20,000 characters, so a longer run whose only marker is its own
  last line lands by the push though Hermes suppresses it (the prompt asks for `[SILENT]` alone).
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
    text: no tool calls or reasoning, and `MEDIA:` tags unresolved. A run parked on an approval
    has its open prompt (the projection's `pending_controls`) raised once as the
    `approval.request` frame its live stream carried, through the same approval tap, so it is
    refused (or carded) within a poll rather than waiting out the agent's approval timeout. The
    projection does not say when a prompt was asked, so the frame carries the earliest moment it
    can have been (the lost stream's last bytes, or the poll before), and a card raised from it
    counts its 115 s from then.

  The **completions arm** (every other chat, channel replies, harness and app turns) names no
  session, but Hermes files each header-less turn under one it derives from the turn's opening
  (`api-` plus a digest of the system rows and the first user message), so it wedges the same
  way after a non-graceful restart: every later turn with that opening is refused with
  `unknown_execution` and queued. A refused turn cannot be withdrawn on this arm (Hermes drops
  its run id before it writes the refusal), so a discard would run every queued refusal,
  unobserved. The repair (`fleet_chat::Thread`) sends the refused turn
  again, once, with one more system row in its opening (`SESSION THREAD: n`), which lands it on a
  fresh derived session and leaves the old one parked with nothing in it ever run. A chat keeps
  its thread on `conversations.hermes_epoch`, so its later turns open on the fresh session; a
  one-shot takes a random thread. The log names the abandoned `api-…` session at warn: do not
  `hermes sessions discard` it.

  A completions turn Talaria lets go of before it ends (a harness run past its deadline, an
  app turn whose caller hung up) is **stopped in the agent** (`fleet_abandoned.rs`). Hermes keeps
  an admitted turn alive past its request, so closing the stream alone left a queued turn to
  run later for nobody, or parked for good behind a lost one, and a running turn going until
  Hermes next failed a write. While the request is open, the completion id every chunk names
  (`chatcmpl-…`) is also the turn's run id, so the door posts `/v1/runs/{id}/stop` (a queued
  turn is cancelled, a started one interrupted) and holds the request open until Hermes has
  ended the turn. It reads the dropped stream on for that: to name a turn whose reader never read
  a frame, to follow a move to a fresh session, and, after the stop lands, until the stream ends
  (each for at most the 10 s control window). While Hermes has not yet bound the id (it writes the
  role chunk before the turn's admission commits, and on a starved agent store that commit has
  taken minutes) it sends the stop again, backing off, for up to the five minutes Talaria gives an
  agent's headers. A turn let go of is never moved. A turn let go of before the agent's response
  headers came has nothing naming it, and stays Hermes' to handle. The
  inbox brief, which runs detached with nobody waiting, gets the time a reply does (90 s) for
  a slow agent, and a brief past it is stopped this way instead of parked.

  Pinned sessions stay a per-agent switch, off by default. One thing blocks making them the
  default: on a session, a per-message tier is not honoured, and the session runs its main
  model (the relay books what ran). The chat surfaces say so: for an agent with pinned
  sessions the tier chip stays on the rail, off, with that reason. Conversations from before
  the switch stay on replay. Hermes' own `todo_list` stays closed for every agent until the
  session arm is the chat default: its list lives in the session transcript, and a
  completions-arm turn carries none, so it would reset every turn (`MANDATORY_DISABLED` in
  talaria-agent-defs, and talaria-agent-todo's header for the migration plan). The first turn after a crash can wait up to five minutes before it runs: the
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
audit log. The door itself never answers `once`, not even for a Developer Agent. What a
Developer Agent still asks under `mode: off` comes from the gates Hermes never lets anything
auto-approve: an MCP server's confirmation request (`elicitation/create`, from any server), an
untrusted MCP server's write tool or spend limit, a saved card fill, a write to a project's
AGENTS.md / CLAUDE.md / SOUL.md. Those want a person, and v0.22 strips the `pattern_key` that
would tell them apart from a command. Talaria renders no MCP `trust` key (Hermes reads a missing
one as `full`), no `tool_costs` and no browser vault, so on a Talaria agent today only the
confirmation request and the protected write ever ask.

The protected write never reaches a person. The render pins every agent's file-tool write root,
`HERMES_WRITE_SAFE_ROOT=/opt/data` (the agent's data volume), and its `HERMES_HOME` to the same
folder (`pin_write_root` in `talaria-fleet-render`; the image sets both values too), and Hermes'
protected-instruction gate exempts the whole `HERMES_HOME`. So the gate only ever asks about a
path outside `/opt/data`, and the write root refuses that path whatever the answer. A Developer
Agent's own checkouts live under `/opt/data/workbench`, inside the root, where the gate does not
ask. The frame names no path, so
the door recognises the gate by Hermes' own words (`fleet_approvals::unanswerable`), refuses it
at once without offering it to a card, records `reason: outside_write_root` on the
`agent.approval_denied` row, and then steers the run with the reason, because Hermes' own BLOCKED
text says the user denied it. The steer can only follow the refusal (Hermes' steer door refuses a
run that is waiting on an approval); one that misses the agent's next model call reaches it as a
follow-up turn in the same run.

Nor does an MCP server's form that asks for fields. A form-mode `elicitation/create` whose
`requestedSchema` names a field (a reason, an amount) comes through the same approval plane as a
confirmation, and Hermes v0.22.4 answers an allowed one `accept` with empty `content`, so the
server would act on an empty form. A card can only allow or deny. Hermes describes such a request
as "Fields requested by MCP server '…':" (a confirmation reads "Approval requested by MCP
server '…'."), so the door declines it the same way, before any card: the server hears
`decline`, the audit row says `reason: fields_requested`, and the run is told why. A
confirmation that names no field is still a card.

So a person can answer one, on exactly one kind of turn (`talaria-approval-cards`). On an
*attended* conversation turn (sent from `/api/chat`, or chained or resumed from one:
`TurnAttribution::attended`), the door first offers the request to a card. A card is raised only
for a **Developer Agent**, and only when the conversation's owner manages that agent (a named
manager, its personal owner, or an admin). It is a `chat_approvals` row of kind
`hermes_approval` and an `approval` chip on the turn's message. Only the owner sees **Allow
once** / **Deny**; anyone else reading the thread sees it read-only. The answer is `once` or
`deny`, never `session` or `always`, which would write the render-owned `config.yaml`.
`POST /api/chat/chips/approvals/{id}` checks the rule again and sends exactly
`{request_id, execution_generation, choice}`. A 409 from Hermes (the agent stopped waiting) marks
the card expired. A Developer Agent renders `approvals.timeout: 120`. The card closes itself at
115 s, answers `deny` and reads expired, and a boot sweep closes the cards a restart left open.
A person's answer is audited as `agent.approval_allowed` / `agent.approval_denied` with that
person as the actor; an expired card is `agent.approval_denied` by `talaria`. An answer that
reaches an agent no longer waiting took nothing, and is `agent.approval_expired` by the person,
never an allow. An allowed step is past the ask, not past Hermes' other limits. An open card
holds only its own conversation: Hermes queues that session's next turn behind the request, and
the agent's other conversations go on (measured on v0.22.4 on both arms: a turn in another
conversation answered in under 10 s while a card waited). Every other request (an ordinary
agent's, or one on a channel, ticket, research, app or harness turn) is refused at once, as
above. Cards stay out of the approvals census: a card lives two
minutes, and the census's announce and escalation sweeps work in hours. A request nobody answers
is refused by Hermes after `approvals.timeout` (60 s for an ordinary agent).

The same render pins Nous-hosted connectors (`tools.connectors.enabled`) and the Nous guest
identity (`nous.guest`) off: SaaS goes through the MCP gateway, and models through the LLM
gateway.

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
sweeps, run reclaim (re-enters a run whose driver died), price refresh, update checks, the
cron-output pull (lands a thread schedule's report the push lost).
Research/plan runs are durable on the same plane: derived run ids + per-step leases, so two
instances cannot double-run and a restart resumes at the last persisted turn.
