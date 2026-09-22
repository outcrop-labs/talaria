- **The work log, readable after the work is done.** `GET
  /api/tasks/{id}/work-sessions` is the record beside the live read's now —
  every session the ticket has had, live first, finished newest-first, with
  the agent, state, turn count, and when it ended — and the ticket detail
  renders it as a Work log strip: one row per session, an eye to open the
  run detail, where the retained per-turn transcripts already live. Reviewing
  what an agent did no longer requires having memorized its run id while the
  session was live.
  Verified: the read exercised authed (empty list, then live + cancelled +
  done rows with the right states and `finishedAt`), and the strip rendered
  in the browser on a ticket with four sessions of history — the View log
  eyes open the run detail on each.
