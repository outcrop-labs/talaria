---
name: email
description: Mail the Talaria way — read_recent_email / read_email / draft_email. Drafts await human approval; there is no IMAP CLI and nothing is sent until a person confirms.
---

# Email (Talaria)

There is no Himalaya, no standing IMAP credential, and no send-from-the-container
path. Mail you act on is the account connected in Talaria:

- **Read** — `read_recent_email` (snippets) then `read_email` (the body). The
  snippet is a teaser, not the letter.
- **Write** — `draft_email`. It queues for a human to approve in Talaria. Never
  say it was sent; say it awaits approval.
- **File** — `list_labels` / `create_label` / `organize_emails`. Nothing is
  deleted; TRASH/SPAM are refused.

The playbook is the **talaria-toolkit** skill. Never run `himalaya`, never
`gh`-style device login for mail, never store a mailbox password. A task that
needs a mailbox Talaria does not have is `report_gap`.
