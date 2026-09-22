- **Enter in the chat composer reliably sends: the @-mention and :emoji
  autocomplete menus no longer swallow the key when they have nothing to
  pick.** The suggestion menus' key handler claimed Enter/Tab even while their
  candidate list was EMPTY (menu mounted by an earlier query, then emptied —
  e.g. `:zz` mounts the emoji menu, appending a character empties its matches),
  and ProseMirror consults plugin key handlers before keymap plugins, so the
  send keymap never saw those presses: intermittent, silent, no error. The
  menus now claim nothing with an empty list (a pure, unit-tested decision
  shared by both renderers), and the send keymap additionally refuses any
  Enter that belongs to an active IME composition — belt to ProseMirror's
  suspend-on-compose braces. Verified: `bun run check`, `svelte-check`, and
  the full vitest suite green (1118 tests incl. 7 new composer-keys tests);
  the production build compiles; and the running surface was driven in a real
  browser via CDP — plain Enter sends once, Shift+Enter inserts a newline and
  the next Enter sends exactly once, a populated menu picks the highlighted
  row without sending (then sends once on the following Enter), a
  composition-Enter never sends, and a mounted-but-emptied menu falls through
  so Enter sends (this last case demonstrated FAILING before the fix, PASSING
  after).
