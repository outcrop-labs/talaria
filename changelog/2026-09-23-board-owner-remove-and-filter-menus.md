- **Board owners can be removed, and filter menus no longer stack.** Adding
  someone as an owner on a board (settings → people, or the new-board invite)
  left the role tag sitting on the remove control, and the settings dialog was
  too narrow to separate them. The dialog is wider, the role tag cannot cover
  the remove icon, and an owner can take another owner off — the last owner
  stays. Opening one board filter menu closes the others.

  **Verified:** `bun run verify` green (svelte-check 0 errors, 1208 UI tests)
  and `bun run api:check` green. Browser on the worktree stack: settings
  dialog is 768×512, an added owner's remove icon is hit-testable and removes
  them, the last owner has no remove control and the API refuses that delete,
  an editor cannot grant owner, and opening Assignee closes the Status menu.
