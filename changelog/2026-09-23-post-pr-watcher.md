- **Agents watch the pull request after it opens.** A local green is not
  done: CI runs the full tree, and `rc` can move under the branch. An agent
  may not report dev work done while its open PR has failing checks, pending
  checks, or a merge conflict. The gate is `scripts/hooks/pr-watch.mjs` — same
  exit contract as the stop gate, and it does not edit or push. The procedure
  is the last step of ship-a-change. Verified: `bun run check` green.
  Exercised for real, not dry-run: the watcher read
  [#447](https://github.com/outcrop-labs/talaria/pull/447) and exited 2 `red`
  with the invariants job log naming `docs/pr-watch-exercise.md:3`
  ([job](https://github.com/outcrop-labs/talaria/actions/runs/35828733119/job/107076192983));
  the broken link was removed, local check went green, and the fix was pushed.
  It read [#448](https://github.com/outcrop-labs/talaria/pull/448) and exited 2
  `conflict` (`mergeable=CONFLICTING`, `mergeStateStatus=DIRTY`); `rc` was
  merged, the overlap in `docs/UPDATES.md` was resolved by keeping `rc`'s fleet
  note, local check went green, and the resolution was pushed. Both exercise
  PRs were then closed; the watcher reported `closed` and did not pass.
