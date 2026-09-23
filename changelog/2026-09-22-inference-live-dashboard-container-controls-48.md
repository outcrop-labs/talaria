- **Inference: live dashboard + container controls (#48).** The Compute page
  is now a real inference dashboard: a live strip showing **generating right
  now** (per agent, from streaming reply rows with a 10-minute recency clamp
  and partial indexes to keep the scan tiny), the **gateway pulse** (upstream
  calls / errors / time-to-first-byte p50+p95 over the last 15 minutes,
  in-memory — a pulse, not a ledger), **fleet temperature**
  (up/warming/unhealthy/down), and the last hour of per-agent activity; the
  page leans into a 5s poll while anything is generating or warming. The
  fleet roster finally shows the **warm-up state**: Docker's `starting`
  health phase (the healthcheck's 60s start_period) renders as a pulsing
  amber "warming up" dot instead of masquerading as up, and `up`/`unretire`
  return immediately instead of blocking the request for up to two minutes —
  the polled roster tells the story. Two new controls per agent:
  **Restart** (quick bounce, confirm-gated since in-flight replies drop;
  owners can restart their own assistant) and **Roll** (admin — the existing
  zero-downtime blue/green replacement, previously only reachable through
  config saves, now a button: fresh container warms up while the old one
  keeps serving, then drains). Both audit-logged. Verified live: restart
  showed `starting` → healthy on the roster, a roll flipped the slot with
  the old container serving throughout, and a real streamed reply appeared
  in "Generating now" with the gateway pulse moving. (Managing inference
  BACKEND containers — Ollama/vLLM lifecycle — is the ROADMAP's separate,
  bigger thread; this ships the agent-container half plus the dashboard.)
