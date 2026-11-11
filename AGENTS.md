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
- [`docs/BRANCHES.md`](./docs/BRANCHES.md) — the branch model: pull requests target `testing`;
  rc takes snapshots of testing; main takes rc — and what each gate proves.
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
| `bun run gate:remote` | compile + test on GitHub before a PR exists: pushes HEAD to `ci/<branch>`, reports the first red job with its log — run it in the background (`--full` adds macOS/Windows/iOS/e2e) |
| `bun run judge` | the PR judge on your branch — judge-pr's checks plus the model review (needs `JUDGE_OPENROUTER_API_KEY`; without it, `bun run gate:remote` reviews the branch on GitHub) |
| `bun run gate` | local scoped compile — only to reproduce one failure CI reported |
| `bun run api:check` | fmt + clippy `-D warnings` + cargo tests — CI's api job; never local |
| `bun run desktop` | Talaria Desktop (`tauri dev`) — run on the **host**, never in a box (no display); `docs/DESKTOP.md` |
| `bun run desktop:check` | fmt + clippy + cargo tests + svelte-check — CI's desktop job; never local |
| `bun run verify` | check + typecheck + ui test — CI's ui/mcp job; never local |
| `bun run test` / `typecheck` / `build` / `start` | the ui/ scripts (typecheck is svelte-check) |
| `bun run docs:api` | regenerate the generated references |

The `talaria` CLI drives everything else — setup, dev, boxes, worktrees, cleanup, resets,
backups, deploys. The full command table is generated at
[`docs/CLI-REFERENCE.md`](./docs/CLI-REFERENCE.md).

Ports: dev UI **5273**, Rust api **5274**. Worktree stacks get deterministic per-name ports
for every service they own, api included (app `53xx`, api `54xx`, Postgres `56xx`, Redis `65xx`,
toolkit `57xx`, omp auth `58xx`, agent gateways from `20000 + slot·100`).

## The rules that aren't style

Distilled from [`CONTRIBUTING.md`](./CONTRIBUTING.md) — the full text is the contract:

- Everything through the Talaria gateway — never wire an agent at a raw provider endpoint (repo tooling such as the CI judge is not an agent).
- **Pull requests target `testing`.** testing is the integration lane: light CI, and every
  push deploys to the testing instance. Merge your own PR into testing once
  `scripts/hooks/pr-watch.mjs` is green (`gh pr merge <n> --merge --admin` — it bypasses the
  review, never the checks), then `node scripts/hooks/testing-watch.mjs <n>` tells you when it
  is live. `rc` takes a frozen snapshot of testing (`promote/testing-<sha>`; full CI +
  `rc-deploy.yml`) plus `hotfix/`, `release/`, `sync/` cut from rc; `main` moves only by the
  promotion of rc. People merge into rc and main. A direct or squashed push to any of the
  three is refused by [`scripts/flow-guard.mjs`](./scripts/flow-guard.mjs), locally and in CI;
  the model is [`docs/BRANCHES.md`](./docs/BRANCHES.md).
- Secrets live envelope-encrypted in Postgres, never in config files
  ([`docs/ENCRYPTION.md`](./docs/ENCRYPTION.md)).
- Never force a `done` transition — agents create and triage; a human signs off.
- Reuse the `ui/src/components/ui/` primitives; don't recreate them.
- Generated docs are regenerated, never hand-edited:
  [`docs/CLI-REFERENCE.md`](./docs/CLI-REFERENCE.md), [`docs/api/`](./docs/api/README.md)
  and the talaria-apps skill's `references/skeleton.md` all come out of the generator behind
  `bun run check` — change the source, run `bun run docs:api`.
- Compile and test on GitHub, not on this machine. Locally: `bun run check` (seconds) and cheap targeted runs — one `bunx vitest run <file>`, one `bun test <file>`, `cargo fmt`. The proof is CI: the pull request's own checks (watched by `scripts/hooks/pr-watch.mjs`), or `bun run gate:remote` before a PR exists — both in the background. A heavy build — `cargo build`/`test`/`clippy`, gradle, a whole-project svelte-check, a docker build — runs locally only to reproduce one failure CI reported, one `-p` crate or one test; `bun run gate` ([`scripts/gate.mjs`](./scripts/gate.mjs)) scopes it to the diff. Do not run a workspace cargo (`api:check`, `desktop:check`, `cargo test` / `clippy` without `-p`) — it locks `api/target` and pins the machine.
- Do not report dev work done while its open pull request has failing checks, pending
  checks, or merge conflicts against its base. Local gates cannot see that. The watcher is
  [`scripts/hooks/pr-watch.mjs`](./scripts/hooks/pr-watch.mjs); the procedure is
  [`ship-a-change`](./.claude/skills/ship-a-change/SKILL.md).
- Do not report a task done until its local workspace is gone. Remove the worktree,
  devbox, scratch checkout, and throwaway download this task created; shared caches
  stay. Every harness, before the claim — the procedure is the
  [`cleanup`](./.claude/skills/cleanup/SKILL.md) skill. The stop gate blocks when disk
  use or stale artifacts cross the line; that is the backstop, not the step.

## Environment facts

- **One dev stack per branch — never two servers on one database.** A second process with a
  stale encryption key can re-seal secrets under the wrong key and corrupt them for
  everyone. Use `bun talaria worktree <name>` (fully isolated: own Postgres + Redis, a
  seeded DB, its own ports); `talaria dev` **refuses** to start in a worktree lacking the
  setup marker, for exactly this reason ([`docs/WORKTREES.md`](./docs/WORKTREES.md)).
- `bun run check` runs with no `node_modules` at all — everything under `scripts/` is
  stdlib-only. Use it as the inner gate anywhere, any time; CI is the compile.
- Parallel *agent* sessions: devboxes give each task a container with the agent CLIs inside
  ([`docs/DEVBOX.md`](./docs/DEVBOX.md)). Never share a host `~/.claude` across concurrent
  CLIs — it corrupts `.claude.json`.
- **Clean up the local workspace before the claim.** The rule above is the step; the
  [`cleanup`](./.claude/skills/cleanup/SKILL.md) skill is how. The stop gate does not
  delete a fresh worktree for you.

## Parallel sessions share working trees

Several agent sessions routinely work this repo at once. Consequences:

- `git status` before you stage anything — uncommitted files may belong to another session.
- Stage by explicit path. Never `git add -A`, never `git clean`, never an unscoped reset.
- Push your own branch. `main`, `rc` and `testing` are not push targets — `talaria setup` wires the guard
  in as a git pre-push hook, and CI refuses the push regardless.
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
| [`ship-a-change`](./.claude/skills/ship-a-change/SKILL.md) | a change is code-complete: gates, changelog, the PR against `testing`, the merge, testing-watch — do not claim done on red, pending, or a conflict |
| [`judge-pr`](./.claude/skills/judge-pr/SKILL.md) | a pull request is being reviewed, or a `judge (review)` finding needs reading |
| [`cut-release`](./.claude/skills/cut-release/SKILL.md) | cutting an RC or stable release, or diagnosing why a channel or image tag didn't move |
| [`cleanup`](./.claude/skills/cleanup/SKILL.md) | before claiming a task done — remove the local workspace it created; also when the stop gate blocks on disk pressure or stale artifacts |

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
- A cargo test or clippy pins the machine → don't; CI is the compile (`bun run gate:remote`). The cap is `api/.cargo/config.toml`.

## The stop gate

`scripts/hooks/stop-check.mjs` runs `bun run check` — the same chain CI runs — under one
contract, so any harness, git hook, or shell can invoke it:

- **stdin** — whatever event payload the caller pipes in: tolerated, never read.
- **exit 0** — pass, silent.
- **exit 2** — block; the reason is on stderr (Claude Code feeds it back to the model).
- **any other exit** — the gate could not run: a non-blocking error, never a pass.

It always runs, on the whole tree, with no diff fast path — a seconds-long, dependency-free
check is cheaper than the scope-matching that would skip it. After that check passes it runs
[`scripts/cleanup-sweep.mjs`](./scripts/cleanup-sweep.mjs) `--gate`: exit 2 means disk pressure
or stale artifacts, and the way through is `bun talaria cleanup --apply`. That scan is not part
of `bun run check` — CI has no one's worktrees. Wiring per harness (Claude
Code's tracked [`settings.json`](./.claude/settings.json) Stop hook, Codex's tracked
[`.codex/hooks.json`](./.codex/hooks.json) Stop hook, the git pre-push
recipe, CI): [`scripts/hooks/README.md`](./scripts/hooks/README.md). No permissions are
tracked anywhere — personal allowlists live in `.claude/settings.local.json`, untracked.

The same contract carries the branch-flow guard: [`scripts/hooks/pre-push`](./scripts/hooks/pre-push)
runs [`scripts/flow-guard.mjs`](./scripts/flow-guard.mjs) against the refs a push is about to
send — exit 2, and the push stops with the reason and the way to do it instead.

The stop gate and the pre-push hook see the local tree. After the pull request opens, CI
and its base can still diverge. The watcher above speaks this same contract; it does not edit
or push.

## Do not touch

- `apps/leadworks/`, `apps/waypoint/` — gitignored client subrepos with their own history.
- `fleet/` — rendered from the chassis by the app; never hand-edit.
- `scripts/skills/` — **product surface**: those skills ship to Hermes agent containers.
  Repo tooling skills go in `.claude/skills/`, never here.
- `docs/api/**`, `docs/CLI-REFERENCE.md` and `scripts/skills/talaria-apps/references/skeleton.md`
  — generated; drift fails `bun run check`.

- `api/` is a Cargo workspace — a thin `talaria-api` binary plus one crate
  per engine under `api/crates/`. New domains get their own crate; the crate
  map and the dependency rules live in
  [`docs/RUST-MIGRATION.md`](./docs/RUST-MIGRATION.md)'s "Layout of the
  workspace".
- `mcp/dist/`, `ui/src/routeTree.gen.ts` — build output.
- `CHANGELOG.md` is append-only and written only by the roll (`scripts/changelog-roll.mjs`);
  unreleased entries live as files under `changelog/`, one per change.

## Conventions

- Commits: `area: lowercase sentence — explanation` (`git log` carries the voice).
- Every user-visible change adds `changelog/YYYY-MM-DD-<slug>.md` — what changed and
  what you verified, verbatim bullet; `bun run check` fails a hand-appended
  `[Unreleased]` bullet.
- Keep this file under ~200 lines: a new invariant earns a line here; a new procedure earns
  a skill.

