- **Work history shows what the agent actually did.** Opening a finished
  work session used to land on an empty Live pane, and Turns said the
  transcript was never captured. The transcript is private and owned by the
  agent model, so the generic artifact read filtered it out. `GET
  /api/runs/{id}/transcript` serves it under the same audience as the watch
  stream. The watch opens on Agent, where those retained replies now live.
  The work-log and stop doors also accept a ticket ref, like the live read.
  Verified: `bun run gate` (check, ui typecheck and tests, cargo
  test of the touched api packages). Not clicked in a browser: no local
  stack, and the dogfood database has no work session to open.
