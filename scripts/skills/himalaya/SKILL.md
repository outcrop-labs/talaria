---
name: himalaya
description: There is no Himalaya CLI here and no IMAP/SMTP from the terminal. Mail goes through Talaria — read_recent_email / read_email / draft_email — and nothing is sent until a person approves it.
---

# Himalaya (Talaria)

`himalaya` is not installed and must not be. It needs a standing IMAP/SMTP
credential in the container, and mail on this box never works that way.
The mailbox is the account connected in Talaria:

- **Read** — `read_recent_email`, then `read_email` for the body.
- **Write** — `draft_email`. It waits for a human to approve in Talaria;
  `list_pending_sends` is the proof it queued.
- **File** — `list_labels` / `create_label` / `organize_emails`.

The **email** skill is the full playbook and the **talaria-toolkit** skill
is the rest. Never store a mailbox password and never configure an IMAP
account. A task that needs a mailbox Talaria does not have is `report_gap`.
