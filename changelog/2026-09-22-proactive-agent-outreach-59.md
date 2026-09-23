- **Proactive agent outreach (#59).** Agents stop waiting to be asked, two
  ways. **`message_user`** — the first agent→human write path: a governed MCP
  tool that starts (or continues) a real chat conversation with a teammate
  plus an inbox notification deep-linked to it (`/comms?a=…&x=…`). Personal
  assistants can only reach their owner; every agent↔person pair is
  rate-capped per day (the declined-send reason goes back to the agent so it
  can adapt). **The check-in sweep** — opt-in (master switch off by default
  AND per-agent flags, Admin → Proactive outreach): each opted-in agent
  periodically gets an automated turn through its OWN persona gateway showing
  its stale/blocked/waiting work and its recent outreach ("don't repeat
  yourself"), and acts through its normal tools — ticket comment, channel
  post, or message_user — so everything stays attributed, board-policy-gated,
  guard-visible, and ledger-metered. `outreach_events` logs every check-in
  and DM (caps, repeat-avoidance, admin visibility). Verified live: with
  nothing stale Jax replied NOTHING_TO_SURFACE; given a 96h-blocked
  high-priority ticket he chose to DM the board owner through message_user —
  message in the conversation, notification in the inbox, reasoning in the
  log — and the dedupe context correctly suppressed a near-duplicate
  follow-up; owner-only + daily-cap + unknown-target guards all refuse with
  plain-language reasons.

### Changed
