- **The agent's work lands in the document pane, not as a wall of text in the
  chat.** A work session's pane now opens the file the session is about and
  follows the agent as it works: when a turn creates or edits a Talaria
  document, the pane brings that file into view without anyone clicking, and
  when a later tool changes it the pane re-reads it mid-turn rather than after
  the turn finishes. It keeps its scroll and any unsaved edit while the new
  body arrives, because a landed write invalidates just that file's cache
  instead of throwing the editor away.

  The work-mode prompt now says the same thing to the agent: the document is
  the output, not the message. Make the change with a tool and let the pane
  show it — do not paste the document, the new section or the rewritten rows
  into the conversation as well, because the teammate is already looking at the
  file and a duplicated wall of content buries the one sentence they needed. A
  turn's message says what changed and why. Proposing rather than doing is
  still prose, and the agent is told to say which one it is doing.

  This needed no new endpoint and no new column: the link chip a landed turn
  already carries names the document, so the pane reads the conversation's own
  chips to know what to open.

  Plan deliberately does NOT work this way, and should not. A plan's document
  is DERIVED from the conversation by a model job that rewrites it after each
  turn, so the prose in a plan's chat is the input rather than a dump, and
  refreshing per tool would re-run that job against a half-finished turn. Two
  panes, two opposite contracts: Work's agent writes the file and the pane
  reads it, Plan's conversation is the file's source.

  Verified: `bun run check` green; `bun run typecheck` 0 errors (2 pre-existing
  a11y warnings in WorkchainCanvas, untouched); 1321 ui tests pass.
