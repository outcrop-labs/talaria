---
name: dev-loop
description: Run and exercise the Talaria dev stack — bring-up, ports, worktree/devbox isolation, seeding, and the check/verify gates. Use when starting or restarting the stack, choosing between a worktree and a devbox, seeding realistic data, or deciding which command verifies which surface.
---

# Dev loop

The inner loop: bring the stack up, exercise the surface you changed, and know which gate
proves what. The repo map and full doc index live in `DEVELOPERS.md` — this skill is
procedure only.

## Bring-up

```bash
bun talaria setup      # first run only — prints the generated admin credentials
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
| a worktree stack | app `53xx`, Postgres `56xx`, Redis `65xx` — deterministic per name |

## The gates, and what each proves

| Gate | Proves | Cost |
|---|---|---|
| `bun run check` | invariants + every doc link resolves + generated references not drifted | seconds, **no install needed** |
| `bun run api:check` | fmt + clippy `-D warnings` + cargo tests — what the CI api job runs | minutes |
| `bun run typecheck` | svelte-check over the ui (it checks `.svelte` files — tsc alone doesn't) | ~a minute |
| `bun run verify` | check + typecheck + test — the PR gate | the sum |

Use `bun run check` as the fast inner gate everywhere — it runs in a fresh worktree before
any install. `verify` is the pre-push gate (see the ship-a-change skill).

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
  pg_dump-seeded DB, `node_modules` symlinked from main. Teardown:
  `docker compose -p talaria-wt-<name> down -v && git worktree remove ../talaria-<name> && git branch -D wt/<name>`.
  Full rules: docs/WORKTREES.md.
- **Devbox** (container, heavy): `bun talaria box new <name>` — a full stack per task with
  the agent CLIs pinned inside. Use for parallel *agent* sessions; never share a host
  `~/.claude` across concurrent CLIs (it corrupts `.claude.json`). Full guide:
  docs/DEVBOX.md.

## When something fails oddly

A change that should work but doesn't — 500s, zombie ports, DNS failures in builds — is
usually a known trap. Switch to the repo-traps skill before debugging from scratch.
