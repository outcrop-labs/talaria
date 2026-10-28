- **Fixed: `talaria reset` run in a worktree resets that worktree's stack, not main's, and every other CLI command that acts on a stack takes its targets from the worktree too.** `reset database|secrets|fleet` exec'd into `TALARIA_PG_CONTAINER ?? 'talaria-postgres-dev'`. Nothing sets that variable in a worktree, so the reset worked on MAIN's database, and its fleet sweep removed every container whose name held `talaria-fleet-agent`, which only main's fleet produces. A new `cli/src/stack.ts` resolves the stack from the checkout's own `ui/.env`: a worktree's `talaria-pg-<name>`, `talaria-wt-<name>` and `talaria-wt-<name>-fleet`, with fleet containers picked by compose project label. `reset` prints those targets before its typed confirm. Before the first count, it checks that the postgres it would exec into belongs to the worktree's compose project and publishes `DATABASE_URL`'s port. It passes the fleet project to `compose down` with `-p`, and it refuses a rendered `fleet/docker-compose.yml` that names another project. Every command that acts on a stack now refuses, and says why, when the target would be a guess:
  - a shell variable naming another stack's container or worktree;
  - a worktree with no `TALARIA_FLEET_PROJECT`, whose agents went to main's project;
  - a linked git worktree with no marker;
  - the installed `talaria` shim run from inside another checkout. The shim always runs the primary's CLI, so in a worktree `talaria reset`, `dev`, `backup` and `restore` acted on the primary.

  More changes:
  - `talaria dev` no longer lets a shell `TALARIA_PG_CONTAINER` override a worktree's own container.
  - `deploy` and `service` in a worktree need a `COMPOSE_PROJECT_NAME` of its own instead of the production default `talaria`, and `service install`/`uninstall` refuse there.
  - `restore` in a worktree refuses blob restores into main's shared object storage, a bucket nobody named, a dir inherited from main's env, and a shell `DATABASE_URL` that differs from `ui/.env` when no `--target` is given.
  - `talaria worktree` takes its docker names from the same table.
  - The name-substring `containerNames` and `containerRunning` had no users left and are deleted.

  Running `reset database` live turned up a bug that predates this change: the mode had not worked since the bash script was ported (3368cf6c). The bash script fed its statements on stdin. The port gave psql all three as one `-c`, which is one transaction, and Postgres refuses `DROP DATABASE` inside one. The mode had already terminated every connection by then. Each statement now runs as its own `-c`, and the drop is `with (force)`. Reset's counts each print on their own line again. Docs: WORKTREES.md, CLI.md and BACKUPS.md.

  Verified: `bun run gate` (check, cli typecheck, cli tests). New unit tests resolve every command's target in a worktree and in the primary: stack.test.ts, reset, dev, deploy, service, backup and restore. Live, in the fu-reset worktree, with this branch's CLI only (the unfixed reset was never run):
  - The dry-run plan for each mode named only `talaria-pg-fu-reset`, `talaria-wt-fu-reset` and `talaria-wt-fu-reset-fleet`.
  - `reset secrets` cleared that DB's sealed rows and kept its board and user.
  - `reset fleet` removed the three containers labelled for that fleet project: a compose-made agent, a compose orphan and a label-only stray. It also removed the rendered files and kept `chassis.yml`. A decoy named `talaria-fleet-agent-decoy-fu-reset`, with no project label, is what the old name filter would have taken. It kept the same `Id` and `StartedAt`. The fleet was a hand-written compose project in the renderer's shape, not a real Hermes hire.
  - `reset database` dropped and recreated that DB.
  - These were each refused before any docker call: a shell `TALARIA_PG_CONTAINER=talaria-postgres-dev`, the CLI run from another checkout, `deploy down` and `service install`.

  Main's postgres, redis, embeddings and searxng containers kept the same `Id` and `StartedAt` across the runs. Its database read the same through the CLI's own `pg_dump` before and after. It held no tables before the runs, so that half of the check compares empty with empty. CI is the full test line.
