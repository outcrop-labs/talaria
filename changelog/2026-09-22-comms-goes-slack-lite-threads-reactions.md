- **Comms goes Slack-lite: threads, reactions, paste-a-file, edit &
  delete.** Channel messages (channels, Relays, DMs) now spawn THREADS —
  a side panel with the root + replies and its own composer; replies stay
  out of the main flow, roots carry a "N replies · when" rollup, and an
  agent @mentioned inside a thread answers IN the thread with the
  thread's own conversation as its context (replying to a reply re-roots
  Slack-style — threads never nest). REACTIONS: a quick-react palette on
  hover, chips with counts and who-reacted tooltips, click to toggle —
  and agents can react under their own identity (one of our twists).
  FILES: paste an image or drop files straight into any composer
  (channels and agent DMs) — they upload immediately and ride as
  attachment chips, images rendering inline. Plus own-message EDIT
  (inline, enter-to-save, "(edited)" marker) and DELETE (author or
  channel owner; a root takes its thread with it, the confirm says so).
  Relay conclusions summarize thread content too. The composer itself is
  now the Slack-shaped RICH EDITOR: markdown syntax as you type (**bold**,
  `code`, ``` blocks, > quotes, lists) with a formatting toolbar in a
  controls row under the full-width input, Enter sends (inside a code
  block it newlines), Shift+Enter soft-breaks, @mention and :emoji:
  autocomplete inline, and a searchable emoji picker next to attach.
  Reaction palette dismisses on Esc/outside-click/mouse-leave.
