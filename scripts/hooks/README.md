# Gate hooks

The repo's local quality gates as a library: the check logic lives in shared scripts, the
wiring is per-harness, and the contract below is the only thing a harness needs to know.
Nothing here requires an install — every script is stdlib-only and runs in a fresh
worktree before `bun install` has ever happened.

## The contract

Every hook in this directory speaks it:

| Signal | Meaning |
|---|---|
| stdin | the caller's event payload, whatever shape — a hook reads it if it needs it (`pre-push` does: git hands it the refs), and one that needs nothing tolerates it |
| exit 0 | pass — silent |
| exit 2 | block — the reason is on stderr |
| any other exit | the gate could not run — bad usage, an unresolvable ref; never a pass |

A Stop gate and a git hook read that last row differently, and the difference is worth
knowing: **git refuses a push on any non-zero hook exit**, so exit 1 from
[`pre-push`](./pre-push) stops the push and names what to fix; a harness running a Stop gate
decides for itself what exit 1 means. Neither ever treats "could not run" as a pass — that
is the part that matters.

Three entry points:

- [`stop-check.mjs`](./stop-check.mjs) — runs `bun run check` (invariants + doc links +
  generated-reference drift) and then [`../cleanup-sweep.mjs`](../cleanup-sweep.mjs) `--gate`
  (disk pressure and stale dev artifacts). Both under this contract, on the whole tree, with
  no fast path — see its header for why the check has no skip list, and why the sweep is not
  part of `bun run check`.
- [`pre-push`](./pre-push) — the branch-flow guard. Git invokes it with one line per ref about
  to leave the machine (`<local ref> <local sha> <remote ref> <remote sha>`) and forwards each
  to [`../flow-guard.mjs`](../flow-guard.mjs), the same policy
  [`../../.github/workflows/flow.yml`](../../.github/workflows/flow.yml) runs server-side. A
  push to `main` or `rc` that the model does not allow stops here with the reason; the model
  is [`../../docs/BRANCHES.md`](../../docs/BRANCHES.md).
- [`pr-watch.mjs`](./pr-watch.mjs) — after a pull request is open. Polls check runs and
  mergeable state. Exit 0 only when checks are green and the PR is mergeable against `rc`;
  exit 2 names `red`, `conflict`, `pending`, `timeout`, `closed`, or `base` on stderr. It
  does not edit or push. Not wired as a Stop hook — [`ship-a-change`](../../.claude/skills/ship-a-change/SKILL.md)
  runs it, because whether a harness should spawn it on its own is an open call. Exit 0
  prints one proof line to stdout (the state an agent may cite); every other rule of this
  contract holds, including "could not run is not a pass".

## Wiring it

| Harness | How |
|---|---|
| Claude Code | already wired — the tracked [`../../.claude/settings.json`](../../.claude/settings.json) Stop hook |
| git (works under any harness) | `talaria setup` sets `core.hooksPath` to this directory, which is what makes [`pre-push`](./pre-push) run; per-clone by hand: `git config core.hooksPath scripts/hooks` |
| CI | nothing to do — [`../../.github/workflows/ci.yml`](../../.github/workflows/ci.yml) runs `bun run check` itself, and [`../../.github/workflows/flow.yml`](../../.github/workflows/flow.yml) runs the branch policy |
| anything else | run [`stop-check.mjs`](./stop-check.mjs) at your "about to claim done" moment and [`pr-watch.mjs`](./pr-watch.mjs) after the PR opens; honor exit 2 |

A git hook is bypassable (`git push --no-verify`) and a stop gate is not a wall either; both
are there so the right thing is the easy thing. What makes the branch model hold is the
server-side half — branch protection, and `flow.yml` failing the push after the fact.

## Adding a hook

A new dependency-free script in this directory, speaking the same contract; wire it once per
harness. Keep it runnable before any install — a gate that needs `node_modules` is a gate a
fresh worktree silently skips. A file git has to find by name (`pre-push`, `pre-commit`) needs
that exact name and the executable bit, not a `.mjs` extension.
