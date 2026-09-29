- **Build caches and finished worktrees stop filling the disk in silence.**
  `api/target` had grown to 34 GiB without `talaria cleanup` ever mentioning
  it: the sweep only built a build-dir row once disk use crossed 85%, which
  on a 638 GiB disk meant 542 GiB used before the largest thing the repo
  produces became visible at all. Build dirs are now inventoried and sized at
  every disk reading, for the primary and for each worktree, and the report
  names them with a `cargo clean` line once the total passes 20 GiB — a loud
  line, never an exit 2, because a gate that blocks on something the sweep
  refuses to delete is a gate with no way through. A side worktree's build dir
  untouched for 14 days is now reclaimable by `--apply`; the primary's stays a
  warm cache that is only ever named. `mise.toml` puts cargo behind a shared
  sccache (20 GiB, `~/.cache/sccache`) so separate target dirs no longer mean
  separate cold builds, and reclaiming one is cheap to undo. A merged pull
  request now ends the seven-day idle clock it used to wait out: the `--gate`
  run stops that worktree's docker stack — containers, volumes, network — and
  the checkout becomes removable by `--apply`, so a finished worktree stops
  costing RAM and a Postgres the moment its work lands. Merged means the pull
  request merged and nothing else; ancestry is explicitly not the test,
  because a worktree cut an hour ago from `main` is also an ancestor of `rc`
  and tearing that down would take out a live stack someone else is using.
  Docker's own reclaimable total is reported too, and left alone — an exited
  container is as often a one-shot init sidecar as it is garbage. Verified:
  `scripts/cleanup-sweep.test.mjs` (20 checks, 8 of them new, covering merged
  vs idle classification, the stack row, build-dir staleness, and the path
  guard that keeps the primary's cache and `../../.ssh` out of reach) and
  `bun run check`. Confirmed against the live box: the 34 GiB cache is named
  at 16% disk where it previously printed nothing, and two active worktrees
  from other sessions were correctly left running after the ancestry
  false-positive was removed.
