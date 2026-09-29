- **Every queued Google write now asks for approval where you are reading, and
  agents can work in Sheets and read Slides.** Three things, one seam.

  **The approval cards were missing for most of the queue.** A queued Google
  write has always answered with a pending action, but only two tools —
  `draft_email` and `draft_calendar_event` — turned that into a card in the
  conversation. Editing or appending to a Doc, moving or renaming a Drive
  file, rescheduling or cancelling an event, and creating a meeting all queued
  correctly and then surfaced nowhere: the only way to find them was to ask
  the agent to list pending sends. All seven now raise the same inline card
  the other two always did.

  **Worse, approving one of those cards would not have sent it.** The approve
  route matched two kinds by name and let everything else fall through a
  catch-all that reported success without doing anything — so a card for a Doc
  edit would have cleared, told the person it was approved, and left Google
  untouched. Approval now routes every Google kind through the confirm-send
  plane that already knew how to execute all eight of them, and an approval
  whose kind has no execution path behind it is refused and logged instead of
  silently succeeding.

  **Google Sheets, read and write.** `read_google_sheet` returns a
  spreadsheet's title, its tab names, the A1 range the rows actually came
  from, and the cells; `update_google_sheet` writes a rectangle back and —
  like every other agent write to Google — queues for a human first. Writes go
  through the Sheets values API rather than re-uploading a CSV, so only the
  range you name changes: other tabs, formatting, and formulas outside it are
  left alone. Cells are interpreted the way a typed cell is, so a `=SUM(...)`
  becomes a formula. Two pending writes to different ranges of the same sheet
  are two cards; a repeat of the same range is recognised as a retry.

  **Google Slides, read.** `read_google_slides` returns a deck slide by slide
  — every text run on each slide plus its speaker notes, which do not bleed
  into the slide body. Reading only: editing a deck means batched requests
  against shape and placeholder ids, and a tool that did not understand
  layouts would produce decks nobody wants. Importing a deck as a PDF artifact
  still works as it did.

  Neither new surface needs anyone to reconnect: the Sheets and Slides APIs
  both accept the full Drive scope every Talaria connection already carries.

  Verified: `bun run check` green (invariants, doc links, generated-reference
  drift after `bun run docs:api`, mcp route resolution — 80 call sites against
  280 routes, changelog, cleanup sweep); `bun run typecheck` 0 errors (2
  pre-existing a11y warnings in WorkchainCanvas, untouched); `cargo check`
  clean on every touched api crate and the new Slides unit tests pass. The
  local cargo gate (fmt, clippy, the full package tests) was deliberately not
  run — CI's api job covers it.
