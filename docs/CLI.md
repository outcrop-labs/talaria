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
| First run on a machine | `talaria setup` → prints your admin credentials, brings up postgres/redis |
| Every day after that | `talaria dev` → infra, readiness waits, the app on **http://localhost:5273** |
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

## Destructive commands

`reset` (secrets / database / fleet) and `restore` drop or replace real state.
There is no `-y`: each asks for a typed confirmation. That is deliberate — see
[`WORKTREES.md`](./WORKTREES.md) for the safer isolation first.
