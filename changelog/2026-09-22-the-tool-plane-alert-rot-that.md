- **The tool-plane alert: rot that every green dashboard missed.** During the
  2026-09-22 incident an agent container failed every tool call for half an
  hour while container health read healthy, stats read idle, and the model
  stream said `completed` on every call — the failure verdict existed nowhere
  the platform could read it. It does now: the plugin's verdicts feed a
  per-agent consecutive-failure streak, and ten in a row raises a warning on
  the alerts surface — "{agent}'s tools are failing" — with the remedy in the
  sentence (roll the container). The work-dispatch sweep's log line is honest
  for the same reason: it counts only tickets actually re-offered to pickup
  columns and says how many sat parked outside them, where it used to count
  both as "re-offered" — a number that read as motion during the incident
  while nothing was dispatchable.
  Verified: a streak key in Redis surfaced the warning live on
  `GET /api/alerts` ("Test Engineer's tools are failing", severity warning,
  roll-the-container sentence), and the sweep's new line ran live in the dev
  stack logs — "re-offered 3 ticket(s) to their agents (1 parked outside
  pickup columns)".
