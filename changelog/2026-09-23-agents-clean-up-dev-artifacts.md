- **Agents clean up the artifacts a dev task leaves behind, and the stop gate
  says so before the disk fills.** Worktrees, devboxes, scratch checkouts and
  throwaway downloads were surviving the task that created them. Each one is
  small; together they fill the disk, and the first notice was a failed build.
  The convention — what a finished task removes, and what it leaves (code,
  configs, `node_modules`, the cargo registry, the shared devbox tools layer,
  the primary checkout's `target/`) — is the cleanup skill. `bun talaria cleanup`
  reports the stale set; `--apply` removes only the safe part of it (a
  `wt/<name>` worktree or a stopped box untouched for a week, clean and pushed;
  `/tmp/talaria-*` older than a day; a compose project whose directory is
  already gone). A dirty tree, unpushed commits, a running stack, the checkout
  you are in, and a `.talaria-keep` file are left alone. The stop gate runs the
  sweep after `bun run check` and exits 2 when disk use crosses 85% or the
  removable set crosses 2 GiB, so claiming done is the moment it gets said.
  `talaria dev` prints the same pressure line and does not refuse to start.
  Verified: `bun run check` (the new sweep tests ride in it — a stale clean
  worktree is removable, a dirty or human checkout is not, `--gate` does not
  delete a worktree, the executor refuses a path outside the convention) and
  `cd cli && bun test`. Exercised `node scripts/cleanup-sweep.mjs` in report
  mode against this checkout (read-only: it flagged a 14-day-old devbox and
  left an unpushed one alone, and exited 0 under the disk line) and
  `--pressure`. A throwaway `/tmp/talaria-*` fixture was removed by the
  executor; a path outside the convention was refused and left in place.