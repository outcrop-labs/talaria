# Dev worktrees

Work on several branches at once without them stepping on each other — or on your
main dev environment.

## TL;DR

```bash
git wt <name>                         # = ./scripts/worktree.sh <name> (alias set by setup.sh)
cd ../talaria-<name>/ui && bun run dev -- --port <printed>
# … hack away …
docker compose -p talaria-wt-<name> down -v   # tear down when done
git worktree remove ../talaria-<name> && git branch -D wt/<name>
```

`setup.sh` registers `git wt` as the alias, and `dev.sh` **refuses to start** in a
linked worktree that wasn't set up this way (see [Manual worktrees](#manual-worktrees))
— so you can't accidentally point a second app at your main DB.

Each worktree is **fully isolated**: its own git worktree, its own Postgres +
Redis, its own `ui/.env` on unique ports. It **cannot** touch your main dev DB,
so nothing you do in a worktree can break your primary stack.

## Why isolation (not a shared DB)

Talaria's app keeps mutable state in Postgres/Redis — conversations, boards, the
fleet registry, **and encrypted secrets**. Two app instances pointed at the *same*
database will fight over that state. The sharpest edge is encryption: a second
process with a stale in-memory data key can re-seal secrets under the wrong key
and corrupt them for everyone (see [`ENCRYPTION.md`](./ENCRYPTION.md)). So a
worktree gets its **own** database, seeded with a point-in-time copy of main's
data so you still have realistic agents/boards/tickets to work against.

## What `scripts/worktree.sh <name>` does

1. `git worktree add ../talaria-<name> -b wt/<name>` off your current `HEAD`.
2. Brings up an isolated Postgres + Redis under compose project `talaria-wt-<name>`
   on **deterministic per-name ports** (app `53xx`, Postgres `56xx`, Redis `65xx`).
3. Seeds the new DB with `pg_dump` of your main DB (a snapshot — later changes on
   main don't propagate).
4. Writes the worktree's `ui/.env`: its own `DATABASE_URL`/`REDIS_URL`/`PORT`,
   **copying the encryption root key** (`TALARIA_SECRET_KEY`, or `AUTH_SECRET` if
   that's what you use) so the seeded secrets decrypt. This is the one thing
   shared, and it's a read-only root — see below.
5. Symlinks `node_modules` from main (fast; no reinstall).

It prints the exact `bun run dev` command and the teardown steps.

## Rules of the road

- **Never regenerate the encryption root in a worktree.** `worktree.sh` copies it
  on purpose. If a worktree got its own fresh `AUTH_SECRET`/`TALARIA_SECRET_KEY`,
  it could not decrypt the seeded secrets — and if it then wrote to a *shared* DB
  it would orphan them. (This is exactly what broke the env once. The fix:
  `setup.sh` now writes a dedicated, stable `TALARIA_SECRET_KEY`; keep it constant.)
- **The worktree's fleet is separate.** Fleet renders resolve to the worktree's
  own `fleet/` dir; a worktree does not manage your main agents.
- **Agent LLM calls** still flow through the *main* gateway if you're testing chat
  — the fleet agents point at whatever `LLM_BASE_URL` they were rendered with.
  Isolated worktrees are for app/UI iteration, not for re-rendering the live fleet.
- **Clean up** with the two teardown commands the script prints; `-v` drops the
  isolated volumes so nothing lingers.

## Manual worktrees

`worktree.sh` stamps `TALARIA_WORKTREE=<name>` in the worktree's `ui/.env`. If you
make a worktree by hand (`git worktree add …`) instead, that marker is absent, so
**`dev.sh` will refuse to start there** — because without its own stack it would
share the main DB. Either use `git wt <name>` / `worktree.sh`, or, if you know
what you're doing, give the worktree its own `DATABASE_URL`/`REDIS_URL` and add
`TALARIA_WORKTREE=<name>` to its `ui/.env` yourself.
