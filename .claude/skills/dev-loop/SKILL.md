---
name: dev-loop
description: Run and exercise the Talaria dev stack — bring-up, ports, worktree/devbox isolation, seeding, and the gates (a local check; CI is the compile). Use when starting or restarting the stack, choosing between a worktree and a devbox, seeding realistic data, or deciding which command verifies which surface.
---

# Dev loop

The inner loop: bring the stack up, exercise the surface you changed, and know which gate
proves what. The repo map and full doc index live in `DEVELOPERS.md` — this skill is
procedure only.

## Bring-up

```bash
bun talaria setup      # first run only — secrets, config, deps; the admin account is claimed in the app
bun talaria dev        # dev infra + the app → http://localhost:5273
bun run api            # the Rust api alone (:5274), when that's the surface in play
```

Reset a wedged stack with `bun talaria reset <mode>`; snapshot and restore with
`bun talaria backup` / `restore` (docs/BACKUPS.md).

## Ports

| What | Port |
|---|---|
| dev UI | 5273 |
| Rust api | 5274 |
| a worktree stack | app `53xx`, api `54xx`, Postgres `56xx`, Redis `65xx`, toolkit `57xx`, omp auth `58xx`, agent gateways from `20000 + slot·100` — one slot per stack |

## The gates, and what each proves

| Gate | Where | Proves | Cost |
|---|---|---|---|
| `bun run check` | local | invariants + every doc link resolves + generated references not drifted | seconds, **no install needed** |
| the PR's CI (`node scripts/hooks/pr-watch.mjs`), or `bun run gate:remote` before a PR exists | GitHub | compile + test — the proof. `gate:remote` is light (Linux jobs, the surfaces the branch moved); a PR into `rc` runs full | minutes on free runners, nothing on this machine |
| `bun run gate:remote --full` | GitHub, full | every surface, plus macOS, Windows, iOS and e2e | longer; still nothing on this machine |
| `bun run gate` | local, reproduce only | `check`, plus compile/test of what the diff touches | the compiles you changed — only to reproduce one failure CI reported |
| `api:check`, `desktop:check`, `verify`, `typecheck` | CI's jobs | the whole api / desktop / ui surface | minutes, locks `api/target`, pins the machine — never local |

Locally: `bun run check` — it runs in a fresh worktree before any install — and cheap
targeted runs (one `bunx vitest run <file>`, one `bun test <file>`, `cargo fmt`). The
compile is CI's: the pull request's own checks, or `bun run gate:remote` before there is
one (the ship-a-change skill). Do not run a workspace cargo locally, and do not set
`CARGO_BUILD_JOBS`: the cap in `api/.cargo/config.toml` is what keeps a compile from pinning
the host.

**Background.** Both CI gates wait minutes; a harness kills a foreground command long before
(Claude Code at 10). Start `pr-watch` and `gate:remote` in the harness's background mode
(Claude Code: `run_in_background`) and act on the exit. A killed one loses nothing:
`gate:remote --watch <run-id>` resumes, and `pr-watch` re-reads the PR from scratch.

## Exercising the path you changed

Typecheck proves nothing about behavior. Drive the real surface, in the running app:

- Data-dependent surfaces (Home brief, boards, inbox): `node scripts/seed-daily-brief.mjs`
  seeds a realistic morning — additive, only touches rows it wrote, `--clean` removes them.
  An empty install shows nothing; seed before you conclude a surface is broken.
- API paths: mint a session and curl with it — the recipe is in the repo-traps skill
  (trap 5).
- UI paths: use the browser. Screenshots are evidence; "it compiled" is not.

## Isolation: worktree or devbox

Never two servers on one database — a stale encryption key re-seals secrets wrong
(docs/ENCRYPTION.md). Pick your isolation:

- **Worktree** (host, light): `bun talaria worktree <name>` (alias `git wt <name>`) — own
  git worktree at `../talaria-<name>`, own Postgres + Redis on deterministic ports, a
  pg_dump-seeded DB, `ui/node_modules` symlinked from main and `cli/` + `mcp/`
  installed from the branch's lockfiles, and an `api/target` **seeded by reflink**
  from the best idle checkout: the registry crates arrive built, and every workspace
  crate builds from this tree's sources (`--desktop` seeds the desktop target too,
  `--no-seed` starts empty). Do not copy another tree's `target/` by hand: a whole
  copy passes another worktree's workspace crates off as this one's, and cargo
  reuses them. `bun talaria seed-target` reseeds an empty one. Teardown:
  `docker compose -p talaria-wt-<name> down -v && git worktree remove --force ../talaria-<name> && git branch -D wt/<name>`.
  Run that from the primary checkout, not from inside the worktree. Full rules: docs/WORKTREES.md.
- **Devbox** (container, heavy): `bun talaria box new <name>` — a full stack per task with
  the agent CLIs pinned inside. Use for parallel *agent* sessions; never share a host
  `~/.claude` across concurrent CLIs (it corrupts `.claude.json`). Tear it down with
  `bun talaria box rm <name>`. Full guide: docs/DEVBOX.md.

## When the task ends

Remove what this task created before you claim done, and not while a pull request
from this task is still red — you may need the workspace to fix. The convention —
what goes, what stays, and the command — is the [cleanup](../cleanup/SKILL.md) skill.
Scratch checkouts and throwaway downloads go under `/tmp/talaria-<name>` so the sweep
can see them. `bun talaria cleanup` reports what is still there; the stop gate runs
the sweep and blocks when disk use or the stale set crosses the line. It does not
delete a fresh worktree for you.

## When something fails oddly

A change that should work but doesn't — 500s, zombie ports, DNS failures in builds — is
usually a known trap. Switch to the repo-traps skill before debugging from scratch.
