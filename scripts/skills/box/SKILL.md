---
name: box
description: Company files live in Talaria Files — create_document / create_sheet / create_page / save_image_artifact. There is no Box drive here.
---

# Box (Talaria)

There is no Box account in this container. Deliverables go in Talaria Files:

- Markdown — `create_document` / `update_document`
- Spreadsheet — `create_sheet` (row 0 is the header)
- Web page — `create_page`
- Images you made under /opt/data — `save_image_artifact`

Sharing and officializing stay a human's. Never install a Box CLI, never store
a Box token. The playbook is the **talaria-toolkit** skill. A task that truly
needs Box is `report_gap`.
