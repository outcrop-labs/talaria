- **A work session builds a living document as you talk, and every shared
  surface now shows who is here.** The document beside a work session is no
  longer something the agent has to be told to write: when a turn lands, the
  session's own document is rewritten from the conversation so far, the same
  way a plan's has always been. That is the point of the surface — you talk,
  and the thing you are making takes shape next to you — so it is the pane's
  default, and a file the agent opens or edits directly takes the pane over
  only while you are looking at it.

  A work document is NOT a plan and is not pushed into a plan's shape. It takes
  whatever form the work calls for — a memo, a brief, an analysis, notes — and
  it is seeded from nothing rather than from the agent's plan template, which
  would have started somebody's memo as a project-plan skeleton and then kept
  reconciling it against that shape. The org's ticket-routing map is offered to
  a plan's rewrite and withheld from a work session's, because appending an
  "Agent routing" section to a deliverable is nonsense. Everything that
  protects a plan's document from a bad rewrite — the whole-document contract,
  the anti-truncation and anti-gutting guard — protects this one unchanged,
  because those are properties of rewriting a document rather than of what the
  document is about.

  Research gained presence. It could already be shared with people and teams,
  but a member's avatar only ever meant "has access"; now a ring means someone
  is reading it right now, the same heartbeat plans and work sessions use.
  Sharing without presence is half a multiplayer surface.

  The Work view's nav icon is a notebook being written in rather than two
  panes — the living document is the point, and the arrangement is the least
  interesting thing about it.

  Verified: `bun run check` green (284 routes, generated reference
  regenerated); `bun run typecheck` 0 errors (2 pre-existing a11y warnings in
  WorkchainCanvas, untouched); 1321 ui tests pass; every touched api crate
  parses and is formatted under `cargo fmt --check`. Compilation is CI's.
