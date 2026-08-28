# CLI reference — `talaria`

> **Generated** by `bun run docs:api` from `cli/src/cmd/**` — the same declarations
> `--help` renders. Do not edit by hand; `bun run check` fails on drift.
> The guide (when to use what): [CLI.md](./CLI.md).

| Command | Summary |
| :--- | :--- |
| [`talaria setup`](#talaria-setup) | first-run dev bootstrap: secrets, fleet config, images, deps |
| [`talaria dev`](#talaria-dev) | run the dev stack: infra, readiness waits, then the app on :5273 |
| [`talaria worktree`](#talaria-worktree) | spin up an isolated worktree stack (own DB seeded from main) |
| [`talaria reset`](#talaria-reset) | destructive resets for a wedged dev stack (typed confirm, no -y by design) |
| [`talaria box new`](#talaria-box-new) | create a devbox: clone + own sidecars, seeded from the primary stack |
| [`talaria box ls`](#talaria-box-ls) | list devboxes (branch, port, container status) |
| [`talaria box enter`](#talaria-box-enter) | run a shell (or command) inside a devbox |
| [`talaria box install`](#talaria-box-install) | install a tool/harness into the layer every devbox shares |
| [`talaria box seed`](#talaria-box-seed) | seed a box with starter data from the primary dev environment |
| [`talaria box stop`](#talaria-box-stop) | stop a devbox's containers (state kept) |
| [`talaria box start`](#talaria-box-start) | restart a stopped devbox (and the shared TEI/SearXNG) |
| [`talaria box rm`](#talaria-box-rm) | tear down a devbox — refuses unpushed work unless --force |
| [`talaria box build`](#talaria-box-build) | rebuild the talaria-devbox:latest toolchain image |
| [`talaria deploy up`](#talaria-deploy-up) | build + start the stack — CONTAINER.md's one command, DOCKER_GID resolved |
| [`talaria deploy down`](#talaria-deploy-down) | stop the stack (--volumes also deletes its data — destructive) |
| [`talaria deploy update`](#talaria-deploy-update) | git pull --ff-only, then the redeploy (up -d --build) |
| [`talaria deploy logs`](#talaria-deploy-logs) | follow the stack's logs (Ctrl-C to detach) |
| [`talaria deploy creds`](#talaria-deploy-creds) | print the first-boot admin Sign-in block from the logs |
| [`talaria deploy status`](#talaria-deploy-status) | effective port/state/fleet + compose ps |
| [`talaria service install`](#talaria-service-install) | start the stack + install the systemd unit that keeps it running across reboots |
| [`talaria service uninstall`](#talaria-service-uninstall) | stop the stack and remove the unit (volumes and state dir are kept) |
| [`talaria service status`](#talaria-service-status) | the unit state (enabled/active), then the compose view |
| [`talaria backup`](#talaria-backup) | snapshot the database + upload blobs (Postgres dump + blobs, staged atomically) |
| [`talaria restore`](#talaria-restore) | restore a snapshot (destructive: drops and recreates the target) |

## `talaria box`

devboxes — a containerized dev environment per task (docs/DEVBOX.md)

### `talaria box new`

create a devbox: clone + own sidecars, seeded from the primary stack

```
talaria box new <name> [--branch <b>] [--from <ref>] [--no-install] [--claude-token <tok>] [--env <k=v>]… [--setup <cmd>]… [--qdrant]
```

Positional `<name>` (required)

| Flag | Kind | Default | Description |
| :--- | :--- | :--- | :--- |
| `--branch` | value | — | branch for the clone (default: agent/<name>) |
| `--from` | value | — | ref to branch from (default: HEAD — committed refs only) |
| `--no-install` | bool | — | skip the in-box bun install + mcp build |
| `--claude-token` | value | — | CLAUDE_CODE_OAUTH_TOKEN for Claude Code inside the box |
| `--env` | value | — | extra env for the box container, KEY=VALUE (repeatable) |
| `--setup` | value | — | shell command run inside the new box (repeatable) — e.g. install another coding harness |
| `--qdrant` | bool | — | also round-trip the Qdrant index (derived; default off) |

### `talaria box ls`

list devboxes (branch, port, container status)

```
talaria box ls
```

### `talaria box enter`

run a shell (or command) inside a devbox

```
talaria box enter <name> [cmd…]
```

Positional `<name>` (required) (takes the rest of the arguments) — then any args pass to the command (default: bash)

### `talaria box install`

install a tool/harness into the layer every devbox shares

```
talaria box install <name> <cmd…>
```

Positional `<name>` (required) (takes the rest of the arguments) — box to run the install in, then the command (quote it as one arg)

### `talaria box seed`

seed a box with starter data from the primary dev environment

```
talaria box seed <name> [--force] [--qdrant]
```

Positional `<name>` (required)

| Flag | Kind | Default | Description |
| :--- | :--- | :--- | :--- |
| `--force` | bool | — | re-copy over an existing seed (and chassis.yml / fleet/.env) |
| `--qdrant` | bool | — | also round-trip the Qdrant index snapshot |

### `talaria box stop`

stop a devbox's containers (state kept)

```
talaria box stop <name>
```

Positional `<name>` (required)

### `talaria box start`

restart a stopped devbox (and the shared TEI/SearXNG)

```
talaria box start <name>
```

Positional `<name>` (required)

### `talaria box rm`

tear down a devbox — refuses unpushed work unless --force

```
talaria box rm <name> [--force]
```

Positional `<name>` (required)

| Flag | Kind | Default | Description |
| :--- | :--- | :--- | :--- |
| `--force` | bool | — | discard uncommitted changes and unpushed commits |

### `talaria box build`

rebuild the talaria-devbox:latest toolchain image

```
talaria box build [--no-cache]
```

| Flag | Kind | Default | Description |
| :--- | :--- | :--- | :--- |
| `--no-cache` | bool | — | rebuild every layer |

## `talaria deploy`

production compose wrappers — up/down/update/logs/creds/status (docs/CONTAINER.md)

### `talaria deploy up`

build + start the stack — CONTAINER.md's one command, DOCKER_GID resolved

```
talaria deploy up
```

### `talaria deploy down`

stop the stack (--volumes also deletes its data — destructive)

```
talaria deploy down [--volumes]
```

| Flag | Kind | Default | Description |
| :--- | :--- | :--- | :--- |
| `--volumes` | bool | — | also remove named volumes: pg-data, qdrant-data, minio-data — the DATABASE goes with them |

### `talaria deploy update`

git pull --ff-only, then the redeploy (up -d --build)

```
talaria deploy update
```

### `talaria deploy logs`

follow the stack's logs (Ctrl-C to detach)

```
talaria deploy logs
```

### `talaria deploy creds`

print the first-boot admin Sign-in block from the logs

```
talaria deploy creds
```

### `talaria deploy status`

effective port/state/fleet + compose ps

```
talaria deploy status
```

## `talaria service`

keep the compose stack running across reboots — a systemd unit (docs/CONTAINER.md)

### `talaria service install`

start the stack + install the systemd unit that keeps it running across reboots

```
talaria service install
```

### `talaria service uninstall`

stop the stack and remove the unit (volumes and state dir are kept)

```
talaria service uninstall
```

### `talaria service status`

the unit state (enabled/active), then the compose view

```
talaria service status
```

### `talaria setup`

first-run dev bootstrap: secrets, fleet config, images, deps

```
talaria setup
```

### `talaria dev`

run the dev stack: infra, readiness waits, then the app on :5273

```
talaria dev
```

### `talaria worktree`

spin up an isolated worktree stack (own DB seeded from main)

```
talaria worktree <name> [base-ref]
```

Positional `<name>` (required) (takes the rest of the arguments) — then optionally a base ref (default: HEAD)

### `talaria reset`

destructive resets for a wedged dev stack (typed confirm, no -y by design)

```
talaria reset <secrets|database|fleet>
```

Positional `<mode>` (required) — secrets | database | fleet

### `talaria backup`

snapshot the database + upload blobs (Postgres dump + blobs, staged atomically)

```
talaria backup [dest] [--keep N]
```

Positional `<dest>` — snapshot directory (default: $TALARIA_BACKUP_DIR or backups)

| Flag | Kind | Default | Description |
| :--- | :--- | :--- | :--- |
| `--keep` | value | — | snapshots to keep, 0 disables pruning (default: $TALARIA_BACKUP_KEEP or 7) |

### `talaria restore`

restore a snapshot (destructive: drops and recreates the target)

```
talaria restore <snapshot-dir> [--target <url>] [--db-only|--uploads-only] [--yes]
```

Positional `<snapshot-dir>` (required) — a directory `talaria backup` wrote

| Flag | Kind | Default | Description |
| :--- | :--- | :--- | :--- |
| `--target` | value | — | postgres URL to restore into (default: DATABASE_URL) |
| `--db-only` | bool | — | restore only the database half |
| `--uploads-only` | bool | — | restore only the blob half |
| `--yes` `-y` | bool | — | skip the typed confirm (automation) |

