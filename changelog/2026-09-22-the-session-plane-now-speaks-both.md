- **The session plane now speaks both runtimes**: the second port batch's first slice is
  serving — Redis `sess:<sid>` sessions (the shared store both runtimes read while the
  port runs), `GET /api/auth/session` (user + denied views + effective permissions, read
  from the DB so an admin's change lands without re-login), `POST /api/auth/logout`, and
  `GET /api/users` — the fleet-wide auth oracle every `mcp/` agent authenticates through,
  ported with its agent-caller resolution: per-agent `tak_` keys where the key proves
  identity and `x-agent-name` can only narrow it, and the org-wide legacy key resolving
  identified-but-untrusted while refusing outright a name that carries human privilege.
  The proxy now forwards `Set-Cookie` plural-safe (a comma-joined multi-cookie header is
  a corrupt one, and an auth plane that cannot clear its cookie through the boundary is
  not ported). Verified: `/api/auth/session` and `/api/users` responses byte-identical
  across the TS and Rust servers against the same Redis sessions (admin, member,
  anonymous); a Rust-served logout observed immediately by TS; the oracle's accept /
  wrong-name-refusal / 401 paths against a real `tak_` key, `last_used_at` landing and
  the row restored after.
