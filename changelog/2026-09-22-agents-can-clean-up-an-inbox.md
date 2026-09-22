- **Agents can clean up an inbox, without ever being able to empty one.** Three
  new fleet tools over the connected Google account: `list_labels` (Gmail's
  folders ARE labels — INBOX and UNREAD are system ones), `create_label`
  (find-or-create, so a retry is safe), and `organize_emails` (apply/remove
  label names on up to 100 messages by id: removing INBOX archives — mail stays
  in All Mail — removing UNREAD marks read). The HITL line is deliberate and
  follows the platform's own rule: sends and invites leave the building under
  the owner's identity and wait for approval, while filing, archiving and
  mark-read stay inside the mailbox and are reversible — so they apply
  immediately, because "clean up my inbox" behind fifty approval cards is not
  cleanup. TRASH and SPAM are refused everywhere (service layer, sandbox,
  agent-facing routes), so nothing in the toolkit can delete mail. Message
  listings now carry label names alongside each message. Organizing needs the
  `gmail.modify` scope (swapped in for `gmail.readonly`); a connection granted
  before this needs one reconnect, and the routes say so when they hit it.
  Fixtures grade the two real risks: filing into a label that was named but
  never created, and reorganizing a mailbox without reading a single message in
  it — including archiving an unread mail its owner still needs.
