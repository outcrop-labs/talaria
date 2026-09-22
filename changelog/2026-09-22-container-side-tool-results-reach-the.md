- **Container-side tool RESULTS reach the run-detail modal — the talaria-events
  Hermes plugin.** Hermes' plugin API exposes `pre_tool_call`/`post_tool_call`
  with full arguments and results (the one datum no platform wire carried); a
  small Talaria plugin — embedded in the api, seeded into the fleet tree by
  render, mounted read-only at every agent's `~/.hermes/plugins/`, enabled in
  config.yaml — reports each call to the new agent-authed
  `POST /api/agents/tool-events`, which re-clamps, secret-scrubs, and lands
  `toolfull` frames on the live run's watch stream (live pane + retained
  transcripts, no Hermes fork). The plugin is best-effort by contract: a
  bounded queue and a short-timeout POST mean observability never blocks an
  agent's tool loop. Correlation is the agent's newest live work session
  (v1 approximation; exact persona-session pinning is the follow-up).
  Verified: cargo + ui gates; dev stack — render seeds and enables the
  plugin, and an agent-authed POST lands a `toolfull` frame on the watch
  replay with the args and result scrubbed (a planted `tak_…` arrived as
  `[redacted:key]`).
