- **Agents can no longer do dev work in chat — the rule is stamped into every
  soul at hire and at render, checked by grading, and written where humans
  configure agents.** The 2026-09-23 policy ticket followed an agent editing
  the codebase straight from a chat thread with no ticket and no go-ahead,
  bypassing every control the ticket + workbench flow provides. The routing
  rule now holds in two layers that share no files. Hire-time: every built-in
  role template, the fleet hire-time starter soul, and personal-assistant
  souls carry it beside the human-in-the-loop line, and the UI blank
  role-template scaffold seeds new templates with it. Render-time: a dev-work
  policy header rides every rendered soul's header chain beside the toolkit
  contract. The Muse soul-generator requires the rule in generated souls, and
  a grading fixture rejects any soul missing it, so edited or regenerated
  agents cannot quietly drop it. Docs state it where humans meet it
  (admin-agents, working-with-agents, WORKBENCH.md), and the in-repo
  workbench-driving skill spells out the chat-to-ticket routing move with no
  "trivial fix" loophole. Routing behavior, everywhere it is stated: a
  dev-work request in chat produces or links a ticket, instructions given in
  chat land on that ticket as comments, and execution moves into a workbench;
  even an explicit out-of-band request routes through a ticket first.

  Verified: `bun run check` green (invariants, doc links, generated
  references, changelog roll); `bun run api:check` green (fmt, clippy
  `-D warnings` all targets, cargo tests incl. the new template/fleet/
  grading pins, the Muse fixture, and the dev-policy header test);
  `bun run verify` green (svelte-check + ui vitest incl. the role-template
  scaffold change). Exercised: `bun talaria dev` — a freshly hired built-in
  agent's soul carries the rule at both layers; Fleet → Role templates
  renders the scaffold with the rule; a Muse document-kind generation passes
  the dev-work guardrail check.
