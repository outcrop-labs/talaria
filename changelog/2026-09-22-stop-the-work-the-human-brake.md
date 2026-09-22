- **Stop the work — the human brake, ported from its stranded branch and
  finished.** Once an agent picked a ticket up, nothing on the board could
  end the session: the send step may legitimately stream for hours, and a
  cancel that only flips the row leaves the driver running anyway. Now
  `POST /api/tasks/{id}/work-session/stop` cancels every live session on a
  ticket (board-member gated, idempotent), the cancel fires the local
  driver's abort so the in-flight call drops at its next await, and the
  ticket keeps its status and assignees — a stop is a triage event, not an
  outcome, recorded as one activity line. The brake is wired where the work
  is watched: the ticket's workbench strip, the run-detail modal, and the
  card's right-click menu.
  Verified: live twice — curl (cancel, the "stopped by Jon" activity line,
  `finishedAt` in the history read, idempotent second stop) and in the
  browser (the strip's Stop button clicked against a live session; the run
  read flipped to cancelled and the strip re-rendered) — plus `GET
  /api/tasks/{id}/work-sessions` answering board-member authed with live and
  finished rows in one read.
