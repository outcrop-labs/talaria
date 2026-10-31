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
   on **deterministic per-slot ports** (app `53xx`, api `54xx`, Postgres `56xx`,
   Redis `65xx`), and reserves the slot's ports for what its api spawns and
   renders: the toolkit MCP (`57xx`), the omp auth bridge (`58xx`), and a
   hundred agent gateway ports from `20000 + slot·100`. Those three used to be
   one fixed number for every stack (`:5280`, `:5276`, and `:8770` upward), so
   the moment two stacks ran agents, the second api adopted the first stack's
   toolkit (its agents' tool calls answered `401`) and the second stack's first
   agent could not publish `:8770` (`port is already allocated`).
   Both containers wait, before anything reads them, until the real server
   answers a query against the `talaria` database over TCP (step 3 says why).

   They run on docker's default `bridge` network, so the project **creates no
   network of its own** ([`docker/worktree.compose.yml`](../docker/worktree.compose.yml)):
   each project network took a subnet from the daemon's ~30 predefined address
   pools, and with parallel worktrees, boxes and other projects on one host
   `compose up` failed with `all predefined address pools have been fully
   subnetted`. The two containers never talk to each other and are reached only
   through their loopback ports, so they need none. **The trade-off:** the default
   bridge is shared by every container on the host started without `--network`,
   and such a container can reach this Postgres (a copy of your main DB, password
   `talaria`) and this Redis (no auth) by container IP — on a project network only
   that project's containers could. Talaria's own containers all name a network;
   a hand-run `docker run` does not. A worktree cut before this keeps its old
   `talaria-wt-<name>_default` network until its next `talaria dev`, which moves
   both containers to the bridge and then removes that network once no
   container, running or stopped, still names it.
3. Seeds the new DB with `pg_dump` of your main DB (a snapshot — later changes on
   main don't propagate), then clears the copied agents' gateway ports so this
   stack's first render assigns its own rather than publishing onto main's. It
   counts main's tables first and says what the copy carried: `seeded (… N
   tables)`, or, when main's database has no tables at all, that there was nothing
   to copy and the worktree starts empty and migrates on its first `talaria dev`.
   The restore runs with `ON_ERROR_STOP`, so one failed statement fails the seed
   (psql otherwise prints it and exits 0), and a restore that comes up with fewer
   tables than main dies too, with the teardown (a copy whose own count cannot be
   read says so, rather than quote main's count as checked). Main's container is
   `talaria-postgres-dev`, or what `TALARIA_PG_CONTAINER` names — but one that
   belongs to a worktree's or a box's compose project is refused: `talaria dev` in
   a worktree sets that variable to the worktree's own Postgres, every process it
   starts inherits it, and a worktree cut from there copied the wrong stack. Both
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
6. Symlinks `ui/node_modules` to the **primary checkout's**, by its physical path
   (fast; no reinstall). The primary is found from git's common dir, which every
   linked worktree shares; when git cannot say, the invoking checkout's own
   `ui/node_modules`, resolved through every link, stands in. It used to link the
   invoking checkout's spelling, so a worktree cut from another worktree linked
   *that* worktree's link, and lost its toolchain the moment that worktree was
   removed. With nothing to link anywhere it warns, and `talaria dev` installs one.
   It also **installs**
   `cli/` and `mcp/` with `bun install --frozen-lockfile` from the branch's own
   lockfiles. Those two used to be missing in every fresh worktree, so its first
   `bun run gate` failed on a missing `tsc` before it read a line of the change;
   they are small, bun hardlinks from its cache, and a link would tie the branch
   to main's lockfile. (`bun run gate` installs any missing `node_modules` of a
   package it typechecks itself, too — never through a symlink.)

It prints the exact `talaria dev` command and the teardown steps.

**The setup is the invoking checkout's, not the base ref's.** `bun talaria worktree`
runs the CLI of the checkout you call it from, and the `talaria` shim always runs the
primary's, wherever you call it. That is usually the primary, on `main`, so
`talaria worktree <name> origin/rc` writes `ui/.env` and picks ports by `main`'s
rules, however far `rc` has moved. A stack cut that way on
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
- **Every command run in a worktree acts on that worktree's stack**
  ([`cli/src/stack.ts`](../cli/src/stack.ts)). `talaria reset`, `dev`, `backup` and
  `restore` take their targets from the worktree's own `ui/.env`: its
  `talaria-pg-<name>` postgres, its `DATABASE_URL`, and its agents in
  `talaria-wt-<name>-fleet`, which are picked by compose project label, never by a
  name prefix. `reset` prints those targets before it asks you to confirm. It also
  checks that the postgres it would exec into is the one `DATABASE_URL` names, by
  compose project, published port, database and role. Where the target would be a
  guess, the command refuses and says why:
  - a shell variable names another stack's container;
  - a shell variable overrides a line the worktree's `ui/.env` owns with another
    value: `DATABASE_URL`, `REDIS_URL`, `PORT`, `TALARIA_API_PORT`,
    `TALARIA_RUST_API_URL`, `TALARIA_GATEWAY_SELF_URL`, `TALARIA_FLEET_PROJECT`,
    the toolkit, omp-auth and gateway ports (`dev`), or `TALARIA_PG_PORT` /
    `TALARIA_REDIS_PORT` naming another port than the URLs. `talaria dev` lets the
    shell win, so a stale `DATABASE_URL` exported for main made the worktree's api
    a second server on main's database. The same value as `ui/.env`'s is fine;
  - the worktree has no `TALARIA_FLEET_PROJECT`, or one that is not
    `talaria-wt-<name>-fleet`, so its agents would render into another project
    (`dev` and `reset fleet` both refuse);
  - the rendered `fleet/docker-compose.yml` names another project;
  - `restore` with a `TALARIA_ENV_FILE` whose `DATABASE_URL` is not the worktree's,
    and no `--target`;
  - you run the installed `talaria` shim from inside a worktree. The shim always
    runs the primary's CLI, so it would act on the primary's stack. Use
    `bun talaria …` from the worktree.

  `talaria deploy` and `service` drive the production compose project, which no
  worktree owns. In a worktree they need `COMPOSE_PROJECT_NAME` set to a project of
  its own. `deploy up`, `down` and `update` need more: every key
  [`CONTAINER.md`](./CONTAINER.md) lists for a second instance on one host
  (`COMPOSE_PROJECT_NAME`, `TALARIA_STATE_DIR`, `TALARIA_HTTP_PORT`,
  `TALARIA_FLEET_PROJECT`, `TALARIA_FLEET_NETWORK`) set to a value of its own,
  none of them a `talaria-wt-*` name, or the deploy would share the production
  instance's state dir and fleet. `service install`/`uninstall` refuse there,
  because the unit is the host's only one. `restore` in a worktree writes nothing
  outside it. It writes no blobs into main's object storage (a worktree's app
  shares it), and no dir it inherited from main's env. Before this, `talaria reset`
  run in a worktree reset **main**: it exec'd into `talaria-postgres-dev` and
  removed every `talaria-fleet-agent*` container. In the primary, `reset fleet`
  refuses a `TALARIA_FLEET_PROJECT` that is a worktree's (`talaria-wt-*`).

  A worktree's `talaria dev` sets `TALARIA_PG_CONTAINER`, `TALARIA_REDIS_CONTAINER`
  and their two ports to its own, in its own process, so its app, its harness runs
  and any shell under them inherit them. The commands that mean **main's**
  containers on purpose refuse such a value: `talaria box new` and `box seed` (main
  is their seed source), and the primary's `talaria dev` and `reset`. A
  `TALARIA_*_CONTAINER` is refused when its name is a worktree's or a box's
  (`talaria-pg-<name>`, `talaria-redis-<name>`, `talaria-wt-*`, `devbox-*`), or when
  docker places the container in such a compose project. The same shell carries the
  worktree's `ui/.env` lines as well (its vite loads the file into its environment,
  and its api gets them too), and the primary lets the shell win. So the primary's
  `talaria dev` also refuses a shell `TALARIA_WORKTREE` and a `talaria-wt-*`
  `TALARIA_FLEET_PROJECT`, as `reset` does. Every one of these refusals prints the
  `env -u …` command to run instead. It names each worktree line the shell sets
  (`TALARIA_WORKTREE`, `DATABASE_URL`, `REDIS_URL`, the ports, the api url, the
  fleet project), not only the container names, since any one left would start
  main's app on the worktree's stack. Before this, `box new` from such a shell seeded
  the box from that worktree, and the primary compose `up` it runs would recreate
  main's postgres under the worktree's name, or remove it on the name conflict.
- **Clean up** with the two teardown commands the script prints, from the primary
  checkout; `-v` drops the isolated volumes so nothing lingers. A worktree left past
  that is what [`bun talaria cleanup`](../.claude/skills/cleanup/SKILL.md) flags, and
  what the stop gate blocks on once it is stale. A `.talaria-keep` file in the
  worktree root opts it out.
- **Networks left behind are swept too.** A worktree cut before its stack moved to the
  default bridge, an interrupted `up`, or a stack whose containers were removed by
  hand can leave a `talaria-*` compose network with nothing on it, still holding its
  subnet; a box can leave its `devbox-*` ones the same way. `bun talaria cleanup` lists
  each one whose project has no container left and no live worktree or box (and is at
  least ten minutes old, so it never races an `up` in flight); `--apply` removes them.
  The shared `talaria` network and anything outside a `talaria-*` or `devbox-*` project
  are never touched. When `compose up` does fail on the pool error, the CLI says what
  it means and points here. An app's own database takes no network either: in dev it
  runs on the default bridge and publishes a loopback port
  ([`APPS.md`](./APPS.md)).
- **So are app databases left behind.** An app database (`talaria-appdb-*`) runs on
  `restart: unless-stopped`, so a worktree removed while its app ran leaves it running.
  `bun talaria cleanup` lists one whose cluster is mounted from a sibling
  `talaria-<name>` directory that is gone; `--apply` removes the container and leaves
  the data directory. A directory still on disk keeps its databases, whether or not
  git still lists it, and one mounted from anywhere else is never touched.
- **The app's port is strict.** `talaria dev` hands vite this worktree's `PORT` with
  `--strictPort` (the primary's `5273` too), and checks the port before anything
  starts: a port already taken stops it with the holder's pid and command (or the
  container that publishes it). vite used to move to the next port and carry on, while
  `TALARIA_GATEWAY_SELF_URL` and every other URL in `ui/.env` kept naming the old one,
  so the stack's agents could not reach the app they belonged to.
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
