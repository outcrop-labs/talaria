# Contributing to Talaria

Talaria is MIT-licensed. It's a multiplayer-first agentic business platform: one workspace where your
people and your AI agents share boards, chats, plans, and more, and run the business together with
human-in-the-loop guardrails. The product is Talaria's own app in [`ui/`](./ui) (Vite + TanStack Start),
backed by its own Postgres/Redis. Underneath the app sits the fleet engine, the runtime that talks to
your Hermes agents. The current fleet engine is the **gateway plane** in [`bridge/`](./bridge) (a fleet
multiplexer). The Python Hermes **plugin** rides on each agent; the mission-control **adapter** and the
older bridge routes are legacy Phase-1 scaffolding, kept but no longer the architecture. A docker
**stack** wires the runtime together.

## Layout

| Path | What | Build / test |
|---|---|---|
| `bridge/` | gateway plane: fleet multiplexer (legacy dashboard/conductor routes live here too) | `cd bridge && npm i && npm run build` (`npm run typecheck`) |
| `plugin/talaria/` | per-agent Hermes plugin (register / heartbeat / report) | `python3 -m py_compile plugin/talaria/*.py` |
| `adapter/` | mission-control `HermesAdapter` (legacy Phase-1 lift source) | applied to a mission-control checkout |
| `stack/` | compose that wires the fleet engine together | `docker compose -f stack/docker-compose.yml config` |
| `scripts/verify-stack.sh` | end-to-end smoke test (M1-M3) | run against an up stack |

## Dev loop

1. Bring up the stack (see [`stack/README.md`](./stack/README.md)). It needs the shared `edge` network
   and reachable agent gateways. (The legacy Phase-1 mission-control build is documented in
   `stack/docker-compose.yml` with its pinned commit if you need it.)
2. Iterate on the gateway plane: `docker compose -f stack/docker-compose.yml up -d --build talaria-bridge`.
   Set `TALARIA_LOG_REQUESTS=1` to log requests (handy for capturing new routes).
3. Verify: `scripts/verify-stack.sh` (exits non-zero on the first failing check).

### Plugin distribution: "one dev instance, sync the rest"

The plugin lives once in this repo (`plugin/talaria/`). Each Hermes agent bind-mounts that *same*
directory read-only into `/opt/data/plugins/talaria` and opts in via `plugins.enabled: [talaria]`,
so there is a single source of truth and N synced mounts (no copy step). Edit the one directory;
recreate the agents to pick it up.

## Conventions

- **Non-destructive first.** The gateway plane passes unknown routes through untouched
  (allowlist-intercept, never denylist). The plugin only reaches *out* and never alters agent output.
- **Keep the guardrails.** Agents create and triage, but they can't self-assign or self-complete. Never
  force a `done` transition; the final sign-off is a human's call.
- Keep the compatibility matrix (README) honest. Upstreams move fast, so pin what you verify.
- Match surrounding style; the gateway plane is strict TypeScript, the plugin is stdlib-only Python.

## Pull requests

Include what you verified (ideally a `verify-stack.sh` run) and update `CHANGELOG.md`. Upstream
contributions (the `HERMES_MISSION_API_URL` workspace PR, the mission-control adapter) are tracked in
[`docs/m0-contract.md`](./docs/m0-contract.md) and [`adapter/`](./adapter).
