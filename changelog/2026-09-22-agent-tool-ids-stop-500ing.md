- **A bad tool id is a client address, and a first comment is not a server
  fault.** `uuid_gate` answered 500 for any non-uuid path id, and several
  toolkit doors cast the id before that gate — board task lists, documents,
  knowledge docs and spaces, channel reactions, a thread page. Those now
  answer 400 or 404 before the query. A fractional `since` on a channel
  read is 400, not an int4 cast. The first comment on an agent-created
  ticket no longer 500s when a `user:` assignee is an email or a name: that
  token is skipped, and a board member can still hold the room. If nobody
  can, the comment door answers the same 409 as opening the room. Verified:
  `cargo check` on the touched route crates; `bun run check`.

### Fixed
