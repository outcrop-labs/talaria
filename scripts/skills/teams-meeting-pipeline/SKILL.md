---
name: teams-meeting-pipeline
description: Meetings, notes, and tickets live in Talaria — not a Microsoft Graph / Teams pipeline. No Graph subscriptions from this container.
---

# Teams (Talaria)

There is no Teams connector in this container. Meeting notes, decisions, and
follow-ups:

- Notes that should persist — `create_kb_doc` or `create_document`
- Work that should be tracked — `create_ticket`
- Something one person must see now — `message_user`

Never register Graph subscriptions, never store a tenant token. A task that
truly needs Teams is `report_gap`.
