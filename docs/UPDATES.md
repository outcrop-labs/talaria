# Updates — how a Talaria install rolls itself

An update to Talaria never cuts the live container out and reboots it. It **rolls**:
the engine pulls the new image, starts a second app container beside the live one,
waits for that container to pass the compose healthcheck, moves traffic to it, and
only then drains and stops the old one. The interruption window is the drain, not
the deploy. This page is the whole story: who may roll, what gets pulled, the
sequence, the escape hatches, and the rules a contributor must keep.

The engine lives in `api/src/update/` (Rust) and answers at
`/api/admin/updates` — the admin panel's **Admin → Security → Updates** is its face.

## Dormant until adopted — the invariant

The engine ships **dormant on every install**. Acting verbs (`apply`, `rollback`)
refuse with a plain sentence until an instance has been *adopted* — a one-time,
admin-triggered move onto the updater-owned container slots. An install that never
adopts keeps deploying exactly the way it always has (dokploy, your own pipeline,
`docker compose up` on a VM); nothing about shipping this engine changes any
running deployment. Adoption is per-instance and opt-in by design.
[The handover itself](#adoption--the-one-time-handover) is below.

## Adoption — the one-time handover

Adoption moves an install from "the orchestrator deploys me" to "I deploy
myself" (`api/src/update/adopt.rs`). It pulls the image the install should
be (the registry's newest, or a re-pin of the running image's own digest when
the registry is silent), renders the updater-owned project **from the live
container's own inspect document**, brings slot A up behind the same health
gate a roll uses, moves the `talaria` alias, and records the handover
crash-safe — `migrated`, the pin, the edge's port, the retired container's
name — **before any port changes hands**. Then the one decision an operator
makes: which port the edge takes.

- **A fresh port** (`edgePort` in the POST — talaria-infra's migration path,
  and any self-hoster whose proxy can repoint): the edge binds the new port
  immediately while the old container **keeps serving its own**. The operator
  repoints the proxy at the edge; the second `adopt` call — which can only
  arrive *through the edge*, which is itself the proof the proxy moved —
  stops the old container and lands the run. **Zero interruption at the
  origin.**
- **Inherit the app's port** (the panel's button, no `edgePort`): the edge
  must own the port the old container holds, so the old one releases it
  first — a **one-time cut of a few seconds**, named in the panel's confirm
  dialog. A dying container cannot run code after its own stop, so it arms a
  tiny host-side helper container (the app image itself — docker-cli and
  compose are baked in) that polls once a second and raises the edge the
  moment the port frees. Green's boot reconcile is the crash fallback, and
  the docker runbook below is the last resort.

The handover is **resumable at every step**: `adopt` on an already-adopted
install is the *resume* — it re-answers `edge-ready`, re-arms the helper, or
finishes the cutover, depending on where the first attempt died. The retired
orchestrator container is stopped by the finish and removed by the reconcile,
so nothing resurrects it — **disarm dokploy's autoDeploy on the resource
before adopting**, or the next push stop-first-deploys underneath the
handover. And after adoption the orchestrator no longer owns the app:
environment changes go through the slot env files in the update dir
(`$TALARIA_UPDATE_DIR`), not through dokploy.

## The taxonomy — who may roll

The mode is resolved fresh from the environment on every call
(`api/src/update/mode.rs`), in trust order:

| Mode | Signal | May roll? |
|---|---|---|
| `image` | the image itself sets `TALARIA_INSTALL=image` (only the published Dockerfile does) | yes — this is the updater's domain |
| `checkout` | a git checkout under `bun server-entry.js` (`TALARIA_RUNTIME=prod-server`, no image signal) | no — the orchestrator that owns the checkout deploys it |
| `dev` | vite dev, or anything unstamped | no — dev reloads on file change; an update button would be a lie |
| `off` | `TALARIA_UPDATER=off` | no — the kill switch deployments that supervise the process themselves have always had |

The old git-checkout updater (pull, build into `dist-next`, restart the bun
process through `scripts/update-restart.mjs`) is retired. Checkout installs were
already deploying through their orchestrator; the panel now says so instead of
offering a rebuild button.

## Digest pinning — a roll never pulls a moving tag

The engine resolves the tracked tag (`ghcr.io/outcrop-labs/talaria:main`) to a
**content digest** (`sha256:…`) the way the distribution spec says — anonymous
bearer token, manifest HEAD, `Docker-Content-Digest` — and pulls
`repo@sha256:…`, never `:main`. A tag that moves mid-pull cannot mix commits into
one container. The human-readable version beside a digest is the image's
`org.opencontainers.image.version` label (`sha-<sha12>` — the commit that built
it), read by walking the OCI index. Private forks point `TALARIA_UPDATE_IMAGE`
at their own registry and hold credentials in the `TALARIA_UPDATE_REGISTRY_AUTH`
env (`user:pass`) — an env var, never a settings row, because settings rows
serialize to the admin panel and credentials must not.

CI publishes the image only after everything it needs is fully built and pushed
(the api package image is digest-pinned to the same commit), so "available"
always means "complete and immutable" — an update never rolls onto a
half-pushed artifact.

## The roll, step by step

One update is split across **two processes by design** — the orchestrating
process is the old container, and its last act stops it, so the choreography
must survive its own death:

1. **Gates** — install mode, adoption, no run in flight. A failure here is a
   sentence in the panel, never a touched container.
2. **The lock** — a fleet-wide redis lease (`update:roll`, TTL 20min, renewed by
   heartbeat while the roller lives). It is **never released**: the roller's
   last act stops its own container, and the TTL is what bounds a roller that
   died without stopping itself.
3. **Pull** — `docker pull repo@digest`, recorded as it starts.
4. **Render** — the updater-owned compose project is re-rendered from the
   **live container** on every roll: env (verbatim, minus a short denylist —
   the new image's own version and install signal must win), binds, docker
   gid, dns, networks. Drift between rolls cannot survive a roll.
5. **Green up, gated** — the other slot (`talaria-app` / `talaria-app-b`,
   flip-rendered) comes up **with no published port of its own**. The edge's
   docker provider refuses to route to a `starting` or `unhealthy` container —
   the compose healthcheck *is* the roll gate. An unhealthy replacement is
   removed, the project re-rendered steady, the run recorded failed, and the
   old container never knew anything happened.
6. **Cutover** — the fleet network alias moves to green, the run records
   `cutting-over`, and blue **stops itself** with a drain
   (`TALARIA_ROLL_DRAIN_SECONDS`, default 45s).
7. **Boot reconcile** — green's first moments (a scheduled read, the panel's
   GET, or its first tick) verify the edge actually routes to it — an HTTP
   healthz **through the edge**, not just a container status — and only then
   mark the run `done`. A run left in flight longer than an hour that nobody
   can finish is closed as failed: no install spins forever.

The traffic edge is a pinned traefik container (`talaria-edge`) that owns the
host port the app used to hold; both slots merge into one health-gated service
behind it, so "which slot is live" is a routing fact, not a port reshuffle.
The public port contract (`PORT`, `TALARIA_HTTP_PORT`, compose publishing)
never changes — the edge inherits whatever host port it finds.

## Rollback

The old slot is **kept stopped for 24 hours** as rollback material; after the
window, tidy removes the container and its image (the last digest is always
kept — a rollback target is never garbage-collected away). Inside that window,
**Roll back** on the panel (or `POST /api/admin/updates {action:"rollback"}`)
restarts the old container, health-gates it, moves the alias, and stops the
new one — the mirrored choreography, with the same never-touches-a-dead-plan
guarantees.

If the instance is unreachable through the app entirely, the docker runbook on
the host needs no cooperation from Talaria:

```sh
docker ps -a                       # which slot is running, which is stopped
docker start talaria-app           # or talaria-app-b — the stopped one
docker inspect talaria-app --format '{{.State.Health.Status}}'
docker network connect --alias talaria <fleet-network> talaria-app
docker stop talaria-app-b          # the bad one, once the old one answers
```

## The deploy key (machine key)

`/api/admin/updates` opens for two callers: an admin session, or a per-instance
**machine key** (`x-talaria-key` header) minted from the panel — shown once,
stored as a sha256 hash, replaceable by minting again. The key may `check`,
`apply`, `rollback`, and `adopt` — it is how the one-time migration script
drives the handover; it can mint nothing and flip no toggles, so a leaked
deploy key updates the install but cannot make itself permanent. This is the
credential an external deploy script (talaria-infra's fleet deploy) drives.

## Auto-update: two switches, both off

Nothing updates itself by default. Automatic rolls require **both**:

- **adoption** — the instance handed update control to the engine, and
- **the toggle** — `Update automatically` on the panel (default off, admin-only).

Behind both, the scheduled `update-check` job (every 6h) resolves the tag,
records what was available, and — only when the digest actually moved and no
run is in flight — rolls. A migrated install with no pinned digest is corrupt
state, and the engine refuses to act on it unattended. A registry that moved is
never, by itself, a reason anything changed on a host.

## The local proof — the devbox E2E

The whole engine, adoption included, proves out on one docker host with a
local registry (the `#[ignore]`d live tests cover the registry wire protocol;
the container half is this runbook, by hand or scripted):

```sh
docker run -d --name registry -p 5000:5000 registry:2
# Build the app image twice, different VERSION labels, push v1 as :main
#   docker build --build-arg TALARIA_API_IMAGE=<pin> --build-arg VERSION=sha-v1 #     -t localhost:5000/talaria:main . && docker push localhost:5000/talaria:main
# Boot the stack from that image (docker/compose.registry.yml's override
# shape) with TALARIA_UPDATE_IMAGE=localhost:5000/talaria:main
```

Then the assertions, in order: `POST {action:"adopt", edgePort:<fresh port>}`
brings slot A up healthy and the edge answers the fresh port while the old
container still serves its own; the second adopt — sent **through the edge** —
stops the old one and the run lands done. Push v2 as `:main`, `POST check`,
`POST apply` while a `curl` loop against the edge port asserts zero non-200
(both slots visible in `docker ps` mid-roll). The poison test — an image whose
healthcheck cannot pass — leaves the old container serving and the panel
showing the failure sentence. The rollback test — a green that 500s a route —
round-trips through `POST rollback`. And the untouched-default contract: a
plain `docker compose -f docker/compose.yml up` (no adoption, no registry)
boots and serves exactly as it always did.

## The overlap contract — and the rule it imposes on migrations

For a few minutes, two app containers run side by side (old serving, new
migrating). This is a **supported topology**: the scheduler's jobs are leased
per-tick in redis (one replica runs each), SSE fans out through redis pub/sub,
and migrations run under `pg_advisory_lock` so exactly one boot pass wins. It
works because of one standing rule on every schema change:

> **Roll-safe migrations are expand-contract.** A migration that ships with
> code may only *add* (nullable columns, new tables, new indexes
> concurrently). Dropping or rewriting what the old code still reads is a
> separate migration, shipped at least one release after the code that stopped
> reading it. The old container is live until the drain completes — a
> destructive step under it is an outage wearing a green checkmark.

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for where this rule sits in the PR
checklist.
