- **`GET /api/agent/whoami`** — the introspection call agents were missing:
  identity (personal? whose? elevated?), every board it can work with *why*
  (policy / owner / elevated), channels, MCP servers, the guardrails that
  will refuse it, and its own pending access requests — plus the MCP tools
  that act on the answer (`whoami`, `join_board`, `leave_board`,
  `request_board_access`). The fleet verifier's auth oracle moved onto this
  purpose-built route from its old borrow on `GET /api/users`, so narrowing
  the people directory can no longer take the toolkit dark.
