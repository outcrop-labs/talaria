# Gate hooks

The repo's local quality gates as a library: the check logic lives in shared scripts, the
wiring is per-harness, and the contract below is the only thing a harness needs to know.
Nothing here requires an install — every script is stdlib-only and runs in a fresh
worktree before `bun install` has ever happened.

## The contract

Every hook in this directory speaks it:

| Signal | Meaning |
|---|---|
| stdin | the caller's event payload, whatever shape — tolerated, never read |
| exit 0 | pass — silent |
| exit 2 | block — the reason is on stderr |
| any other exit | the gate could not run — a non-blocking error; never a pass |

[`stop-check.mjs`](./stop-check.mjs) is the entry point today: it runs `bun run check`
(invariants + doc links + generated-reference drift) under this contract, on the whole
tree, with no fast path — see its header for why.

## Wiring it

| Harness | How |
|---|---|
| Claude Code | already wired — the tracked [`../../.claude/settings.json`](../../.claude/settings.json) Stop hook |
| git (works under any harness) | pre-push: `node scripts/hooks/stop-check.mjs` — per-clone via `core.hooksPath`, opt-in |
| CI | nothing to do — [`../../.github/workflows/ci.yml`](../../.github/workflows/ci.yml) runs `bun run check` itself |
| anything else | run the script at your "about to claim done" moment and honor exit 2 |

## Adding a hook

A new dependency-free `.mjs` in this directory, speaking the same contract; wire it once
per harness. Keep it runnable before any install — a gate that needs `node_modules` is a
gate a fresh worktree silently skips.
