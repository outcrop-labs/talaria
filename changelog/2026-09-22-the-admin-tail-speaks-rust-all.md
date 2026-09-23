- **The admin tail speaks Rust — all fifteen admin routes, the secrets
  inventory, and the engine halves behind them.** Settings, users, the
  secrets inventory, encryption, domains, invites, email, search, judge,
  outreach, guardrails, platform-agents, storage, apps, and the
  workspace-secrets admin console crossed together, carrying their engines:
  `secret_health` (presence and provenance over every store that holds a
  sealed value — never the value), the rotation engine (unit-verified, not
  fired on a shared dev box), invites with their email half, the email
  config and test send, search reachability, the apps discovery/enable
  plane, and the nine-action workspace-secrets union. The slice's real work
  was fidelity to zod's own rejections: every body was re-probed against the
  TS oracle and rewritten onto the grown `body.rs` helper set (the enum's
  `Invalid option: expected one of` with no received clause, `expected int,
  received number`, the literal's `expected true`, custom grammar sentences,
  and the tri-state `nullish_member` that tells an absent patch key from a
  null one). Verified by a 60-case byte-diff; the diff caught a silent-200
  bug (an invalid role enum value was dropped, not refused), a doubled
  backslash in SQL that 500'd the encryption GET, and the guard findings'
  float4 confidence decoding into an empty list. `talaria dev` now lifts
  `SEARXNG_URL` for the Rust process so the search panel's env flag agrees
  with its own probe.
