---
name: cleanup
description: Remove the local workspace a dev task created — worktree, devbox, scratch checkout, throwaway download — and leave shared caches alone. Use when a task is about to be claimed done, when the stop gate blocks on disk pressure or stale artifacts, or when the disk is filling up.
---

# Clean up after a dev task

This is the last step of every task, on every harness. Do it before you claim done.
The stop gate is the backstop, not the step: [`scripts/hooks/stop-check.mjs`](../../../scripts/hooks/stop-check.mjs)
runs [`scripts/cleanup-sweep.mjs`](../../../scripts/cleanup-sweep.mjs) `--gate` after `bun run check`, and exit 2
means you are not done. It does not delete a fresh worktree for you. The numbers live in `POLICY` in that script. Today: a worktree or box
untouched for 7 days, scratch under `/tmp/talaria-*` older than a day, disk use at 85%, or
2 GiB of artifacts the sweep is willing to remove. Harnesses whose stop event can block a
turn run that script themselves (see [`scripts/hooks/README.md`](../../../scripts/hooks/README.md)); the rest still do this step.

## When the task ends

Remove what **this task** created. Do it before you claim done, and not while a
pull request from this task is still red — you may need the workspace to fix.
Once the proof line is green, tear it down.

| You created | Tear it down |
|---|---|
| `bun talaria worktree <name>` | from the **primary** checkout, not from inside it: `docker compose -p talaria-wt-<name> down -v && git worktree remove --force ../talaria-<name> && git branch -D wt/<name>` |
| `bun talaria box new <name>` | `bun talaria box rm <name>` (it refuses a dirty or unpushed clone; `--force` is a human decision) |
| a scratch checkout or throwaway download | put it under `/tmp/talaria-<name>` when you create it, and `rm -rf` it when you are done |
| a one-off `CARGO_TARGET_DIR` or cold build dir | same prefix, `/tmp/talaria-*`, so the sweep can see it |

If you are still inside the worktree, the sweep will not delete the checkout it is running in.
Leave, then run the teardown from the primary. The next stop gate, in any checkout, will
flag it once it is stale.

## What stays

- code, configs, the branch, the pull request
- `node_modules` (worktrees symlink the primary's)
- the cargo registry, the bun cache, `../devboxes/shared` (the tools layer every box mounts)
- the primary checkout's `api/target` and `desktop/src-tauri/target` — a warm cache. The
  sweep names them when the disk is already over the line; it does not delete them.
  `cargo clean --manifest-path api/Cargo.toml` is a human decision
- another session's checkout. A worktree is ours only when it is the `wt/<name>` +
  `../talaria-<name>` pair `talaria worktree` creates. A long-lived checkout on another
  branch is not yours to remove
- anything with a `.talaria-keep` file in its root

A dirty tree or commits no remote has are flagged, not removed. Read the flag. Do not
`--force` someone else's uncommitted work.

## The command

```bash
bun talaria cleanup            # report. Prints even when under the line
bun talaria cleanup --apply    # remove the safe set, then report what remains
```

`--apply` removes stale clean worktrees, stopped clean boxes, old `/tmp/talaria-*`, and
compose projects whose directory is already gone. It will not remove the primary, the
checkout you are in, a running stack, a dirty or unpushed tree, or a `.talaria-keep`.

`talaria dev` prints one line when disk use crosses the threshold. It does not refuse
to start — you may need the stack to finish the task that will free the space.

## When the stop gate blocks

stderr names the disk reading and the removable set. Run `bun talaria cleanup --apply`,
then claim done. If the block is only pressure and the report's removable set is empty,
the disk is full of something this sweep does not own (an oversized `target/`, docker
images). The message says which. Free that, or say so — do not silence the gate.
}