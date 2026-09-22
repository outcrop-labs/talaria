- **Toolkit onboarding — agents get the playbook, not just the tools.** The
  talaria MCP was attached to every agent but nothing taught them when to
  reach for it. Now a fleet-wide `talaria-toolkit` skill (seeded from
  `scripts/skills/` on render, admin-editable after, mounted read-only at
  `/opt/skills` — a mount that was documented but never actually wired) walks
  the reflexes: search before planning, keep the ticket alive, durable output
  goes in Talaria, drafts await approval, report_problem on breakage. The
  rendered SOUL header points at it.
