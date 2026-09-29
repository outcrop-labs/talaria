# Dev worktrees

Work on several branches at once without them stepping on each other — or on your
main dev environment.

## TL;DR

```bash
git wt <name>                         # = bun talaria worktree <name> (alias set by setup)
cd ../talaria-<name> && bun talaria dev
# … hack away …
docker compose -p talaria-wt-<name> down -v   # tear down when done
git worktree remove ../talaria-<name> && git branch -D wt/<name>
```

`talaria setup` registers `git wt` as the alias, and `talaria dev` **refuses to start** in a
linked worktree that wasn't set up this way (see [Manual worktrees](#manual-worktrees))
— so you can't accidentally point a second app at your main DB.

Each worktree is **fully isolated**: its own git worktree, its own Postgres +
Redis, its own **Rust api**, and its own `ui/.env` on unique ports. It **cannot**
touch your main dev DB, so nothing you do in a worktree can break your primary
stack.

> The api used to be the hole in that sentence. Every stack put it on the fixed
> `:5274`, and `talaria dev` **adopts** an api already listening there rather than
> double-binding — so the second worktree to start proxied every `/api/*` request
> to the *first* worktree's api, which is pointed at the first worktree's
> database. Two app servers, one database, silently, and the app answered
> normally the whole time because it was reading someone else's data rather than
> failing. Each worktree now gets its own api port (`54xx`) in its `ui/.env`.

## Why isolation (not a shared DB)

Talaria's app keeps mutable state in Postgres/Redis — conversations, boards, the
fleet registry, **and encrypted secrets**. Two app instances pointed at the *same*
database will fight over that state. The sharpest edge is encryption: a second
process with a stale in-memory data key can re-seal secrets under the wrong key
and corrupt them for everyone (see [`ENCRYPTION.md`](./ENCRYPTION.md)). So a
worktree gets its **own** database, seeded with a point-in-time copy of main's
data so you still have realistic agents/boards/tickets to work against.

## What `talaria worktree <name>` does

1. `git worktree add ../talaria-<name> -b wt/<name>` off your current `HEAD`.
2. Brings up an isolated Postgres + Redis under compose project `talaria-wt-<name>`
   on **deterministic per-name ports** (app `53xx`, api `54xx`, Postgres `56xx`,
   Redis `65xx`).
3. Seeds the new DB with `pg_dump` of your main DB (a snapshot — later changes on
   main don't propagate). It waits for main's `talaria` database to answer
   `pg_isready` first: the container existing is not the same as the database
   existing, and a worktree created moments after `talaria dev` used to die
   mid-way with `FATAL: database "talaria" does not exist` — after both
   containers were up and before `ui/.env` was written, which is the step that
   makes the checkout usable.
4. Writes the worktree's `ui/.env`: its own
   `DATABASE_URL`/`REDIS_URL`/`PORT`/`TALARIA_API_PORT`, **copying the encryption
   root key** (`TALARIA_SECRET_KEY`, or `AUTH_SECRET` if that's what you use) so
   the seeded secrets decrypt. This is the one thing shared, and it's a read-only
   root — see below. Any of those four inherited from main is stripped first:
   `envValue` takes the *first* match, so a leftover line would shadow the one
   written here.
5. Symlinks `node_modules` from main (fast; no reinstall).

It prints the exact `talaria dev` command and the teardown steps.

## Rules of the road

- **Never regenerate the encryption root in a worktree.** `talaria worktree` copies it
  on purpose. If a worktree got its own fresh `AUTH_SECRET`/`TALARIA_SECRET_KEY`,
  it could not decrypt the seeded secrets — and if it then wrote to a *shared* DB
  it would orphan them. (This is exactly what broke the env once. The fix:
  `talaria setup` now writes a dedicated, stable `TALARIA_SECRET_KEY`; keep it constant.)
- **The worktree's fleet is separate.** Fleet renders resolve to the worktree's
  own `fleet/` dir; a worktree does not manage your main agents.
- **Agent LLM calls** still flow through the *main* gateway if you're testing chat
  — the fleet agents point at whatever `LLM_BASE_URL` they were rendered with.
  Isolated worktrees are for app/UI iteration, not for re-rendering the live fleet.
- **Clean up** with the two teardown commands the script prints, from the primary
  checkout; `-v` drops the isolated volumes so nothing lingers. A worktree left past
  that is what [`bun talaria cleanup`](../.claude/skills/cleanup/SKILL.md) flags, and
  what the stop gate blocks on once it is stale. A `.talaria-keep` file in the
  worktree root opts it out.
- **Once the pull request merges, the stack stops itself.** The work is finished, so
  the sweep does not wait out the seven-day idle clock: the next stop gate in any
  checkout runs `docker compose down -v` on that worktree's stack, and the checkout
  becomes removable by `bun talaria cleanup --apply`. Nothing on disk is deleted
  without that explicit apply, and `talaria dev` restarts a stack that was stopped too
  early. This is why a worktree costs RAM and two containers only while its work is
  live — five stale stacks quietly running Postgres was the thing this fixed.
- **Each worktree builds into its own `api/target`**, which is what keeps two stacks
  from locking each other out of cargo. `mise.toml` puts every cargo behind a shared
  **sccache**, so the second worktree's build reuses the first's compiled dependencies
  instead of paying for the graph again. A side worktree's build dir untouched for 14
  days is reclaimed by `--apply`; the primary's is a warm cache the sweep only ever
  names.

## Manual worktrees

`talaria worktree` stamps `TALARIA_WORKTREE=<name>` in the worktree's `ui/.env`. If you
make a worktree by hand (`git worktree add …`) instead, that marker is absent, so
**`talaria dev` will refuse to start there** — because without its own stack it would
share the main DB. Either use `git wt <name>` / `bun talaria worktree`, or, if you know
what you're doing, give the worktree its own `DATABASE_URL`/`REDIS_URL` and add
`TALARIA_WORKTREE=<name>` to its `ui/.env` yourself.
