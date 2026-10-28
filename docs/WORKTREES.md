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
   — on docker's default `bridge` network, so the project **creates no network of
   its own** ([`docker/worktree.compose.yml`](../docker/worktree.compose.yml)): each
   project network took a subnet from the daemon's ~30 predefined address pools,
   and with parallel worktrees, boxes and other projects on one host `compose up`
   failed with `all predefined address pools have been fully subnetted`. The two
   containers never talk to each other and are reached only through their loopback
   ports, so they need none — and they wait, before anything reads them, until the
   real server answers a query against the `talaria` database over TCP (step 3
   says why) — on **deterministic per-slot ports** (app `53xx`, api `54xx`, Postgres `56xx`,
   Redis `65xx`), and reserves the slot's ports for what its api spawns and
   renders: the toolkit MCP (`57xx`), the omp auth bridge (`58xx`), and a
   hundred agent gateway ports from `20000 + slot·100`. Those three used to be
   one fixed number for every stack (`:5280`, `:5276`, and `:8770` upward), so
   the moment two stacks ran agents, the second api adopted the first stack's
   toolkit (its agents' tool calls answered `401`) and the second stack's first
   agent could not publish `:8770` (`port is already allocated`).
3. Seeds the new DB with `pg_dump` of your main DB (a snapshot — later changes on
   main don't propagate), then clears the copied agents' gateway ports so this
   stack's first render assigns its own rather than publishing onto main's. Both
   ends of the copy are waited for with **a real query over TCP**
   (`psql -h 127.0.0.1 … -c 'select 1'` in the container), not `pg_isready`:
   the container existing is not the same as the database existing, and a
   worktree created moments after `talaria dev` used to die mid-way with
   `FATAL: database "talaria" does not exist`. The worktree's own Postgres is
   the sharper case, because its volume is always fresh: the image's entrypoint
   first runs a **temporary, socket-only server** to create the database, then
   shuts it down and starts the real one. A socket `pg_isready` passed against
   that temporary server — and `pg_isready -d` says "accepting connections" for a
   database that does not exist at all — so the seed met `the database system is
   shutting down`. The temporary server has no TCP listener, so the query cannot
   pass until the real one serves the real database. A wait that runs out stops
   the command there, before the seed, and prints the teardown.
4. Writes the worktree's `ui/.env`: its own `DATABASE_URL`/`REDIS_URL`/`PORT`,
   its own api port **and the url the app's proxy dials**
   (`TALARIA_API_PORT`, `TALARIA_RUST_API_URL` — `talaria dev` only derives the
   url when nothing names it, and main's env names it, hardcoded to `:5274` by
   `talaria setup`), its fleet's callback url and compose project
   (`TALARIA_GATEWAY_SELF_URL`, `TALARIA_FLEET_PROJECT`), its toolkit,
   omp-auth and gateway ports (`TALARIA_MCP_PORT`, `TALARIA_OMP_AUTH_PORT`,
   `TALARIA_GATEWAY_PORT_BASE`, lifted into the api by `talaria dev`), and the
   `TALARIA_WORKTREE` marker — **copying the encryption root key**
   (`TALARIA_SECRET_KEY`, or `AUTH_SECRET` if that's what you use) so the seeded
   secrets decrypt. This is the one thing shared, and it's a read-only root — see
   below. Every key that block writes is stripped from the copy of main's env
   first (`WORKTREE_ENV_KEYS` in `cli/src/cmd/worktree.ts`, derived from what is
   written rather than kept by hand): `envValue` takes the *first* match, so a
   leftover line would shadow the one written here. That is exactly how a
   worktree once answered every `/api/*` with `upstream unreachable` — its own
   api healthy on `:54xx`, its app still dialling main's `:5274`.
5. Seeds the fleet config plane the way `talaria setup` seeds the primary's:
   `fleet/skills`, `fleet/hooks`, `fleet/agents`, **`fleet/chassis.yml` from
   the worktree's own `scripts/chassis.template.yml`** and a blank `fleet/.env`.
   `fleet/` is gitignored, so a fresh checkout has none of it — and without the
   chassis the first fleet render 500s with `fleet chassis missing`. The
   template is the base ref's, so the chassis matches the renderer the worktree
   runs; a base ref without one falls back to the invoking checkout's. An
   existing chassis is kept; it is your config.
6. Symlinks `ui/node_modules` from main (fast; no reinstall), and **installs**
   `cli/` and `mcp/` with `bun install --frozen-lockfile` from the branch's own
   lockfiles. Those two used to be missing in every fresh worktree, so its first
   `bun run gate` failed on a missing `tsc` before it read a line of the change;
   they are small, bun hardlinks from its cache, and a link would tie the branch
   to main's lockfile. (`bun run gate` installs any missing `node_modules` of a
   package it typechecks itself, too — never through a symlink.)

It prints the exact `talaria dev` command and the teardown steps.

**The setup is the invoking checkout's, not the base ref's.** `bun talaria worktree`
and the `talaria` shim run the CLI of the checkout you call them from — usually the
primary, on `main` — so `talaria worktree <name> origin/rc` writes `ui/.env` and
picks ports by `main`'s rules, however far `rc` has moved. A stack cut that way on
2026-10-22 got no `TALARIA_FLEET_PROJECT`, because the primary's CLI predated it.
When the running `cli/src/cmd/worktree.ts` differs from the base ref's, the command
ends with a warning that says so: bring the invoking checkout up to the base ref and
recut, or check the new `ui/.env` against step 4.

## Rules of the road

- **Never regenerate the encryption root in a worktree.** `talaria worktree` copies it
  on purpose. If a worktree got its own fresh `AUTH_SECRET`/`TALARIA_SECRET_KEY`,
  it could not decrypt the seeded secrets — and if it then wrote to a *shared* DB
  it would orphan them. (This is exactly what broke the env once. The fix:
  `talaria setup` now writes a dedicated, stable `TALARIA_SECRET_KEY`; keep it constant.)
- **The worktree's fleet is separate, and it runs.** Fleet renders resolve to the
  worktree's own `fleet/` dir (seeded by `talaria worktree` the way `talaria setup`
  seeds the primary's), its agents run under their own compose project
  (`talaria-wt-<name>-fleet`), and they call back to *this* app's port — the
  worktree's gateway for their thinking, its MCP gateway for their tools
  (`TALARIA_GATEWAY_SELF_URL`, `TALARIA_FLEET_PROJECT` and `PORT` in `ui/.env`,
  lifted into the api by `talaria dev`). A worktree never manages your main
  agents, and the cleanup sweep knows a worktree's fleet project is the
  worktree's own. Hiring an agent still needs the Hermes image on the host.
- **Clean up** with the two teardown commands the script prints, from the primary
  checkout; `-v` drops the isolated volumes so nothing lingers. A worktree left past
  that is what [`bun talaria cleanup`](../.claude/skills/cleanup/SKILL.md) flags, and
  what the stop gate blocks on once it is stale. A `.talaria-keep` file in the
  worktree root opts it out.
- **Networks left behind are swept too.** A worktree cut before its stack moved to the
  default bridge, an interrupted `up`, or a stack whose containers were removed by
  hand can leave a `talaria-*` compose network with nothing on it, still holding its
  subnet. `bun talaria cleanup` lists each one whose project has no container left and
  no live worktree (and is at least ten minutes old, so it never races an `up` in
  flight); `--apply` removes them. The shared `talaria` network and anything outside a
  `talaria-*` project are never touched. When `compose up` does fail on the pool
  error, the CLI says what it means and points here.
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
