- **A ticket ref is an address, not a server fault.** Work sessions title
  the ticket `PLAT-118` and tell the agent to open it; `get_ticket`,
  `comment`, `triage_ticket`, `add_time`, `log_usage`, and
  `add_dependency` then ran that ref through `uuid_gate`, which answers
  500 before the access check. An agent who had the ticket could not open
  it. Every task door now resolves a uuid or `PREFIX-N` (the same
  expression the wire uses) and then runs the existing board policy. A
  miss is 404. Two boards sharing the ref is 409, naming the collision,
  rather than picking one. Other toolkit ids (boards, docs, artifacts,
  channels) stay uuids from the list tools — those lookups were not
  casting an agent model as a uuid. Verified: `parse_ticket_address` unit
  test; `cargo check -p talaria-routes-boards`.

### Fixed
