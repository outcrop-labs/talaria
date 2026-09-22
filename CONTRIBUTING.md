# Contributing to Talaria

Talaria is MIT-licensed — issues and pull requests are the whole idea. The on-ramp:
[`DEVELOPERS.md`](./DEVELOPERS.md) (setup, repo map, every doc),
[`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) (how it works), and
[`README.md`](./README.md) (what it is).

## The flow, in one screen

**Pull requests target `rc`. Nothing targets `main`.** `rc` is the integration
branch *and* the staging environment: your pull request is reviewed there, CI runs
there, and every push to it is deployed and smoke-tested by
`.github/workflows/rc-deploy.yml`. `main` is the release trunk, and it takes one
input only — a promotion of the whole of `rc`, offered by
`.github/workflows/promote.yml` once that deploy is green and merged by a
maintainer as a merge commit. The model, the gates and the repository settings
that enforce them: [`docs/BRANCHES.md`](./docs/BRANCHES.md).

You do not have to remember any of that from a doc: `scripts/flow-guard.mjs`
refuses a pull request aimed at `main`, refuses a direct or squashed push to
either branch, and `talaria setup` wires the same rule into `git push` locally.

## Before you send a PR

1. **Verify.** `bun run verify` from the repo root (typecheck + tests + invariants + the docs
   and generated-reference drift checks). Green before you push, every time. Touched `api/`?
   `bun run api:check` too; touched `desktop/`? `bun run desktop:check`.
2. **Exercise the path you changed** in the running app (`bun talaria dev` →
   <http://localhost:5273>) — typecheck alone doesn't prove a surface works.
3. **Add a changelog entry file** — `changelog/YYYY-MM-DD-<slug>.md`, the bullet verbatim
   (bold lead, what changed, what you verified). One file per change; the release roll
   folds them into [`CHANGELOG.md`](./CHANGELOG.md).
4. **Commit in the repo's style:** `area: lowercase sentence — explanation`, one change per
   commit, the changelog entry riding with the change it describes. `git log` carries the
   voice; the em-dash clause says why, not what.
5. **Open the pull request against `rc`** with a body that mirrors the changelog entry: what
   changed, and what you verified. If a whole-tree gate failed on something that predates your
   change (several sessions share this tree), say so rather than absorbing it silently.

A test earns its place only where a plausible bug would fail it. Behaviour, boundaries,
invariants, transitions, real errors — not wiring, defaults, or the shape of the source.
A behaviour change with no test that would catch its regression is a review conversation.

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
- **Migrations are roll-safe.** An update runs two app containers side by side
  until the drain completes, so a migration shipping with code may only *add*;
  dropping or rewriting what the old code still reads ships at least one release
  later ([`docs/UPDATES.md`](./docs/UPDATES.md) — the overlap contract).

## Style

Match the surrounding code. The app is strict TypeScript (Svelte 5 runes); the Hermes plugin
is stdlib-only Python. Full conventions:
[`docs/API-CONVENTIONS.md`](./docs/API-CONVENTIONS.md) ·
[`docs/UI-CONVENTIONS.md`](./docs/UI-CONVENTIONS.md).

## Working in parallel

One dev stack per branch, not two servers against one database:
[`docs/WORKTREES.md`](./docs/WORKTREES.md) (`bun talaria worktree <name>`), or a
containerized devbox per task: [`docs/DEVBOX.md`](./docs/DEVBOX.md).

## Review, conduct, and security

Reviews happen on the pull request against `rc`, and a review is about the change rather than
the author: what could break, what the changelog claims versus what the diff shows, and whether
the verification was real. [`CODE_OF_CONDUCT.md`](./CODE_OF_CONDUCT.md) is the standard we hold
each other to. Security issues do not belong in an issue tracker entry — report them privately
through [`SECURITY.md`](./SECURITY.md).