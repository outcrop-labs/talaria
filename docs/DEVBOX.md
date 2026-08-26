# Devboxes: a container per task

One command, one disposable environment that owns its own repo clone, database,
fleet, and agent CLIs:

```bash
bun talaria box new demo          # clone + sidecars + seed + install
bun talaria box enter demo        # bash inside
bun talaria box enter demo claude # …or Claude Code
bun talaria box enter demo opencode
```

A devbox is the containerized alternative to a
[worktree](./WORKTREES.md): instead of a second checkout sharing the host, it's
a full stack per task — its own Postgres/Redis/Qdrant/MinIO, its own fleet
project, its own agent-CLI home — while the heavyweight *stateless* services
(embeddings, search) are shared with the primary dev stack. Multiple agents
(or humans) can run boxes side by side without touching each other or the
primary: two boxes' apps, databases and fleets coexist by construction.

The trade vs a worktree is weight — each box is five containers, a database
copy, and a home volume — so boxes suit parallel *agent* sessions and
disposable experiments; a worktree remains the lighter tool for a quick second
branch on the host.

## What `new` actually does

1. Ensures the primary dev stack is up (its Postgres is the seed source; the
   shared embeddings/SearXNG are started non-fatally, the `talaria dev`
   posture).
2. Builds `talaria-devbox:latest` if missing (`docker/devbox.Dockerfile` —
   node 22 + bun + git + the docker CLI/compose + Claude Code + opencode).
3. Clones the repo **locally** (hardlinked objects, near-free on btrfs) into
   `../devboxes/<name>/talaria`, checks out `agent/<name>` (`--branch`/`--from`
   to override), and repoints `origin` at the real remote so pushes leave the
   box. **Committed refs only** — uncommitted primary work does not ride in.
4. Snapshots `apps/leadworks` (the gitignored client subrepo) and both
   `node_modules` trees in with reflinks, so a box's builds match the
   primary's and the first `bun install` is a no-op reconciliation.
5. Writes the per-box compose env (0600) and brings up the box project:
   `devbox-<name>` plus sidecars, nothing published except
   `127.0.0.1:53xx:5273` for your browser.
6. Writes the box's `ui/.env`: its own service-DNS URLs, its own state dirs,
   `TALARIA_AGENT_DIAL=container`, `TALARIA_FLEET_PROJECT=devbox-<name>-fleet`
   — and the primary's `TALARIA_SECRET_KEY`/`AUTH_*` verbatim, so sealed
   secrets decrypt and your primary login works unchanged.
7. Seeds starter data from the primary (below).
8. Installs deps and builds the toolkit MCP (`--no-install` to skip).
9. Prints how to get in, and how to tear it down.

## Layout

```
../devboxes/<name>/
├── talaria/            the clone (branch agent/<name>) — /work/talaria in-box
├── state/              fleet/, apps/, uploads/ — bind-mounted at the SAME path
├── compose.env         per-box interpolation (ports, paths, creds) — 0600
├── compose.override.yml  optional: auth/provider env, SSH agent forwarding
└── box.env             the registry `ls` reads
```

`TALARIA_DEVBOX_HOME` relocates the whole tree.

Two paths are load-bearing:

- **`/work/talaria`** — the clone binds at this fixed path (the image's
  `WORKDIR` and its baked `safe.directory` assume it).
- **Same-path state** — `state/` binds at *the identical absolute path* on
  host and container, because the fleet renderer bakes absolute host paths
  into agent bind mounts that the **host** daemon resolves. This is the same
  rule the production container follows, for the same reason.

All paths are canonicalized (`pwd -P`) — invoke through a symlink if you like;
the symlink spelling never leaks into config the box must resolve.

## Networking: three networks, the production shape

```
                    ┌──────────────────────────────────────────┐
                    │ devbox-<name> (the app, the CLIs, you)   │
                    └──────┬───────────────┬───────────────┬───┘
                           │ default       │ talaradev     │ fleet
        ┌──────────────────▼──┐  ┌─────────▼──────────┐  ┌─▼─────────────────┐
        │ postgres redis      │  │ talaria-embeddings │  │ devbox-<name>-fleet│
        │ qdrant    minio     │  │ talaria-searxng    │  │ (spawned agents)   │
        │ (per box, private)  │  │ (shared, from the  │  │ (per box, private) │
        │ nothing published   │  │  primary dev stack)│  │                   │
        └─────────────────────┘  └────────────────────┘  └───────────────────┘
```

- **default** — the box's own project network. The app reaches its sidecars
  by service DNS (`postgres:5432`, `redis:6379`, …). Nothing is published:
  several of these services are unauthenticated, the box network is the
  boundary.
- **talaradev** — the primary dev stack's network, joined read-only in
  spirit. Boxes only dial the two stateless services on it
  (`TALARIA_EMBED_URL`/`SEARXNG_URL` by container name).
- **fleet** — `devbox-<name>-fleet`, this box's private agent network.
  Spawned agents attach to it and reach the app container-to-container as
  `devbox:5273` (`TALARIA_MCP_GW_URL`/`TALARIA_GATEWAY_SELF_URL`).

Why not host-gateway dialing: loopback-published ports are unreachable via
`host-gateway`, and publishing `0.0.0.0` would expose a dev-credentialed app
to the LAN. The shared-network shape is what the production container runs.

## The agent CLIs

Claude Code and opencode are installed in the image (pinned; bump = rebuild
via `bun talaria box build`). Their state lives in the box's **home volume**
(`/home/dev` — `~/.claude`, `~/.config/opencode`, the bun cache), so logins
survive stop/start and are never shared with the host or other boxes.
(Sharing a host `~/.claude` read-write across concurrent CLIs corrupts
`.claude.json` — last writer wins — and bleeds sessions across boxes.)

Auth options:

- **Interactive login** — `bun talaria box enter demo claude` and follow the
  device flow; `enter demo opencode` has its own login. Per-box, isolated.
- **GLM (or any Anthropic-compatible provider) via env** — no OAuth at all.
  Edit `<box>/compose.override.yml`:

  ```yaml
  services:
    devbox:
      environment:
        ANTHROPIC_BASE_URL: https://<provider>
        ANTHROPIC_AUTH_TOKEN: <token>
        ANTHROPIC_MODEL: <model>
  ```

  then `bun talaria box stop demo && bun talaria box start demo`. The
  override file is the deliberate channel: unset vars stay truly absent (an
  empty `CLAUDE_CODE_OAUTH_TOKEN` would shadow real auth).
  **Never set `CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_AUTH_TOKEN` together.**
- **Headless Claude token** — `bun talaria box new demo --claude-token <tok>`
  writes the override for you (same file, same rule).

## Seeding

`new` seeds starter data from the primary dev environment
(`bun talaria box seed <name>` re-runs it; `--force` re-copies):

| What | Always? | How |
|---|---|---|
| Postgres | yes | point-in-time `pg_dump` restore — everything the UI shows |
| MinIO | yes | `mc mirror` of the primary bucket through a dual-homed throwaway (no-op when the primary never uploaded) |
| chassis + `fleet/.env` | yes | template with the network repointed at this box's fleet; LLM endpoint copied from the primary |
| Qdrant | `--qdrant` | snapshot round-trip — **derived** data; default is to re-run the KB backfill in the box's app instead |
| Redis | never | sessions/queues are transient by design |

A seed is a **snapshot, not a link**: later primary changes don't flow.
`apps/leadworks` and `node_modules` copies are snapshots too — refresh by
re-copying or re-running install. The box's `TALARIA_SECRET_KEY` matches the
primary's so seeded sealed secrets decrypt (the one rule from
[`ENCRYPTION.md`](./ENCRYPTION.md)).

## The fleet from a box

Full capability: the box's app drives the host daemon through the mounted
socket exactly like the production container. Each box gets its own
`TALARIA_FLEET_PROJECT` (`devbox-<name>-fleet`), network, chassis and
gateway config — so N boxes + the primary run N+1 fleets on one host without
reconciling each other into oblivion (the fleet project is what used to be
hardcoded; env-parameterizing it fixed the collision at the root, including
for multi-instance production hosts).

Gateway ports seeded from the same dump are *identical across boxes* —
harmless, because `TALARIA_AGENT_DIAL=container` never publishes them; agents
dial compose service names on the box's fleet network.

## Merging and pushing

```bash
bun talaria box enter demo
git push -u origin agent/demo   # then open the PR
```

`origin` is real (the local clone's URL was repointed at creation). If your
`SSH_AUTH_SOCK` was set when the box was created, the agent socket is
forwarded in (`/ssh-agent`) — `git push` just works. The repo also carries an
agentless passphrase key that hangs non-interactive pushes: if a push stalls
with no prompt, run it from a TTY.

`bun talaria box rm demo` refuses while the clone has uncommitted changes or
commits no remote has (`--force` overrides) — those are the only things in a
box that can't be recreated.

## Troubleshooting

- **Builds fail with `Temporary failure resolving…`** — the host daemon's DNS
  config; `bun talaria box build` already passes `--network=host`, which is
  load-bearing on such hosts (see [`CONTAINER.md`](./CONTAINER.md),
  Troubleshooting). Running boxes pin their own resolvers and are unaffected.
- **First `claude` in a box** — expect the login flow; credentials live in
  that box's home volume only.
- **Stale `apps/leadworks` / deps** — snapshots from creation time; re-copy
  from the primary or `bun install` to reconcile the lockfile.
- **A box's app won't start** — `docker logs devbox-<name>`, then
  `bun talaria box enter demo sh -lc 'cd ui && bun run dev'` for the
  foreground story. The box owns its infra; `talaria dev` inside detects
  `TALARIA_DEVBOX` and skips the primary bring-up.
