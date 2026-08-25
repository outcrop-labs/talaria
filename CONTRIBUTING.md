# Contributing to Talaria

Talaria is MIT-licensed. It's a multiplayer-first agentic business platform: one workspace where your
people and your AI agents share boards, chats, plans, and more, and run the business together with
human-in-the-loop guardrails. The product is Talaria's own app in [`ui/`](./ui) (Vite + Svelte 5),
backed by its own Postgres/Redis. Underneath the app sits the fleet: a set of Hermes agent containers
that Talaria renders and manages. Every agent routes its LLM **and** its persona chat through Talaria's
own gateway — Talaria reaches each agent directly on its published port, so there's no separate
multiplexer. The Python Hermes **plugin** rides on each agent (register / heartbeat / report). The whole
thing runs on one `talaria` docker network with **no Dockerfiles** — official/published images plus the
host-run app.

## Layout

| Path | What | Build / test |
|---|---|---|
| `ui/` | the Talaria app (product + LLM gateway + fleet renderer) | `cd ui && bun i && bun run dev`; `bun run typecheck` |
| `mcp/` | `talaria-mcp` — the agent-facing MCP server | `cd mcp && bun i && bun run build` |
| `plugin/talaria/` | per-agent Hermes plugin (register / heartbeat / report) | `python3 -m py_compile plugin/talaria/*.py` |
| `docker/dev-compose.yml` | dev infra (Postgres + Redis) | `docker compose -f docker/dev-compose.yml config` |
| `scripts/setup.sh`, `scripts/dev.sh` | first-run setup + bring-up | `./scripts/setup.sh` then `./scripts/dev.sh` |

The fleet itself is **rendered**, not hand-written: Talaria materializes `fleet/` (gitignored —
`docker-compose.yml`, `fleet.json`, per-agent config) from one Talaria-owned chassis when you design an
agent in the app.

## Dev loop

1. `./scripts/setup.sh` once (generates `ui/.env`, the fleet config plane, the `talaria` network, pulls
   infra images, installs deps — prints your admin login).
2. `./scripts/dev.sh` brings up Postgres + Redis and the app on <http://localhost:5273>.
3. Add an LLM endpoint on `/models` (your provider's key is stored **encrypted in the DB**), then design
   an agent on `/agents`. Talaria renders the fleet and brings it up under the `talaria-fleet` compose
   project; each agent's models route through Talaria's gateway.
4. Verify your change: `bun run typecheck` in `ui/` (svelte-check), and exercise the affected path in the running app.

### Plugin distribution: "one dev instance, sync the rest"

The plugin lives once in this repo (`plugin/talaria/`). Each Hermes agent bind-mounts that *same*
directory read-only into `/opt/data/plugins/talaria` and opts in via `plugins.enabled: [talaria]`,
so there is a single source of truth and N synced mounts (no copy step). Edit the one directory;
recreate the agents to pick it up.

## Conventions

- **Everything through Talaria.** Agent LLM and persona chat route through Talaria's gateway, so calls
  are guarded, metered, and observable in one place. Don't wire agents at raw provider endpoints.
- **Secrets in the DB, never in configs.** Provider keys and tokens are sealed (AES-256-GCM envelope
  encryption) in Postgres; a config file never holds a live credential.
- **Keep the guardrails.** Agents create and triage, but they can't self-assign or self-complete. Never
  force a `done` transition; the final sign-off is a human's call.
- **Reuse the primitives.** Build on the components in `ui/src/components/ui/`; don't recreate them.
- Match surrounding style; the app is strict TypeScript, the plugin is stdlib-only Python.

## Working in parallel

Spin up an **isolated** dev stack per branch instead of running two servers
against one database — see [`docs/WORKTREES.md`](./docs/WORKTREES.md)
(`./scripts/worktree.sh <name>`). How secrets are protected and the one rule that
keeps them recoverable is in [`docs/ENCRYPTION.md`](./docs/ENCRYPTION.md).

## Pull requests

Include what you verified (typecheck + the exercised path) and update `CHANGELOG.md`.
