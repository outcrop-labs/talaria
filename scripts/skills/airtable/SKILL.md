---
name: airtable
description: Airtable is not a system of record here — records are Talaria tickets or spreadsheets. Never the Airtable REST API.
---

# Airtable (Talaria)

There is no Airtable base here. Rows and records are:

- **Tracked work** — `create_ticket` on a board (`list_boards`).
- **A grid someone will sort** — `create_sheet` (row 0 is the header).

Never curl api.airtable.com, never store a personal access token. The playbook
is the **talaria-toolkit** skill. A task that truly needs Airtable is
`report_gap`.
