- **Boards can move between teams after creation.** The board settings
  dialog (General tab) now has a Team select for owners — Personal or any
  org team — and the sidebar's drag-a-board-onto-a-team-header move now asks
  first. Moving changes who can see the board, so the confirm dialog
  previews exactly who would lose sight of it before the move applies;
  cancelling applies nothing. The move itself records who moved the board
  and from which team to which in the audit trail (`board.team_move`).
  New rule alongside: the mover must be a member of the destination team —
  an owner marching a board into a team they are not on was an access grant
  nobody approved. People with a direct share of the board keep it through
  any move; only team-derived access follows the board's team. The API
  gained `previewAccessLoss` on `PATCH /api/boards/{id}` (answers the loss
  list without applying the move; owner-only like the move itself).
  Verified: cargo check on the touched crates, the boards-store and
  route-test suites, `bun run check` (invariants + generated docs drift),
  svelte-check, and the UI test suite; the settings dialog was exercised
  in a real browser against a live dev stack.
