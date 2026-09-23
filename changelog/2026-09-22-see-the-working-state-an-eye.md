- **See the working state: an eye beside the link, a dither trace around the
  card.** Every copy-link affordance on a ticket — kanban card, list row,
  ticket header — now carries a hover-revealed eye that opens the watch modal
  (replacing the card's old sliding watch chip), and a ticket with live work
  wears a two-pixel dither border tracing its edges — the same canvas
  material as the work indicator, masked to the ring — so the card reads
  "something is happening here" from across the board.
  Verified: in the browser against a live session — the eye renders beside
  every copy-link site and opens the run detail, the dither ring was checked
  in headless Chromium (stripe follows the rounded edges, clean interior),
  and `bun run typecheck` + the full `bun run verify` gate are green.
