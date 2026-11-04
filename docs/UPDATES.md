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
  compose are baked in, carrying the same host group ids as the app so it
  can speak to the docker socket) that polls once a second and raises the
  edge the moment the port frees; what it saw, it writes to
  `edge-boot.log` in the update dir. The `update-reconcile` job — a once-a-
  minute reconcile — is the crash fallback (green publishes no port of its
  own until the edge rises, so the fallback cannot wait on a reader), and
  the docker runbook below is the last resort.

The handover is **resumable at every step**: `adopt` on an already-adopted
install is the *resume* — it re-answers `edge-ready`, re-arms the helper, or
finishes the cutover, depending on where the first attempt died. The retired
orchestrator container is stopped by the finish and removed by the reconcile,
so nothing resurrects it — **disarm dokploy's autoDeploy on the resource
before adopting**, or the next push stop-first-deploys underneath the
handover. And after adoption the orchestrator no longer owns the app:
environment changes go through the slot env files in the update dir
(`${TALARIA_STATE_DIR}/update` — `TALARIA_UPDATE_DIR` overrides it), not
through dokploy.

## The taxonomy — who may roll

The mode is resolved fresh from the environment on every call
(`api/src/update/mode.rs`), in trust order:

| Mode | Signal | May roll? |
|---|---|---|
| `image` | the image itself sets `TALARIA_INSTALL=image` (only the published Dockerfile does) | yes — this is the updater's domain |
| `checkout` | a git checkout under `bun server-entry.ts` (`TALARIA_RUNTIME=prod-server`, no image signal) | no — the orchestrator that owns the checkout deploys it |
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
7. **Reconcile** — green's first chances (the `update-reconcile` job's
   once-a-minute tick, the panel's GET, or the 6h check) verify the edge
   actually routes to it — an HTTP healthz **through the edge**, not just a
   container status — and only then mark the run `done`; a green whose edge
   is down raises it itself first. A run left in flight longer than an hour
   that nobody can finish is closed as failed — except an adoption hold
   (this container is the run's own retired orchestrator, alive and serving
   it: the age measures the operator's proxy repoint, not a dead roller) —
   and a run the close marked failed while its containers finished the
   handover anyway heals to `done` on the next reconcile. No install spins
   forever, and no landing is lost to a wrong close.

8. **The fleet** — once that landing is real, every *running* agent rolls,
   one at a time, the same in-place roll an edit that needs a restart gets: its image is
   pulled first if the host lacks it (a failed pull leaves the agent
   running), then the container is recreated and health is waited for. This
   is how a new Hermes pin reaches an image install's running agents. Stopped
   agents stay stopped — they read the new render on next start. An adopted
   install rolls only from the container that *is* the pin (green during
   blue's drain is not a second deploy). An unadopted image install (dokploy,
   plain compose) does the same when its container comes up on a digest the
   fleet has not been rolled for; a restart of the same image is not a
   deploy. Checkout and dev installs do not roll the fleet from here — they
   have no image digest, and a dev restart is not a fleet event; after an
   upgrade there, **Roll all** once (`POST /api/fleet/roll`) moves running
   agents onto a new pin. The roll is detached from the reconcile tick (a
   fleet outlives two minutes) and held on its own redis lease, so two
   replicas do not roll the same fleet twice. A roll that dies mid-way does
   not record the digest, so the next tick retries.


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

## The agent image — Hermes stable, on by default

The agents' Hermes image updates itself, separately from Talaria's own image and
**on by default** (Talaria's own updates above stay off until adopted): every instance
follows `nousresearch/hermes-agent:stable` and keeps its agents on Hermes' stable
releases, with no Talaria release, pull request or repository change per Hermes
release — a release is held for an admin only when it changes a Hermes internal
Talaria relies on. `talaria-hermes-channel` drives it; the records live in Postgres
(`hermes_images`, `hermes_agent_images`, and the `hermes_channel` settings row). The
whole mechanism is in [ARCHITECTURE.md, "The fleet"](./ARCHITECTURE.md#the-fleet).

**Before you upgrade an instance to a release with the channel**, two choices are
yours:

- **To keep every agent where it is**, set `HERMES_IMAGE=<image>` in `fleet/.env`:
  every agent runs that image, as it always did, and the channel says it is off and
  why. (Or Pin a version on the panel afterwards.)
- **Set the workspace's time zone** (Settings, the daily brief's time zone). The
  maintenance window is 02:00–05:00 in it; with none set it falls back to the api's
  `TZ`, then to the zone most admins chose on their profiles, then to UTC — 18:00 in
  California. The panel says which zone it uses, in red when none is set.

Otherwise the channel takes over the fleet as it finds it — see **Release day** below
for what each agent does, and when.

What a release goes through:

- **Check**, every ~6 h with ±30 min of jitter (the first within 10 min of boot):
  one anonymous manifest HEAD for `stable`'s digest, and for a new digest a tag list
  and a HEAD per recent `vX.Y.Z` tag to name it. On Docker Hub none of these counts
  against the anonymous pull limit, which counts manifest GETs.
- **48 hours on stable, before anything is downloaded**, counted from the first check
  that saw the digest as `stable` and started again whenever `stable` moves. A waiting
  release that `stable` stops naming is never rolled out: it gives way to whatever
  `stable` names now (or, if `stable` went back to the fleet's own image, to nothing).
  One withdrawn after it moved some agents takes them back to the fleet's image first,
  banning nothing.
  This is the defence against a bad or compromised tag that needs no Talaria release.
  There is no remote kill switch.
- **Prepare**: pulled in the background through the agents' own shared pull, waiting
  while any other download runs, and **never past the disk's room** — the docker root
  (where Postgres's volume lives too) must have room for the image, sized as the one
  the fleet runs now plus a quarter, and still keep a tenth of its filesystem free; a
  host without it holds the release and tells the admins. A failed pull is retried
  15 minutes later, then twice as long each time, to six hours. A **rollback**'s
  download needs only the image's size and a fiftieth of the disk free — it is the way
  off a bad release. Where the api cannot read the docker disk, or would read the wrong
  one — no `/host` mount, a remote docker (`DOCKER_HOST`, or a `DOCKER_CONTEXT` other
  than the default), devicemapper or zfs storage — nothing but a rollback is downloaded
  until an admin presses **Roll now**. On the containerd image store, containerd's root
  is read too, and the smaller free space counts. (A remote daemon reached another way,
  or a containerd root moved from `/var/lib/containerd`, is not detected.) Then the image is read
  with networking off for its `_config_version`, the env names its Hermes strips and
  its s6 drain tools, and **its contract with Talaria is checked**: on every agent's
  rendered config, whole — MCP servers, messaging platforms and toolset lists included, each
  literal credential replaced by a stand-in first, found by the same rule Talaria keeps
  every config's secrets with (credential-named keys, URL userinfo and query tokens,
  provider token prefixes, JWTs, a command line's `--token` value) — what Hermes runs
  with its own migration skipped (Talaria stamps the image's version, so Hermes never
  migrates it) must equal what it runs once it has migrated it; Hermes must still keep a
  sealed `CONFIG_*` credential out of the commands an agent runs; and a scheduled job
  Hermes has claimed must still read as claimed, refreshed often enough for Talaria to see
  it running. The inspection runs with networking off, read-only, as nobody, with every
  capability dropped; what it stores or shows never repeats a value from the configs. It
  fails closed: an agent whose config cannot be read stops the check (the host's, retried),
  never a pass on a default config; its sandbox failing (out of memory, a full tmpfs) is
  the host's too, never a contract change to hold. A digest is inspected again when the
  agents' configs have changed since (an agent hired, a toolkit or platform added),
  before its next rollout. Every rollout's image is checked to be on the host before it
  moves an agent; one that left (an operator's prune) is pulled again.
- **Canary, then one agent at a time, each watched**, in the maintenance window or on
  **Roll now** (which also skips the 48 hours). The canary also answers the
  `/health/detailed` and `/api/jobs` fields Talaria reads to tell whether an agent is
  working. The canary soaks for 5 minutes; the first agent is watched 10 minutes and
  each later one 3 before the next moves; the whole rollout soaks 15 minutes before the
  release is called good. A release that changed a contract is **held**, not banned:
  the panel lists what changed, and **Accept and roll out** sends it on — for the problems
  it listed; one the canary finds afterwards holds it again. A canary that does not answer
  is the host's, not a contract change.
- **An agent that is working or about to is passed over** and asked again a minute
  later: a reply being written or a conversation in the last 10 minutes; a turn in
  flight in its Hermes API server or a delegated task (`/health/detailed`); a
  messaging-platform turn (the gateway's own snapshot); a scheduled job running (its
  claim, which Hermes refreshes every minute while the run lives, long tool calls
  included) or due within 3 minutes — except a job that runs every few minutes, whose
  missed fire Hermes runs late after the restart; a work session or workbench job; a
  model call in the last 5 minutes. An agent whose Hermes does not answer those
  questions is not restarted on its own; **Roll now** and **Pin** restart it anyway,
  after a confirm. A Hermes whose answer never passed the canary's check here — one
  from before the channel, such as the `:latest` (v0.21.4) an older Talaria left its
  agents on — lacks some of those fields: v0.21.4 has no `api_server`, and its queue
  count sees neither a Talaria chat turn nor a cron. Its answer is read for what it has,
  and Talaria's own records decide: a reply being written or a recent message catches a
  chat turn, and the job's claim (which v0.21.4 heartbeats too) catches a cron. One
  busy all window is left for a later window rather than hold the release; one left
  behind three windows in a row is said to the admins, and again every week it stays
  behind.
- **What a person set, stays**: an agent that is down — stopped with Talaria's **Stop**,
  `docker stop` or `compose stop`, removed by `compose down`, or left created — is
  assigned the new image and starts on it when someone starts it; the channel never
  starts it. Down means "start it" only when docker says it crashed (restarting or
  dead, seen by two checks in a row: docker's own restart answers one crash) or the
  channel itself left it down (a recreate of its own that did not finish). A Stop
  pressed while the channel recreates an agent stands. (A chassis whose agents run
  `restart: no` leaves a crashed container exited, and that reads as stopped; the stock
  chassis restarts a crash.)
- **No agent is moved down.** An update or a catch-up never moves an agent onto an
  older Hermes than it runs (a newer `:latest` an older release left it on waits for a
  newer stable), and an agent that fails on a release goes back to what it ran — its
  own image, not the fleet's. Only a Pin or a Roll back, an admin's word, moves down.
- **An agent counts as moved only once its container runs the image.** A roll that
  did not land, a put-back that failed, or an agent running another image than it was
  assigned is moved again; with no rollout open, an agent off its image opens a
  catch-up onto it. One that fails on the fleet's own image during a catch-up goes back
  to what it ran and is left there — the admins are told — until the fleet's image
  changes or an admin presses **Roll now**; one the host fails three times in a row is
  given up the same way, and the catch-up goes on with the rest.
- **A failure is confirmed before anything is banned**: a failure the host caused
  (docker timing out or refusing, the api unable to reach a container docker calls
  healthy, the disk saturated) is retried after 15 minutes, then twice as long each
  time to six hours, and the third in a row tells the admins once. A failure the image
  caused puts that agent back at once and is tried once more; the same thing failing
  again marks the release bad and rolls every agent on it back, and every admin is
  notified (`hermes_update_failed`). A third failure of anything bans it too, and a
  saturated disk excuses at most two. A reboot or a docker restart during a soak is the
  host's, not the release's. A version already known good here (a rollback's, a
  catch-up's, a pin to one) is never banned; a pin to one stops at its first agent that
  will not come up healthy on it, and **Roll now** goes on. A release no agent ran (every
  one stopped) is not called good. **Allow again** on the panel lifts a ban.
- **Roll back** moves every agent on the version being left back now, mid-turn
  included, one at a time; agents on other versions are left alone. A release `stable`
  goes back from (to the fleet's image, or an older release) is rolled back the same
  way, banning nothing; one merely superseded by a newer stable is not — its agents go
  on to the newer release in its own rollout. **Unpin** stops a move onto the pin.
- **Stuck states alert** (`hermes_channel_stuck`), once a day each: checks failing for
  3 days, a rollout open for 7, the host failing three times in a row, or a channel
  state row this release cannot read (written by a later one, or damaged). Such a row
  pauses only the updates — hiring, starting and editing agents never read it — and
  **Reset** on the panel sets it aside (kept as it was, under
  `hermes_channel:archived:<ms>`) and starts afresh with every agent on its image.

The panel is **Manage → Agent image** (admins): the version each agent runs, the
channel, the last check, the rollout, and Check now, Roll now, Roll back, Pin, Allow
again, Accept and Reset. `GET/POST /api/fleet/hermes` is the same surface. An action
is carried out by the channel's next step, within a minute.

**The knobs.** `TALARIA_HERMES_MIN_STABLE_HOURS` (default 48) and
`TALARIA_HERMES_STABLE_REF` (default `nousresearch/hermes-agent:stable`; a mirror) pass
through `docker/compose.yml` to the api — set them in `docker/.env`. The soak lengths
(`TALARIA_HERMES_CANARY_SOAK_SECS`, `TALARIA_HERMES_FIRST_AGENT_SOAK_SECS`,
`TALARIA_HERMES_AGENT_SOAK_SECS`, `TALARIA_HERMES_ROLLOUT_SOAK_SECS`) are for tests and
are not passed through.

**Opting out, or holding a version.** Any one of these stops the channel moving agents:

- **Pin** on the panel (`POST /api/fleet/hermes {"action":"pin","digest":…}` or
  `{"action":"pin","version":"v0.22.5"}`) holds every agent on any version not marked bad,
  until **Unpin**; one that never ran here is canaried first.
- `HERMES_IMAGE=<image>` in `fleet/.env` (or the api's environment) runs every agent on
  that image, as it always did; the channel says it is off and why.
- A chassis whose `image:` is not a stock `${HERMES_IMAGE:-…}` default is the operator's,
  and the channel leaves it alone.

**A host that cannot reach Docker Hub** keeps every agent where it is: the failed check
is recorded (the panel shows it) and retried at the next check. A fresh install renders
the compiled pin first (the last entry of `HERMES_IMAGE_DEFAULTS`).

**Release day — upgrading from a release before the channel.** The channel is the one
thing that changes an agent's Hermes, and it keeps what it finds. With the defaults
(48 h on stable, the 02:00–05:00 window), in order:

- **The upgrade (T0).** The migrations add `hermes_images`, `hermes_agent_images` and
  `agent_defs.stopped_at`. Nothing restarts yet.
- **T0 + 1½ min — the channel's first step.** Every agent found down (stopped on the
  older release, by hand, or by `compose down`) has its stop recorded and stays stopped.
  Every running agent is assigned the image it runs (main's `:latest`, recorded as found
  — by its registry digest, or by its image id if it was built or loaded on the host), so
  every recreate from here on keeps it on that Hermes. An agent docker will not describe
  is asked again every minute, holds up no other, and is said to the admins after ten
  tries. Each image found is inspected, one a minute, with nothing downloaded, for its
  Hermes version.
- **T0 + 5 min — image installs only.** The deploy's own fleet roll recreates every
  running agent on the new Talaria render, each on the Hermes it runs. It waits for the
  step above, at most 30 min; past that it still leaves any agent the channel has not
  recorded exactly as it is. Agents a person stopped stay stopped. A checkout install
  has no such roll.
- **T0 + up to 10 min — the first stable check.** It records today's `stable`, and its
  48 h start.
- **Day 1, within hours of T0 — the pin's download.** Agents on a Hermes known to be
  older than the compiled pin (main's `:latest` was v0.21.4 on most hosts) are to move
  onto it, so the pin is downloaded (about 1.1 GB compressed, about 4.5 GB on disk) and
  inspected as soon as the found images' versions are read — in the day, not the night.
  Only the canary and the moves wait for the window. An image whose version cannot be
  read (a nightly-style tag, an inspection that failed) is kept and shown on the panel as
  "version unknown"; it is never moved down, and stable takes it on like any other.
- **Night 1 (the first window after the pin is downloaded and inspected** — so an upgrade
  late in the evening moves on night 2). The pin's canary soaks 5 min, then the agents below it
  move one at a time: the first watched 10 min, each other 3 min. The move stops at the
  first agent that will not come up on the pin: it goes back on the image it ran, the
  admins are told, and the rest stay where they are until stable takes them on. Agents at
  or above the pin don't move.
- **Day 3, typically — stable's download,** the moment its release has been on stable 48
  h (another ~1.1 GB compressed), and its inspection. **Night 3 (the first window after):**
  its canary, then every agent onto it one at a time, watched, then good.
- **Fleet size.** One window is 180 min, and a watched move costs about 6 min of canary,
  12 min for the first agent, 5 min for each other (the roll and its 3-min watch) and a
  15-min soak: about `1 + (180 − 6 − 12 − 15) / 5`, so **25–30 agents a night**. A larger
  fleet pauses at the window's end and goes on the next night, and the move onto stable
  waits until the move onto the pin is done, by as many nights.
- **Restarts per agent,** for a fleet that fits in one window: on an image install, one
  at T0 + 5 min on the same Hermes; then night 1 for an agent below the pin; then night 3
  onto stable. On a checkout install, night 1 (below the pin only) and night 3. Larger
  fleets spread each move over more nights. An agent stopped before the upgrade, if a
  person starts it before a rollout reaches it, starts on its own old `:latest` (the
  image it was adopted on) and moves with the next rollout; once a rollout has assigned
  it the fleet's image, it starts on that (not watched: it was stopped).
- **Disk.** Until the old `:latest` goes, the docker disk holds it (about 3.9 GB), the pin
  (about 4.5 GB, never pruned) and stable's release (about 4.6 GB): **about 13 GB of
  Hermes images** at the peak, less where layers are shared. Have **about 15 GB free on
  the docker disk, with 10 % of it kept free** — the channel checks before each download
  (the fleet's current image's size plus a quarter — 6 GB when that image is not on the
  host — with a tenth of the disk kept free) and holds the download, telling the admins,
  when there is not room.
- **The old `:latest` is not removed by the channel:** it was found, not pulled by this
  install, and another stack on the host may use it. Once no stack uses it, remove it:
  `docker image rm nousresearch/hermes-agent:latest`. (The channel prunes only images it
  pulled itself, and counts one pruned only once its id is gone from the host: on
  docker's classic image store, removing a digest reference while a tag remains only
  untags, and that is recorded as "untagged, still present".)

Pre-pulling the pin before an upgrade (`docker pull <the pin>`) still saves day 1 its
download.

**Rolling Talaria back to a release before the channel** leaves agents on the newer
Hermes until each is next rendered and applied: the older release ignores the channel's
tables and row, renders its own pin and stamps that pin's config version. Hermes never
migrates a config while it runs (it reads `config.yaml` again on change, and migrates
only at a boot on a new image), so a container still on the newer image reads the
older stamp as it is — safe for any release the channel rolled out, because the
contract check proved that release's migration from the compiled schema leaves every
agent's config, as Hermes reads it, unchanged. The next apply recreates the agent on the older pin. A
later upgrade picks the channel up from its row.

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
plain `docker compose -f docker/sidecars.compose.yml -f docker/compose.yml up` (no adoption,
no registry)
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

### What counts as a contract step, and the check that enforces it

The rule covers more than data loss. The previous release keeps serving on
the migrated schema during the drain, after an aborted roll (the unhealthy
green is removed, but its migrations stay applied), and for the whole Roll
back window. A step that takes away something its code reads or writes breaks
it, even when every row survives:

| Step | What the previous release hits |
| :--- | :--- |
| drop a table or column, rename one | its queries name the old thing |
| `alter column … type` | its binds and reads use the old type |
| `alter column … drop default` or `set not null` | its inserts leave the column out and fail NOT NULL |
| drop a primary key or unique constraint (a re-key) | its `ON CONFLICT (…)` names a target that is gone, and the write answers 500 |
| delete an `app_settings` row (a setting moved elsewhere) | it reads the key and finds nothing |

`bun run check` enforces this list on every statement from index 526 on
(the roll-safety guard in `scripts/check-invariants.mjs`). Such a statement
must carry `-- contract: <what goes>, unread since <vX.Y.Z or #PR>` inside
it, naming the release or pull request that stopped reading it. A
`-- deliberate:` marker does not pass it. Moving data follows the same rule:
copy it to the new place, keep the old copy current while both releases can
run (the new code writes both, or a trigger mirrors them), and delete the old
copy one release later.

### Rolling back across the git-hosts release

The git-hosts registry (#611) broke this rule. Its migration deleted
`app_settings.github_config` after moving it into `git_hosts`, dropped the
`'github'` default from every `host_id`, and re-keyed `workbench_repos` and
`workbench_repo_flow`. On rollback the previous release showed GitHub as not
connected, and every grant, job and repo request it filed failed NOT NULL. The
release that ships the registry also appends the compensation (`pg.ts`, "The
git-hosts move, made roll-safe"):

- `github_config` is copied back from the `github` row. Two triggers keep the
  copies equal while both releases can run, so a change on either side
  (connect, rotate, clear, remove) reaches the other.
- `host_id` defaults to `'github'` again on `workbench_repos`,
  `workbench_repo_flow`, `workbench_jobs` and `workbench_repo_requests`, so
  the previous release's inserts work.
- A render on the previous release puts `fleet-gateway` back on
  `gateway_unmetered_keys`. This release's relay ignores that entry, so after
  rolling forward again every call on Talaria's own key is still metered.

Two things still fail on the previous release over this schema:

- **Editing a repo's git flow** (`PUT /api/workbench/flow`) answers 500. Its
  upsert targets `ON CONFLICT (repo)`, and a unique index on the repo alone
  would refuse the same owner/name on two hosts. Reading flows works. Edit
  the flow after rolling forward.
- On an upgrade straight from the previous release, the #611 statements and
  the compensation apply in the same migration pass. While that pass runs,
  the old container sees GitHub as not connected.

The contract step ships in the release after this one: drop both triggers and
their functions, delete `github_config`, and drop the four defaults. Each
statement is marked `-- contract: … unread since <this release>`.
