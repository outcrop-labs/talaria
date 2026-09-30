- **A Google file opens in the Work pane, in Google's own editor.** Paste a link
  to a Doc, Sheet, Slides deck or Drive file and it sits beside the session —
  edited in Google's real editor, with Google's own cursors and presence doing
  the collaboration, rather than mirrored into a Talaria copy that would then
  disagree with the original. The file is pinned to the session, so it survives
  a reload and a collaborator opening the session sees the same file rather than
  an empty pane.

  Each file type gets the embed Google actually serves for it: a Doc and a Sheet
  stay editable in the frame, a deck embeds as a player because Slides has no
  embedded editor, and anything that is not a native Google type falls back to
  Drive's preview. The pane says "read-only here" on the two that are, rather
  than leaving someone to discover it by typing into a frame that ignores them.

  "Open" — in a new tab, in the real editor — is always there, not an error
  state that appears when something breaks. The embed is cross-origin, so a
  browser blocking third-party cookies shows Google's sign-in wall inside the
  frame and the page cannot see that, or anything else, through it. There is no
  honest "did it work" signal to branch on, so the escape hatch is permanent.

  A pasted link that is not a Google file is refused with a sentence instead of
  pinned as a blank frame, and look-alike hosts are refused too — the value ends
  up as an iframe source, so it is parsed rather than trusted.

  Verified: `bun run check` green; `bun run typecheck` 0 errors (2 pre-existing
  a11y warnings in WorkchainCanvas, untouched); 1332 ui tests pass, 11 of them
  new and covering the embed mapping and the URL parser — including that a
  `docs.google.com.evil.test` link is refused. Every touched api crate parses
  and is formatted under `cargo fmt --check`; compilation is CI's.
