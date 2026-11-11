# Contributing to Talaria

Talaria is MIT-licensed — issues and pull requests are the whole idea. The on-ramp:
[`DEVELOPERS.md`](./DEVELOPERS.md) (setup, repo map, every doc),
[`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) (how it works), and
[`README.md`](./README.md) (what it is).

## The flow, in one screen

**Pull requests target `testing`.** `testing` is the integration lane: your pull
request gets light CI there (the surfaces it touched, Linux), it lands as a merge
commit once that is green, and every push to `testing` deploys the
[testing instance](./docs/TESTING-INSTANCE.md), where changes are exercised
together. `rc` is the release candidate and the staging environment: it takes a
frozen snapshot of `testing` (`promote/testing-<sha12>`, full CI on every
platform, a code-owner review), and every push to it is deployed and smoke-tested
by `.github/workflows/rc-deploy.yml`. `main` is the release trunk, and it takes
one input only — a promotion of the whole of `rc`, offered by
`.github/workflows/promote.yml` once that deploy is green. The model, the gates
and the repository settings that enforce them:
[`docs/BRANCHES.md`](./docs/BRANCHES.md).

**The hotfix lane.** A fix `rc` needs before the next snapshot is a `hotfix/<name>`
branch cut from `origin/rc` (never from testing), with no migration in it, opened
against `rc`; a maintainer merges it, and the back-flow into `testing` is opened
automatically.

**Who merges.** Into `testing`, the author, once the checks are green (maintainers
with admin rights merge their own; an outside contributor's pull request waits
for a code owner's approval). Into `rc` and `main`, a maintainer, after a
code-owner review — always as a merge commit.

You do not have to remember any of that from a doc: `scripts/flow-guard.mjs`
refuses a feature aimed at `rc` or `main`, refuses a direct or squashed push to
any of the three, and `talaria setup` wires the same rule into `git push` locally.

## Before you send a PR

1. **Verify on CI.** `bun run check` from the repo root — seconds, no install. Then CI is the
   compile and the test: once the change is code-complete, open the pull request (a draft is
   fine) and its own checks are the proof; before one exists, `bun run gate:remote` pushes
   HEAD to `ci/<branch>` and reports the first red job with its log. Run either in the
   background. Locally, only cheap targeted runs — one `bunx vitest run <file>`, one
   `bun test <file>`, `cargo fmt`. Do not run a workspace cargo (`bun run api:check`,
   `bun run desktop:check`, `cargo test` / `clippy` without `-p`) to be safe — those lock
   `api/target` and pin the machine for everyone else in the checkout. A red CI log names the
   crate and the test; `cargo test -p <crate> <test>` reproduces that one failure.
2. **Exercise the path you changed** in the running app (`bun talaria dev` →
   <http://localhost:5273>) — typecheck alone doesn't prove a surface works.
3. **Add a changelog entry file** — `changelog/YYYY-MM-DD-<slug>.md`, the bullet verbatim
   (bold lead, what changed, what you verified). One file per change; the release roll
   folds them into [`CHANGELOG.md`](./CHANGELOG.md), and the release notes are built from
   them. The Verified line names the gate — "CI: this PR's checks", or `gate:remote` — and
   what you exercised, never a run URL: a commit that only adds a link re-runs the whole
   battery. Two optional extras:
   - an indented `Upgrade:` paragraph, for a step an operator takes, such as rolling agents
     or setting a variable. The notes list it under "Before you upgrade".
   - a `<!-- bump: minor -->` (or `major`) line, when the change is more than a patch.
4. **Commit in the repo's style:** `area: lowercase sentence — explanation`, one change per
   commit, the changelog entry riding with the change it describes. `git log` carries the
   voice; the em-dash clause says why, not what.
5. **Open the pull request against `testing`** with a body that mirrors the changelog entry: what
   changed, and what you verified. Run links — this PR's checks, a `gate:remote` result — go
   in the body (`gh pr edit <n> --body-file …`), which starts no compile. If a whole-tree gate
   failed on something that predates your change (several sessions share this tree), say so
   rather than absorbing it silently.
6. **Watch the pull request until it is actually green.** Do not call the work done while its
   checks are failing, still pending, or the PR conflicts with its base.
   `node scripts/hooks/pr-watch.mjs`, run in the background, is that gate. Then merge it
   into `testing` and, once `node scripts/hooks/testing-watch.mjs <n>` says it is live,
   exercise it on the testing instance. The procedure is
   [`.claude/skills/ship-a-change/SKILL.md`](./.claude/skills/ship-a-change/SKILL.md).
7. **Remove the local workspace this task created** before you call the work done —
   worktree, devbox, scratch checkout, throwaway download. Shared caches stay. Do not
   tear it down while the watcher is still red; you may need it to fix. The procedure is
   [`.claude/skills/cleanup/SKILL.md`](./.claude/skills/cleanup/SKILL.md).

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

Reviews happen on the pull request — into `testing` for a change, into `rc` for a snapshot or a
hotfix — and a review is about the change rather than
the author: what could break, what the changelog claims versus what the diff shows, and whether
the verification was real. [`CODE_OF_CONDUCT.md`](./CODE_OF_CONDUCT.md) is the standard we hold
each other to. Security issues do not belong in an issue tracker entry — report them privately
through [`SECURITY.md`](./SECURITY.md).