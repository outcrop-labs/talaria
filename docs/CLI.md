# The `talaria` CLI

One command owns the whole repo: first-run setup, the dev stack, isolated
worktrees and devboxes, container deploys, backups, and the destructive resets.
`bun talaria setup` installs a bare `talaria` on your PATH; before that (or
anywhere Bun is present) the same tree runs as `bun talaria <command>`.

Every command, flag, and positional — the complete table:
[`CLI-REFERENCE.md`](./CLI-REFERENCE.md). It is generated from the same
declarations `--help` renders, so it cannot drift. This page is the guide:
which command when.

## The everyday loop

| When | Command |
| :--- | :--- |
| First run on a machine | `talaria setup` → brings up postgres/redis; claim the admin account in the app |
| Every day after that | `talaria dev` → infra, readiness waits, the app on **http://localhost:5273** |
| New Talaria app | `talaria app new <slug>` → TypeScript skeleton in `apps/<slug>` ([`APPS.md`](./APPS.md), [`sdk/`](./sdk/README.md)) |
| Something's wedged | `talaria reset --help` first; every reset is destructive and typed |
| Shipping it | `talaria deploy up` → the container stack ([`CONTAINER.md`](./CONTAINER.md)) |
| A host that reboots | `talaria service install` → the stack, supervised by systemd across reboots |
| Before you need it | `talaria backup` ([`BACKUPS.md`](./BACKUPS.md)) |


## Parallel work

Two isolation levels, depending on how far you want to be from main:

| Tool | Isolation | Use it when |
| :--- | :--- | :--- |
| `talaria worktree <name>` | git worktree + its own stack (DB seeded from main) | A second feature in flight; throwaway branches |
| `talaria box new <name>` | full devbox container ([`DEVBOX.md`](./DEVBOX.md)) | Reproducing an environment, agent sandboxes, long-running side work |

Worktrees and boxes both get their own ports and state; `talaria dev` inside
either is the same loop as the primary stack.

Inside a worktree, run commands as **`bun talaria …` from the worktree**. Each
command that acts on a stack (`dev`, `reset`, `backup`, `restore`, `deploy`,
`service`) then takes every container, compose project and database from the
worktree's own `ui/.env`, never from main's defaults. The installed `talaria`
shim always runs the primary's CLI, so run from inside a worktree those commands
refuse and print the `bun talaria` line to use instead
([`WORKTREES.md`](./WORKTREES.md#rules-of-the-road)).

## Destructive commands

`reset` (secrets / database / fleet) and `restore` drop or replace real state.
There is no `-y`: each asks for a typed confirmation. Before it asks, `reset`
names its target: the stack, its postgres container with compose project and
port, and for `fleet` the project and every container in it. Answering
anything but the mode name is the dry run. That is deliberate — see
[`WORKTREES.md`](./WORKTREES.md) for the safer isolation first.
