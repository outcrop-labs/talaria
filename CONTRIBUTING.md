# Contributing to Talaria

Talaria is MIT-licensed — issues and pull requests are the whole idea. The on-ramp:
[`DEVELOPERS.md`](./DEVELOPERS.md) (setup, repo map, every doc),
[`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) (how it works), and
[`README.md`](./README.md) (what it is).

## Before you send a PR

1. **Verify.** `bun run verify` from the repo root (typecheck + tests + invariants + the docs
   and generated-reference drift checks). Green before you push, every time.
2. **Exercise the path you changed** in the running app (`bun talaria dev` →
   <http://localhost:5273>) — typecheck alone doesn't prove a surface works.
3. **Update [`CHANGELOG.md`](./CHANGELOG.md)** with what changed and what you verified.

## The rules that aren't style

- **Everything through Talaria.** Agent LLM and persona chat route through the gateway —
  guarded, metered, observable. Don't wire agents at raw provider endpoints.
- **Secrets in the DB, never in configs.** Envelope-encrypted in Postgres; a config file
  never holds a live credential ([`docs/ENCRYPTION.md`](./docs/ENCRYPTION.md)).
- **Keep the guardrails.** Agents create and triage, but they can't self-assign or
  self-complete; the final sign-off is a human's call. Never force a `done` transition.
- **Reuse the primitives.** Build on `ui/src/components/ui/`; don't recreate them.
- **Docs are generated where they can drift.** The CLI reference and the HTTP API
  reference both come out of the generator behind `bun run check` — don't hand-edit
  [`docs/CLI-REFERENCE.md`](./docs/CLI-REFERENCE.md) or
  [`docs/api/`](./docs/api/README.md); change the source and regenerate
  (`bun run docs:api`).

## Style

Match the surrounding code. The app is strict TypeScript (Svelte 5 runes); the Hermes plugin
is stdlib-only Python. Full conventions:
[`docs/API-CONVENTIONS.md`](./docs/API-CONVENTIONS.md) ·
[`docs/UI-CONVENTIONS.md`](./docs/UI-CONVENTIONS.md).

## Working in parallel

One dev stack per branch, not two servers against one database:
[`docs/WORKTREES.md`](./docs/WORKTREES.md) (`bun talaria worktree <name>`), or a
containerized devbox per task: [`docs/DEVBOX.md`](./docs/DEVBOX.md).
