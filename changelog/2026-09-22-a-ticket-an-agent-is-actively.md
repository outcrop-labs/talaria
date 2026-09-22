- **A ticket an agent is actively working wears the work: a dithered
  live-work strip with a Watch-the-work modal.** While a work session is
  live on a ticket, the detail view carries the same dither field the
  generating blocks wear — "work is happening here" — with the agent's name,
  the turn count, and the live phase sentence; a CTA opens a modal that
  streams the session's own run events (`/api/runs/{id}/events`) for
  instant phase transitions, shows the tail of the agent's last reply (the
  session's checkpointed answer, not a re-read of the model), and refreshes
  the ticket's activity and comments as work lands. Backed by a new
  `GET /api/tasks/{id}/work-session` (board-gated like the ticket read)
  returning the live session row or null. A window into the work, not
  control of it — nothing in the modal touches the session.
