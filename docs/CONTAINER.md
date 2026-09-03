# Running Talaria as a container

One app image, one compose file, one command:

```bash
git clone https://github.com/outcrop-labs/talaria && cd talaria
DOCKER_GID=$(stat -c %g /var/run/docker.sock) \
  docker compose -f docker/compose.yml up -d --build
```

The stack comes up with zero configuration: secrets the environment doesn't
supply are generated on first boot, the admin credentials print once in the
logs, and the app answers on `http://localhost:5273`.

```bash
docker compose -f docker/compose.yml logs talaria | grep -A2 'Sign in'
```

This is the deploy for a Docker host you point at once and drive afterwards
through an orchestrator — [Dokploy](#dokploy-the-control-plane), Portainer,
plain compose, anything that can run a compose file and set environment
variables. (An earlier golden-image flow — a full VM per instance for
Proxmox-first environments — has been retired; the container deploy is the
self-hosting path.)

One habit worth forming immediately: any variable you override on the command
line (`DOCKER_GID`, `TALARIA_STATE_DIR`, `TALARIA_HTTP_PORT`, …) belongs in a
`docker/.env` file — compose loads it automatically. Interpolation happens on
*every* `up`; a later `docker compose up -d` from a shell without those exports
silently re-interpolates the defaults, republishing the default port and
remounting the default state dir on a running instance.

## The api binary arrives pre-built

The image build compiles the UI and the toolkit (bun, in the `build` stage)
but never the Rust api. That binary is published as its own package image —
`ghcr.io/outcrop-labs/talaria-api`, built in CI on every api-touching push
to main (`api/package.Dockerfile` is its build) — and the Dockerfile consumes
it with a plain `COPY --from`. So nothing that builds the app image, this
host included, needs a Rust toolchain, a C compiler, or the minutes the TLS
stack takes to compile; `docker build .` stays docker-and-network only.

The default package tag is `main`, which tracks the source a checkout build
happens against (the same ref Dokploy builds). Release builds pin the exact
`sha-<sha12>`-tagged package they were built with, so a published image
names the api bits it carries.

Changed `api/` yourself? Build the package locally and point the app build at
it — the compile stays out of the app image either way:

```bash
docker build --network=host -f api/package.Dockerfile -t talaria-api:local .
docker build --build-arg TALARIA_API_IMAGE=talaria-api:local .
```

## The `talaria` CLI (optional convenience)

Every command in this doc is plain docker compose on purpose — no lock-in,
nothing to uninstall. The repo's [`talaria` CLI](../cli) wraps exactly those
commands for a checkout-driven host (a checkout is required either way, since
the image builds from the repo; bun is the only extra prerequisite):

```bash
bun talaria deploy up       # the one command at the top of this page
bun talaria deploy update   # git pull --ff-only, then the same up -d --build
bun talaria deploy down     # stop (--volumes also deletes the data — careful)
bun talaria deploy logs     # follow every service's logs
bun talaria deploy creds    # the 'Sign in' block from the logs
bun talaria deploy status   # effective port/state/fleet + compose ps
bun talaria service install # start the stack + keep it running across reboots
```

Three things the wrappers add beyond typing the compose command yourself:

- **Every command prints the exact `docker compose …` it is about to run,
  before running it** — the output doubles as the copy-pasteable equivalent,
  so nothing the CLI does is hidden and moving off it costs nothing.
- **`DOCKER_GID` is resolved from the socket automatically** (the `stat -c %g`
  half of the documented command). A value you exported, or one already in
  `docker/.env`, always wins — the CLI only fills the gap.
- **The drift trap above is detected**: any variable the compose file
  interpolates that is exported in your shell but absent from `docker/.env`
  gets a loud warning before anything runs. The CLI refuses to be the `up`
  that silently strands your overrides.

On a host without bun, build the CLI once from a machine that has it — it
compiles to a standalone binary:

```bash
bun build --compile cli/bin/talaria.ts --outfile talaria
./talaria deploy up
```

Plain compose remains the canonical path; the CLI is a convenience that
always shows its work.

## Keep it running across reboots

Every service in the compose file carries `restart: unless-stopped`, so once
the *docker daemon* is back, the containers come back on their own. What that
doesn't give you is boot ordering, a health-gated start, a clean shutdown
before the daemon stops, and one place to ask "is my instance up". On a
Linux host, one command adds all of it:

```bash
bun talaria service install
```

It refuses to hide anything: it prints every command it runs, and the three
privileged steps it needs sudo for (`install` the unit into
`/etc/systemd/system`, `systemctl daemon-reload`, `systemctl enable --now`)
run one at a time with the password prompt on your terminal. What it does,
in order:

1. Guards: Linux, systemd actually running, docker and the compose plugin,
   a checkout with `docker/compose.yml`.
2. Pins `DOCKER_GID` into `docker/.env` — the boot unit has no shell to
   resolve it from, and compose interpolates from that file on every `up`.
   A value already in the file (or exported) wins; nothing is overwritten.
3. Starts the stack if it isn't already running — the same `up -d --build`
   as `deploy up`, so a first boot's build failures land in your terminal,
   not in journald.
4. Writes and enables the unit below. On re-install it overwrites in place.

```ini
# /etc/systemd/system/talaria.service — installed by `talaria service install`.
[Unit]
Description=Talaria (docker compose stack)
Requires=docker.service
After=docker.service network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=<your checkout>
ExecStart=/usr/bin/docker compose -f docker/compose.yml up -d --wait
ExecStop=/usr/bin/docker compose -f docker/compose.yml down
TimeoutStartSec=20min
Restart=on-failure
RestartSec=10s

[Install]
WantedBy=multi-user.target
```

The unit is only the boot/stop handle — `ExecStart` has no `--build` on
purpose (boot starts what exists; `talaria deploy update` rebuilds), and
`--wait` is dropped automatically on compose builds too old to gate on a
one-shot service. `systemctl status talaria` and `journalctl -u talaria`
are the supervision view; `talaria service status` shows both that and the
compose view.

- **Updates** stay checkout-driven: `talaria deploy update` (pull +
  rebuild + recreate). The unit starts whatever exists; no restart needed.
- **`talaria service uninstall`** stops the stack (its ExecStop is
  `compose down`) and removes the unit — named volumes and the state dir
  are kept, the same blast radius as `deploy down`. Bring it back with
  `deploy up`.
- **Two instances on one host** need separate compose projects (see
  [Networking](#networking)) and, by hand, a renamed unit — the CLI
  installs exactly `talaria.service`.
- **Orchestrator-driven hosts don't need it.** When Dokploy (or any other
  orchestrator) runs the stack, it owns the lifecycle — reboots are
  already covered by the daemon starting at boot and the orchestrator
  bringing the stack back (swarm reconciliation, restart policies).
  `service install` is for the checkout-driven host the command at the
  top of this page describes.
- **Hosts without systemd** (macOS, Windows, containers-as-hosts) keep the
  plain path: enable docker at boot and the restart policies carry the
  stack.

## The pieces

| File | What |
|---|---|
| [`Dockerfile`](../Dockerfile) (repo root) | Multi-stage build: toolchain → pruned prod deps → a bun + docker-cli + git runtime. No config inside. |
| [`docker/compose.yml`](../docker/compose.yml) | The instance: the app plus postgres, redis, qdrant, embeddings (TEI), minio, searxng. |
| [`docker/entrypoint.sh`](../docker/entrypoint.sh) | Secrets bootstrap, fleet config seeds, dependency gate — the container's first boot. |
| [`docker/await-deps.mjs`](../docker/await-deps.mjs) | Waits for postgres + redis before the server starts. |

The app container is deliberately *not* the whole product. Talaria spawns
agent containers through the host's docker daemon (the fleet), and its
stateful sidecars are separate services — so the unit of deployment is this
compose file, with the app image as its centerpiece.

## The env-var contract

The environment is the **only** config channel. There is no `ui/.env` in the
image, and real env wins everywhere it exists
([`server-entry.js`](../ui/server-entry.js) skips `.env` keys already in
`process.env`; the entrypoint follows the same rule against its generated
file). Whatever your orchestrator sets is what runs.

Set through compose interpolation (`${VAR:-default}` — override by exporting
`VAR`, or a `--env-file`):

| Var | Default | What |
|---|---|---|
| `TALARIA_HTTP_PORT` | `5273` | Host port for the UI |
| `TALARIA_STATE_DIR` | `/var/lib/talaria` | Host path of the state bind — mounted at the *same* path inside the container, whatever it is (see [State](#state--backups)) |
| `DOCKER_GID` | `999` | Group of `/var/run/docker.sock` on the host — `stat -c %g` it |
| `POSTGRES_PASSWORD` | `talaria` | Postgres + the `DATABASE_URL` default (random on first `talaria deploy up` — see below) |
| `TALARIA_S3_ACCESS_KEY` / `TALARIA_S3_SECRET_KEY` | `talaria` / `talaria-container-secret` | App + minio share the pair (random on first `talaria deploy up` — see below) |
| `TALARIA_S3_BUCKET` | `talaria` | Built-in object storage bucket |
| `TALARIA_EMBED_MODEL` | `BAAI/bge-small-en-v1.5` | The TEI model (changing it means re-backfilling) |
| `TALARIA_FLEET_NETWORK` | `talaria` | Shared fleet network name (per-instance on shared hosts) |
| `TALARIA_DNS_1` / `TALARIA_DNS_2` | `1.1.1.1` / `1.0.0.1` | SearXNG's upstream resolvers |
| `TALARIA_PG_POOL_MAX` | `40` | The api's postgres pool ceiling (postgres sidecar runs default `max_connections=100`; api + ui + migration ≈ 61) |
| `TALARIA_PG_ACQUIRE_TIMEOUT_MS` | `15000` | How long a db-bound request queues before failing — bursts wait instead of 500ing |
| `TALARIA_UI_PG_POOL_MAX` | `20` | The UI's own postgres pool ceiling |

Generated by `talaria deploy up` into `docker/.env` **only when the
environment doesn't supply them** (written once, 0600, git-ignored; real env
and existing file lines always win). These are the ones compose interpolates
into **both the app and a sidecar**, so no container's entrypoint can
generate them — interpolation happens at container-create time, before any
entrypoint runs — which is why the wrapper, the one thing that runs *before*
compose, owns them:

| Var | Role |
|---|---|
| `POSTGRES_PASSWORD` | Postgres + the `DATABASE_URL` default. **Pinned by initdb**: an existing cluster keeps the password it was born with — `deploy up` detects its volume and pins the current value (with a rotation note) instead of minting a random the data has never heard of |
| `TALARIA_S3_ACCESS_KEY` / `TALARIA_S3_SECRET_KEY` | The app + minio root pair. Minio takes root from the env at every boot, so these rotate freely |

Generated by the entrypoint **only when the environment doesn't supply them**
(persisted in `/var/lib/talaria/env/generated.env`, real env always wins):

| Var | Role |
|---|---|
| `TALARIA_SECRET_KEY` | The encryption root — never rotates; back it up with the DB ([`ENCRYPTION.md`](./ENCRYPTION.md)) |
| `AUTH_SECRET` | Session signing — rotatable |
| `SEARXNG_SECRET` | Rendered into SearXNG's settings file |
| `TALARIA_AGENT_KEY` | The app's hop to the toolkit service |

No admin account is generated. The first boot boots empty on purpose: open the app and
**claim** the instance — the account you create at the claim screen is the admin.

Everything else passes straight through to the app — the full inventory is
[`ui/.env.example`](../ui/.env.example): Google OAuth (`AUTH_GOOGLE_*`),
allowed domains/emails, fetch policy, and the TLS trio below. Prefer
`talaria deploy up` over a bare `docker compose up`: without the generated
`docker/.env` a compose instance runs on the published dev defaults — reach
the claim screen over a private network or TLS first, since whoever claims a
fresh instance becomes its admin.

## Networking

Two docker networks, one socket — this is the "right shape"
[`AGENT-NETWORKING.md`](./AGENT-NETWORKING.md) describes, implemented:

```
 ┌─ internal network ────────────────────────────────┐
 │  talaria ←→ postgres · redis · qdrant ·           │
 │            embeddings · minio · searxng           │
 └───────┬───────────────────────────────────────────┘
         │ (app also on ↓; sidecars are NOT)
 ┌─ talaria network (shared with the fleet) ─────────┐
 │  talaria ⇄ agent containers (spawned siblings)    │
 └───────────────────────────────────────────────────┘
         │ /var/run/docker.sock (mounted, read-write)
         ▼
    host docker daemon ── runs the fleet's compose project
```

- **Agents → app**: the fleet's rendered configs point at
  `TALARIA_MCP_GW_URL` / `TALARIA_GATEWAY_SELF_URL`, which the compose sets to
  the app's service name (`http://talaria:5273/...`). Container→container on
  the shared network — **no host firewall rule needed**, which is the entire
  point: the dev/host install needs an INPUT-chain rule for exactly this hop.
- **App → agents**: `TALARIA_AGENT_DIAL=container` makes the fleet manifest
  ([`api/src/fleet/render.rs`](../api/src/fleet/render.rs)) dial agents by their
  compose service names (`agent-<dept>:8642`, slot-aware) instead of the
  host-loopback ports the dev stack publishes — and in this mode the renderer
  doesn't publish those ports at all (the manifest never dials them, and
  installs sharing a host would collide on the identical allocations).
  Memory, media and crons go through `docker exec` and never touched the
  network anyway.
- **Isolation**: only the app joins the fleet network. Agent containers are
  model-driven and semi-trusted; they can reach the app, never postgres,
  redis, or minio.
- **Preflight** follows the renderer (api/src/fleet/preflight.rs derives its probe
  target from `MCP_GW_BASE`), so it probes the name agents actually use.

The `talaria` network is compose-managed with a fixed name so a fresh host
works on the first `up`; the fleet renderer adopts it as its external network.
Running **more than one instance per host**: each needs its own
`COMPOSE_PROJECT_NAME`, `TALARIA_STATE_DIR`, `TALARIA_HTTP_PORT`, *and*
`TALARIA_FLEET_PROJECT` + `TALARIA_FLEET_NETWORK` — the fleet's compose
project and network are how agents get container names and DNS aliases, so
two instances sharing either would reconcile each other's agents into
oblivion — plus a matching `network.name` in that instance's
`fleet/chassis.yml` — or docker DNS will happily round-robin two apps sharing
one alias.

## Dokploy: the control plane

The compose file is Dokploy-shaped on purpose (it's also just a compose file
— Dokploy is a consumer, not a dependency):

1. **Create a Compose resource** pointing at this repo. Dokploy builds the
   image from the root Dockerfile at deploy time and runs
   `docker/compose.yml`.
2. **Set environment** in the resource's panel (or through the REST API —
   that surface, `x-api-key`-authenticated project/compose/env endpoints, is
   what an orchestration agent drives): the contract table above plus
   `TALARIA_SECRET_KEY` (pin it before first boot if you ever want to restore
   a backup onto a rebuilt instance) and `TALARIA_S3_SECRET_KEY`.
3. **Domain**: uncomment the Traefik labels block in the compose, add the
   dokploy network to the app service, and set the TLS trio:
   `COOKIE_SECURE=1`, `AUTH_PUBLIC_URL=https://your-domain`,
   `TALARIA_TRUST_PROXY=1` (plain http is the default posture — browsers
   refuse `Secure` cookies over it, so login would silently break).
4. **Redeploys** are API calls; the app itself never updates in place
   (`TALARIA_UPDATER=off` is baked — the updater assumes a git checkout the
   image doesn't have, and the orchestrator owns this job now).

`DOCKER_GID` still applies: `docker compose exec talaria docker ps` inside the
resource's terminal should list host containers. Multi-instance hosts see the
note above about per-instance networks. Reboots are covered on this path —
the daemon enabled at boot plus the stack's restart policies bring everything
back; the [systemd unit](#keep-it-running-across-reboots) is for
checkout-driven hosts.

## Security posture

- **The docker socket is host root.** The app container mounts
  `/var/run/docker.sock` because the fleet drives the host daemon — that is
  the documented tradeoff ([`AGENT-NETWORKING.md`](./AGENT-NETWORKING.md)),
  accepted for v1 — the same grant a docker-group app user would have. The
  narrowing path, when you want it: a socket
  proxy (e.g. tecnativa/docker-socket-proxy) allowing only the endpoints the
  fleet touches (containers, images, networks, volumes), placed between the
  app and the socket.
- The app container otherwise runs unprivileged: uid 10001, no capabilities
  beyond its groups, `no-new-privileges`.
- Sidecar ports are published **nowhere**. The dev stack published each to
  `127.0.0.1` because the app ran on the host; here in-container DNS replaces
  publishing entirely, and several sidecars (qdrant, TEI, searxng) have no
  authentication at all — the network is the boundary.
- The only published port is the app itself (`TALARIA_HTTP_PORT`). Put TLS in
  front of it for anything beyond a trusted LAN (see the Dokploy section).

## State & backups

| Path | What | Back up |
|---|---|---|
| `$TALARIA_STATE_DIR` (bind, default `/var/lib/talaria`) | uploads, fleet render + agent state, `env/generated.env`, seeded chassis | **yes** — with the DB, and `generated.env`/`TALARIA_SECRET_KEY` also somewhere a snapshot of this host isn't |
| `pg-data` (volume) | the database | **yes** |
| `qdrant-data`, `minio-data` (volumes) | KB vectors, stored files | yes |
| `redis-data` (volume) | sessions/queues | no (transient) |
| `tei-cache` (volume) | the embeddings model | no (re-pulls) |

The state dir is a **host bind mounted at the same path inside the container**
— not a named volume — because the fleet renderer bakes absolute host paths
into agent bind mounts and the *host* daemon resolves them; a volume would be
invisible to every agent container. On first boot the init service (running as
root) creates and chowns the tree to the app's uid; if you ever see the
entrypoint's ownership error, the fix is the `mkdir` + `chown 10001:10001` it
prints.

`bun talaria backup` / `bun talaria restore` and [`BACKUPS.md`](./BACKUPS.md)
cover the database side. The rule that ruins days if skipped: a restored
database without the `TALARIA_SECRET_KEY` it was sealed with cannot read its
own secrets ([`ENCRYPTION.md`](./ENCRYPTION.md)).

## Updating and installing apps

Updates are a redeploy — `docker compose ... up -d --build`,
`bun talaria deploy update` on a checkout host (it pulls with `--ff-only`
first and prints each command it runs), or an API call
through the orchestrator. Migrations run as the server boots (expect a minute
of downtime on schema changes; the health check covers the window — a FAILED
pass reports `migrations: !ok` on `/api/healthz` and flips the container
unhealthy, instead of a green container whose table queries all 500).

**There is no manual post-deploy step, ever.** Every schema change and every
one-time data operation (a backfill, a watermark reset, a repair) ships as an
appended statement in the `MIGRATIONS` array (`ui/src/server/db/pg.ts`) and
applies itself on the next boot, exactly once per database, under an advisory
lock. A fix that lived only in someone's shell history did not ship: the next
fresh install won't have it. Two gates keep a bad migration out: CI
(`migrations.yml`) replays the whole array against a scratch
`postgres:16-alpine` and diffs the resulting schema against the committed
snapshot (`ui/src/server/db/schema.snapshot.sql` — regenerate with
`cd ui && bun run migrations:snapshot`; the diff in the PR is the schema
change, in review form), and the invariant check refuses any statement that
can destroy or rewrite data (drop table, truncate, unscoped delete/update, …)
unless it carries an inline `-- deliberate: <why>` comment.

Installing an app from the marketplace clones it into
`/var/lib/talaria/apps/`, but apps *compile into the image* at build time
([`APPS.md`](./APPS.md)) — an install reports `pendingBuild` until a rebuild
happens. In container mode that rebuild is a redeploy with the app present in
the build context (a checkout's `apps/` — local apps included — or a fork that
vendors it), which is exactly the orchestrator's job. Nothing breaks in the
meantime; the app simply isn't live yet.

One caveat for exotic setups: chassis mounts (workbench profiles, plugins)
reference **host** paths — the rendered fleet compose resolves them on the
host, so anything an agent mounts must live somewhere host-visible, not inside
the app container's filesystem.

## Prebuilt images

The path above builds from a checkout — zero prerequisites beyond git and
Docker, and still the default. Prebuilt images are the alternative: every
channel this repo publishes lands on `ghcr.io/outcrop-labs/talaria`.

| Image tag | What it is | How it moves |
|---|---|---|
| `main` | whatever main shipped last | every app-touching push to main |
| `sha-<sha12>` | one commit on main, frozen | with that push; never rewritten |
| `nightly` | testing branch, built daily 03:17 UTC | automatic, nightly |
| `nightly-YYYYMMDD` | that day's nightly, frozen | automatic, daily; never rewritten |
| `X.Y.Z-rc.N` | a release candidate | a `vX.Y.Z-rc.N` tag on `rc` |
| `rc` | whatever RC was cut last | with each RC |
| `X.Y.Z` | a stable release | a `vX.Y.Z` tag |
| `latest` | whatever stable shipped last | with each stable release |

Pin anything you care about to the dated/versioned column — the moving tags
are pointers, rewritten by design.

The `main`/`sha` pair is the trunk feed, built by its own workflow
(`.github/workflows/app-image.yml`) on every push to main that touches
running bits — the release channels below it are cut by humans, the trunk
never stops. A trunk image is always a matched pair: the build resolves the
api package digest first (that commit's `sha-<sha12>` when the push touched
the api, the digest `talaria-api:main` names otherwise), so main's image
never carries this commit's UI on an older api.

The api binary rides inside these images but is also its own package,
`ghcr.io/outcrop-labs/talaria-api`, carrying the same channel tags plus an
immutable `sha-<sha12>` per commit — that sha tag is what a release build
actually consumes (see [The api binary arrives pre-built](#the-api-binary-arrives-pre-built)).

Running one is an override file layered on the base compose (the base stays
checkout-build; this never edits it):

```bash
docker compose -f docker/compose.yml -f docker/compose.registry.yml pull talaria searxng-config
docker compose -f docker/compose.yml -f docker/compose.registry.yml up -d --no-build
```

Pin the channel in `docker/.env` (`TALARIA_CHANNEL=0.2.0-rc.1`; unset means
`nightly`). The `pull` step is deliberate: a registry problem should report
as a pull failure, not as a boot timeout. `pull_policy: always` in the
override makes the registry the only image source even if a checkout is
present. Updating is the same two commands again.

`docker inspect` on a pulled image answers "what exactly is this" without a
checkout: the OCI labels carry the version, the revision, and the build time.

How the tags get made, how to cut an RC, and the branch model behind them:
[`RELEASING.md`](../RELEASING.md) (repo root). The `bun talaria deploy`
wrappers stay checkout-build — this path is plain compose.

## Troubleshooting

**Build fails with `no such package` on every apk package, or `bun install`
can't resolve registry.npmjs.org** — the host's `/etc/resolv.conf` names a
loopback stub resolver (systemd-resolved, Tailscale MagicDNS), which docker's
embedded DNS forwarder can't use; build containers then have no working
external DNS. Same failure the agent chassis and searxng guard against with
explicit `dns:` (this compose pins the app's resolvers for the same reason —
service names still resolve either way). For builds: `docker build
--network=host .` uses the host's resolver directly, or set `"dns":
["1.1.1.1"]` in `/etc/docker/daemon.json` to fix every container at once.
