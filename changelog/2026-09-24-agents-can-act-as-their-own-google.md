- **An agent can act as its own Google account, and can use Google Docs.**
  An admin sets each agent's principal on its record: its owner's account,
  the shared org account, or the agent's own connection. Existing agents are
  seeded from the old owner-column rule, so nothing changes until an admin
  opts in. An outbound write for an owner who has not connected Google is
  refused by name and does not fall back to the org account. Creating a Doc
  or a folder is immediate and returns the link. Importing a Drive file
  creates a Talaria artifact and does not change the Drive file. Editing a
  Doc the agent did not create, moving or renaming a Drive file, rescheduling
  or cancelling an event, and creating a meeting all queue for approval. A
  meeting's agenda Doc is created only when a human approves. In the doc
  editor, a file that already has a Google link offers Sync to Google and
  Pull from Google; pull overwrites the Talaria body only after a confirm.
  Calendar reads accept timeMin, timeMax, and maxResults. Verified:
  `bun run gate` is green, including the import tool (check, svelte-check
  0 errors, 1305 ui tests, mcp typecheck, and fmt, clippy, and tests for
  the touched api packages). Schema snapshot was not regenerated.
