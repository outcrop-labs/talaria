- **Agents carry their own default effort, set where the model is picked.**
  The agent editor's model rows — main model and every tier alias — now show
  the effort chip beside the picker when the chosen model publishes levels,
  and the saved level becomes that agent's conversation default: Comms DMs
  with the agent (or the tier) and the assistant panel start there, and the
  chat routes apply it server-side when a sender made no pick, re-validated
  against the model's live levels so a stale config is inert. Precedence is
  specific-over-general: your explicit conversation pick > the agent's
  configured default > your platform default (Settings) > the model's own.
  `/api/models/efforts` answers both halves (`efforts` + `default`), and the
  persona resolver reads the configured effort from the same cached agent
  config walk that resolves capability keys — one read, one TTL, re-pointing
  an agent's model or effort follows within a minute.
