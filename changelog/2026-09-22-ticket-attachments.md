- **Ticket attachments.** Tickets now carry the same attachment chips as chat
  messages: uploaded files plus knowledge-doc/artifact refs (ACL-checked
  against the attacher, content clipped into the chip for models). Attach and
  remove from the ticket detail; changes log to the ticket's activity. Agents
  see attachment metadata in `GET /api/tasks/:id` and can now pull the bytes
  from `/api/uploads/:id` with the fleet key; agent callers can attach uploads
  but not refs (no session to ACL-check). Verified live end to end, 11/11
  checks.
