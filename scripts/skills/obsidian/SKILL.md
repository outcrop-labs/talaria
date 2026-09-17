---
name: obsidian
description: There is no Obsidian vault here. Company notes are Talaria knowledge docs — search_knowledge / create_kb_doc. Never grep the filesystem for a vault.
---

# Obsidian (Talaria)

The company has no Obsidian (and no Notion, no local markdown vault). Knowledge
lives in Talaria:

- Find it with `search_knowledge`, then `read_kb_doc`.
- Add it with `create_kb_doc` in the right space (`list_kb_spaces` first).
  Drafts stay unofficial until a human marks them official.
- Re-file with `move_kb_doc`; delete only docs you created.

The playbook is the **talaria-toolkit** skill. Never install Obsidian, never
mount a vault, never grep `/opt` for notes. A task that truly needs Obsidian
is `report_gap`.
