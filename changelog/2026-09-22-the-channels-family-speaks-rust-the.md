- **The channels family speaks Rust — the comms plane crosses whole**: all
  ten `/api/channels*` route files (list/create, `{id}` detail/rename/
  archive/hard-delete, the agents pair, conclude, the SSE events stream, the
  members pair, messages GET/POST, message PATCH/DELETE, reactions, read)
  serve from Rust under one proxy prefix, which subsumes the plan-draft
  mount's old path-shape entry. The engine crossed with them (the message
  insert/list/edit/delete, members, agents, the reaction toggle, read
  cursors, the relay-summary conclude, the hard-delete's detached qdrant
  purge) plus the reply plane it drags along — @mention notifications, the
  DM peer notify, and `trigger_agent_replies` with its streamed replies.
  The POST's echo carries a different wire shape than the list (the insert's
  RETURNING selects no guard/editedAt — the keys are absent, not null), so
  the port serializes the page shape and projects the insert shape from it;
  the agent GET strips `guard` outright, keeping findings — flagged
  content's verbatim excerpt — out of model contexts. Verified by a
  102-case byte-diff against TS on the same sessions (reads across the
  channel kinds, a restore-safe write cycle through both runtimes, the 405
  `allow` tables, the zod 400 table with its parse-order probes), which
  caught three port bugs the unit tests hadn't — a mis-numbered bind that
  500'd every message edit, the rename's min-length bound, and a fractional
  `since` that floored where TS fails.
