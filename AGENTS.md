# AGENTS.md — building Talaria

This is the canonical instruction file for anyone — or anything — writing code against this
repo: human contributors and coding agents alike. It is written for every harness: opencode,
Pi, Codex, and Oh My Pi read it directly; Claude Code imports it through `CLAUDE.md`.

The split that keeps it small: **universal invariants live here** (this file loads into agent
context at launch — keep it under ~200 lines), **procedures live in the skills**
(`.claude/skills/`, plain markdown, indexed below), and **gates run as scripts**
(`scripts/hooks/`). How the three layers fit together — and how each harness consumes them:
[`docs/AGENT-TOOLING.md`](./docs/AGENT-TOOLING.md).

## Orientation

- [`DEVELOPERS.md`](./DEVELOPERS.md) — the repo map and the index of every doc. Start here.
- [`CONTRIBUTING.md`](./CONTRIBUTING.md) — PR norms: verify, exercise, changelog.
- [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) — how the platform works.
- [`HANDOFF.md`](./HANDOFF.md) — where to start when picking up ongoing work.
- Conventions by surface: [`docs/API-CONVENTIONS.md`](./docs/API-CONVENTIONS.md) ·
  [`docs/UI-CONVENTIONS.md`](./docs/UI-CONVENTIONS.md) ·
  [`docs/RUST-MIGRATION.md`](./docs/RUST-MIGRATION.md).

## Commands

Bun is the runner for the whole repo (the root `package.json` is the hub):

| Command | What it does |
|---|---|
| `bun run dev` | full dev stack → <http://localhost:5273> |
| `bun run api` | the Rust api (`cargo run`, :5274) |
| `bun run check` | invariants + doc links + generated-reference drift — seconds, **needs no install** |
| `bun run api:check` | fmt + clippy `-D warnings` + cargo tests — the CI api job |
| `bun run verify` | check + typecheck + test — the PR gate |
| `bun run test` / `typecheck` / `build` / `start` | the ui/ scripts (typecheck is svelte-check) |
| `bun run docs:api` | regenerate the generated references |

The `talaria` CLI drives everything else — setup, dev, boxes, worktrees, resets, backups,
deploys. The full command table is generated at
[`docs/CLI-REFERENCE.md`](./docs/CLI-REFERENCE.md).

Ports: dev UI **5273**, Rust api **5274**. Worktree stacks get deterministic per-name ports
(app `53xx`, Postgres `56xx`, Redis `65xx`).

## The rules that aren't style

Distilled from [`CONTRIBUTING.md`](./CONTRIBUTING.md) — the full text is the contract:

- Everything through the Talaria gateway — never wire an agent at a raw provider endpoint.
- Secrets live envelope-encrypted in Postgres, never in config files
  ([`docs/ENCRYPTION.md`](./docs/ENCRYPTION.md)).
- Never force a `done` transition — agents create and triage; a human signs off.
- Reuse the `ui/src/components/ui/` primitives; don't recreate them.
- Generated docs are regenerated, never hand-edited:
  [`docs/CLI-REFERENCE.md`](./docs/CLI-REFERENCE.md) and
  [`docs/api/`](./docs/api/README.md) both come out of the generator behind
  `bun run check` — change the source, run `bun run docs:api`.
- `bun run verify` green before every push, every time.

## Environment facts

- **One dev stack per branch — never two servers on one database.** A second process with a
  stale encryption key can re-seal secrets under the wrong key and corrupt them for
  everyone. Use `bun talaria worktree <name>` (fully isolated: own Postgres + Redis, a
  seeded DB, its own ports); `talaria dev` **refuses** to start in a worktree lacking the
  setup marker, for exactly this reason ([`docs/WORKTREES.md`](./docs/WORKTREES.md)).
- `bun run check` runs with no `node_modules` at all — everything under `scripts/` is
  stdlib-only. Use it as the fast inner gate anywhere, any time.
- Parallel *agent* sessions: devboxes give each task a container with the agent CLIs inside
  ([`docs/DEVBOX.md`](./docs/DEVBOX.md)). Never share a host `~/.claude` across concurrent
  CLIs — it corrupts `.claude.json`.

## Parallel sessions share working trees

Several agent sessions routinely work this repo at once. Consequences:

- `git status` before you stage anything — uncommitted files may belong to another session.
- Stage by explicit path. Never `git add -A`, never `git clean`, never an unscoped reset.
- The stop gate (below) checks the whole tree — a failure may predate your change. If it
  does, verify that and say so in the PR.

## Skills

Procedures live in `.claude/skills/<name>/SKILL.md` — plain markdown, readable by any agent.
Claude Code and opencode discover them natively; on anything else, read the file when the
situation matches.

| Skill | Use it when |
|---|---|
| [`dev-loop`](./.claude/skills/dev-loop/SKILL.md) | starting or restarting the stack, choosing worktree vs devbox, seeding data, or deciding which command verifies which surface |
| [`repo-traps`](./.claude/skills/repo-traps/SKILL.md) | a change that should work fails oddly — a 500, a zombie port, docker DNS, or an API test that needs auth |
| [`ship-a-change`](./.claude/skills/ship-a-change/SKILL.md) | a change is code-complete: gates, exercising the path, CHANGELOG, commit and PR conventions |
| [`cut-release`](./.claude/skills/cut-release/SKILL.md) | cutting an RC or stable release, or diagnosing why a channel or image tag didn't move |

## Known traps

One line each — the full symptom → check → fix procedure is the
[`repo-traps`](./.claude/skills/repo-traps/SKILL.md) skill:

- A query 500s that looks right → bind casts: `$N::uuid` on a TEXT column, or a missing
  `$N::timestamptz` on an epoch-ms bind.
- The whole API 500s after a route/import edit — and keeps 500ing after the fix → vite
  serving a stale negative; restart the dev server before debugging the code.
- Port 5274 looks free but runs fail with bugs your binary doesn't have → a "killed" api
  process still ticking its scheduler with no port bound.
- Docker builds fail resolving hosts → the daemon's resolver config; builds may need
  `--network=host`.
- Authed curl without a browser → mint a Redis session directly.

## The stop gate

`scripts/hooks/stop-check.mjs` runs `bun run check` — the same chain CI runs — under one
contract, so any harness, git hook, or shell can invoke it:

- **stdin** — whatever event payload the caller pipes in: tolerated, never read.
- **exit 0** — pass, silent.
- **exit 2** — block; the reason is on stderr (Claude Code feeds it back to the model).
- **any other exit** — the gate could not run: a non-blocking error, never a pass.

It always runs, on the whole tree, with no diff fast path — a seconds-long, dependency-free
check is cheaper than the scope-matching that would skip it. Wiring per harness (Claude
Code's tracked [`settings.json`](./.claude/settings.json) Stop hook, the git pre-push
recipe, CI): [`scripts/hooks/README.md`](./scripts/hooks/README.md). No permissions are
tracked anywhere — personal allowlists live in `.claude/settings.local.json`, untracked.

## Do not touch

- `apps/leadworks/`, `apps/waypoint/` — gitignored client subrepos with their own history.
- `fleet/` — rendered from the chassis by the app; never hand-edit.
- `scripts/skills/` — **product surface**: those skills ship to Hermes agent containers.
  Repo tooling skills go in `.claude/skills/`, never here.
- `docs/api/**` and `docs/CLI-REFERENCE.md` — generated; drift fails `bun run check`.
- `mcp/dist/`, `ui/src/routeTree.gen.ts` — build output.
- `CHANGELOG.md` is append-only; its links are frozen history.

## Conventions

- Commits: `area: lowercase sentence — explanation` (`git log` carries the voice).
- Every user-visible change appends to [`CHANGELOG.md`](./CHANGELOG.md) — what changed and
  what you verified.
- Keep this file under ~200 lines: a new invariant earns a line here; a new procedure earns
  a skill.
