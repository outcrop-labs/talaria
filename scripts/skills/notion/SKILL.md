---
name: notion
description: Notion is not a system of record here — pages and databases are Talaria knowledge docs and boards. Never the Notion API or ntn CLI.
---

# Notion (Talaria)

There is no Notion workspace on this box. "Put it in Notion" means:

- **Pages / wikis** — `create_kb_doc` / `edit_kb_doc` (drafts unofficial until
  a human promotes them). Find existing with `search_knowledge`.
- **Databases / trackers** — a board (`create_ticket`) or a spreadsheet
  (`create_sheet`, row 0 is the header).

Never install `ntn`, never mint a Notion token, never scrape notion.so. The
playbook is the **talaria-toolkit** skill. A task that truly needs Notion is
`report_gap`.
