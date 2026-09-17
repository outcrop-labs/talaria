---
name: google-workspace
description: Google the Talaria way — read_calendar / draft_calendar_event / read_recent_email / draft_email / search_drive. Outbound is queued for a human; there is no gws CLI.
---

# Google Workspace (Talaria)

Gmail, Calendar, and Drive are the account connected in Talaria, through the
`talaria` MCP tools — not the `gws` CLI, not a service-account JSON in the
container.

- **Mail** — `read_recent_email` / `read_email` / `draft_email` (awaits approval).
- **Calendar** — `read_calendar` / `draft_calendar_event` (awaits approval).
- **Drive** — `search_drive` (read-only). Export a Talaria doc with
  `export_to_google_doc`.

Never say something was sent or a meeting was created; say it awaits approval.
Never run `gws auth`. The playbook is the **talaria-toolkit** skill.
