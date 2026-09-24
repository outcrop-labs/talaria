- **A drafted email is not ready until it is in the confirm-sends queue.**
  `draft_email` used to return a pending id whenever the insert succeeded,
  even when that row was not in the queue the owner opens — and nothing an
  agent could call would list that queue, so the assistant reported a draft
  as ready when the owner saw nothing. A draft now succeeds only when the
  approver's queue (the same read as the Inbox) contains the id; otherwise
  it errors and the unproven row is discarded. `list_pending_sends` and
  `read_pending_send` are the check. A conversation whose user is not the
  assistant's owner is refused before anything is queued. Verified:
  `bun run check` and `bun run verify` are green (svelte-check 0 errors,
  1267 ui tests). `cargo test -p talaria-google-pending
  -p talaria-routes-integrations -p talaria-fitness-toolbox
  -p talaria-routes-fleet` is 104 passed. Clippy `-D warnings` is clean.

