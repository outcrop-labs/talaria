# Architecture

How Talaria works, in one place: the processes, the request lifecycle, the auth stack, the
data layer, realtime, the agent fleet, and the app platform. Each section names the source
files that hold the truth — when this page and the source disagree, the source wins.

## The two planes

Talaria is two processes, not an app plus an API host:

```
  browser ──► Talaria app (ui/, one process)
              • SPA + HTTP API on the same origin (Vite + Svelte 5)
              • Postgres + Redis, the LLM gateway, the MCP gateway,
                the fleet renderer/orchestrator, the app host, the scheduler
                    │
     ┌──────────────┼───────────────────────────┐
     ▼              ▼                           ▼
  fleet of Hermes  talaria-mcp (mcp/,            providers you register
  agent containers a stateless proxy on :5280)   (cloud + self-hosted)
  (rendered from   — the agent-facing MCP
  one chassis)      server; no DB of its own
```

**The app (`ui/`)** serves the client and the API from one origin. In dev, Vite's middleware
loads the real server handler (`src/server/app.ts`) in-process with HMR (`vite.config.ts`).
In prod, `server-entry.js` wraps the SSR-bundled handler in a Node server, serves the built
client, owns the SSE pump, graceful shutdown, and boots the scheduler before `listen()`.

**The MCP server (`mcp/`)** is the agent-facing protocol plane: a pure proxy on `:5280`
whose every tool call becomes an HTTP call to the app with the agent's own credential. It
holds no database and no identity — it authenticates connecting agents by asking the app
(`GET /api/users` is the fleet-wide auth oracle; `agent-auth.ts` and `mcp/README.md` carry
the warning). Two transports: stdio for one agent, streamable-HTTP for the whole fleet.

| Surface | Port | Notes |
| :--- | :--- | :--- |
| The app (dev and container) | 5273 | `PORT` in bare prod |
| talaria-mcp | 5280 | spawned by the app as a child process |
| Postgres / Redis (dev) | 5544 / 6399 | dev infra, `--restart unless-stopped` |
| Qdrant / TEI / MinIO / SearXNG (dev) | 6333 / 8055 / 9010 / 8888 | retrieval, embeddings, the built-in bucket, search |
| Each agent's persona gateway | 8770 up | stable per-agent port, persisted in `agent_defs.gateway_port` |

Dev infra is **six services**, not two (`docker/dev-compose.yml`): Postgres, Redis, Qdrant,
TEI embeddings, MinIO, SearXNG.

## Request lifecycle

1. **Entry.** Node request → `Request` → `server/app.ts`'s fetch handler (prod), or the same
   via `ssrLoadModule` (dev).
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

- **Postgres via `postgres.js`** — no ORM. Tagged-template `sql` fragments everywhere.
- **Schema in one file, append-only by contract.** `MIGRATIONS` in `db/pg.ts` is an array of
  idempotent statements; an entry's index is its identity. Inserting mid-array trips a
  checksum and the app refuses to boot. Migrations run lazily on first query, once per
  statement ever, under an advisory lock; a failed migration is cached — every request 500s
  until restart. Bring the containers up before the app.
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

Redis pub/sub → SSE. No websockets anywhere (`realtime.ts`). One dedicated Redis subscriber
per connected client; 25-second pings; four topics: `board:<id>`, `channel:<id>`,
`run:<id>`, and `user:<id>` — the per-person firehose that makes runs multi-device.

**The load-bearing rule: an event says *what changed*, never *what it says*** — the client
re-fetches through the ordinary ACL'd route, so realtime can never widen a read. Payloads
are serialized field-by-field so a wider object can't leak.

Chat streaming is *not* on the bus: `/api/chat` tees the agent's own SSE stream (above).

## The fleet

Agents are **rendered, not hand-written**. `fleet-render.ts` materializes gitignored
`fleet/` from one Talaria-owned chassis: per-agent `config.yaml` + `SOUL.md`, a
`docker-compose.yml` under the `talaria-fleet` project, and `fleet.json` — the manifest the
app reads to dial each agent's persona gateway directly on its published port. No bridge, no
multiplexer.

- Every model spec in a rendered config routes through the app's LLM gateway, so agents have
  exactly one upstream — guarded, metered, attributed.
- Each agent presents its own `tak_` key; the renderer rewrites `fleet/.env` from the DB
  every render (0600 in a 0700 dir).
- **Blue/green rolls** (`fleet-reconcile.ts`): the incoming slot comes up on a fresh port,
  must pass `docker inspect` health within 120s (a sick newcomer is discarded; the old
  container never stopped serving), then cutover is one `UPDATE agent_defs` + re-render —
  traffic shifts on the next manifest read — a drain window (default 45s), then the old slot
  is removed. `proxyChat` re-reads the manifest per attempt and holds a turn up to two
  minutes across a restart.
- **Preflight probes from where the agent stands**: a throwaway container on the fleet
  network probes the gateway/MCP base URLs (`fleet-preflight.ts`) — the host's view proves
  nothing.
- **Crons live in the agent**, not in Talaria: Hermes' own scheduler ticks inside the
  container; Talaria reads it for truth and mutates through `docker exec`
  (`agent-crons.ts`).

## Agent work and the guardrails

The MCP toolkit (`mcp/src/index.ts`) is **51 registered tools** — tickets, documents, KB,
channels, Google calendar/mail, research, web search, board membership. The guardrail that
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

Apps compile into the deployment — four `import.meta.glob`s reach into `apps/<slug>/`
(manifests + servers, MCP modules, harnesses, client surfaces), with one deduped copy of
svelte/svelte-query/sv-router in the build. The host dispatch does the trust work before an
app sees anything: session → app enabled → view not denied → then the app's own server
handler with `{ user, app, path, url, store }` — apps never see raw cookies or each other's
data. The store is `storeFor(app)` over the shared `app_data` table, namespaced by slug.
Apps are explicit-grant: enabling gives members nothing until an admin allows the view.
Building them: `docs/APPS.md` + `docs/sdk/`.

## Jobs and durable runs

One scheduler (`scheduler.ts`), registered jobs with named intervals, **exactly-once via a
Redis lease** — one run per interval fleet-wide; unreachable Redis skips the tick rather
than running unguarded. The first eight jobs are required: a missing registration fails
boot. The scheduler runs in prod only — `vite dev` never arms it (`TALARIA_SCHEDULER=off`
is the prod kill switch), which is why "my background job never fires locally" is usually
not a bug. The app container warms the route graph with an in-process health check before
listening, so jobs run only on an instance that can serve.

The scheduled family: comms decay (distill idle threads), the daily digest and approval
escalation, the daily brief (opens per person, per timezone), notification mail, outreach
sweeps, run reclaim (re-enters a run whose driver died), price refresh, update checks.
Research/plan runs are durable on the same plane: derived run ids + per-step leases, so two
instances cannot double-run and a restart resumes at the last persisted turn.
