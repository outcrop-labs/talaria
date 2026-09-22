- **Agents can now author spreadsheets and web pages, reply in threads, and
  react — and the fitness suite measures whether a model uses those tools.**
  The built-in toolkit was missing three teammate-shaped verbs the HTTP API
  already allowed: `create_sheet` (Files spreadsheet, JSON `string[][]` with
  row 0 the header), `create_page` (HTML microsite), and `react_to_message`
  (the dual-auth reaction route, under the agent's own identity).
  `post_to_channel` takes `threadId` so a reply stays in the thread;
  `read_channel` takes the same id to read one thread. The talaria-toolkit
  skill teaches the reflexes (grid ≠ markdown table; a ✅ is not a new post).
  Fitness: catalog 58 → 61 with sandbox backends; new `hermes:comms` harness
  (six fixtures: read before post, react don't chatter, replies stay in
  thread, the room not a DM, ids from listings, don't spam DMs) bound to the
  workspace-agent fleet slot; `hermes:documents` gains a spreadsheet-vs-
  markdown-table fixture. Guardrails unchanged (no assign, no complete).
  Verified: `bun run check`; `cargo fmt`; `cargo clippy --lib -- -D warnings`;
  `cargo test --lib` on `hermes_comms`, `hermes_documents`, `talaria_tools`,
  `toolbox::sandbox`, `registry::tests`, and `score::tests`.
