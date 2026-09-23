- **Right-click a plan or a research run to rename it, archive it, or delete it.**
  Home's Plans and Research tabs, the plan rail, and the record pages offer the
  same actions — a context menu is a shortcut, not the only home. Plans: rename
  (the existing PATCH), archive (DELETE, the `archived` flag that was already
  on the row — chat decay only sets it for `kind = 'chat'`, so on a plan it
  means a person put it away), restore (PATCH `{archived:false}`, owner-only),
  and hard delete (`?hard=1`, owner-only, a confirm). Archived plans sit in a
  collapsed Archived group on the plan rail, the same shape as archived boards.
  Research: rename (new PATCH `{title}`, owner or admin; the list shows the
  title when it is set and the question when it is null; the Titler only writes
  a title that is still null) and the existing delete, now on the list as well
  as the run page. Runs are not archived. They are ephemeral, and delete is
  already their stop.

  Verified: `bun run check` green. svelte-check over the UI, 0 errors.
  `cargo test --test plan_row_live -- --ignored` against the worktree database,
  3 passed: an owner archives and restores, a collaborator cannot archive or
  restore, a stranger gets 404, hard delete removes the row, PATCH refuses to
  be a second archive door, and a renamed research title survives the Titler's
  `where title is null` write. In the worktree app: right-clicking a plan
  offered Open, Copy link, Rename, Archive, Delete; Archive took it off the
  live list and the archived query returned it; the plan rail's archived row
  offered Restore, and restoring put it back; a research row offered Rename
  and Remove (no Archive), the rename prompt wrote the title, and the run page
  header shows Rename and Remove beside New. `bun run api:check` was not
  re-run locally — it pins the box — CI runs it.
}