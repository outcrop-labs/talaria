- **@mention autocomplete in the rich editor.** The thread #60 left open:
  ticket comments and descriptions NOTIFIED on mentions but their TipTap
  composers had no autocomplete — you had to guess the token. `RichEditor`
  now takes a `mentions` prop (a TipTap Suggestion popup, modeled on the
  slash-command menu): type `@` and pick from the people the mention will
  actually notify. Picks insert plain `@token ` text — the exact grammar
  the server parses and the renderer highlights, so the markdown
  round-trip is untouched. Wired on the ticket surfaces with board
  members: the comment composer and both description editors (inline +
  slide-in). Also closed for consistency: a ticket CREATED with an
  @mention in its description now notifies board members (only the edit
  path did; template-seeded descriptions can't fire it). KB/artifact
  bodies stay unwired on purpose — mentions there don't notify yet
  (eligibility undecided, recorded in TODO).
