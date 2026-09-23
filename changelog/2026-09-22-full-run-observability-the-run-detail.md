- **Full run observability — the run-detail modal replaces the small watch
  modal.** The old surface showed agent prose and tool names only; now every
  "watch the work" affordance opens a takeover modal with three panes. LIVE:
  the agent's stream with each tool call's argument preview (the persona's
  display-redacted primary argument — the whole terminal command, where the
  harness steering is legible) and the workbench's own MCP calls with full
  arguments and outcomes (`wtool` frames recorded at dispatch). TURNS: a
  retained per-turn transcript (prompt + stream) captured to a
  `run-transcript` artifact on the ticket at each turn's end — scrubbed of
  known credential shapes, bounded (16K prompt / 256K stream per turn), and
  retained per the new `observability.transcriptRetentionDays` admin setting
  (default 7 days, null = permanent; samples keep a 7-day cap). RESOURCES:
  the agent container's cpu/mem/pids sparklines over the run's window, from
  a new once-a-minute `agent-resource-sample` scheduler job over `docker
  stats` (new `agent_resource_samples` table; admin-only
  `GET /api/fleet/resources`). Known limit, documented: container-side tool
  RESULTS don't ride the persona wire — richer capture needs a Hermes-side
  change (follow-up). Verified: cargo + ui gates; dev stack — a work-session
  turn writes its transcript artifact (secret-shaped strings scrubbed), the
  modal's Turns pane parses prompt + tool-preview lines, wtool frames
  render live, the resources route answers (403 for non-admins), and the
  watch replay shows previews mid-stream.
