- **Workchains: the node editor actually edits.** The canvas had every piece
  of TALA-34/TALA-35 and almost none of them survived contact with a pointer.
  Dragging a card fired the click the gesture began, so every reposition ended
  in the ticket overlay. The card tracked screen pixels, not canvas units, so
  at any zoom but 1:1 it slid away from the cursor. Dragging the blank space
  BETWEEN cards refused to pan — the empty-canvas test compared `event.target`
  against two elements that the plane's own sized child covered, so the one
  gesture a reader tries first did nothing. Auto-layout levelled only the
  UNPLACED steps, which collapsed every placed predecessor to column 0: moving
  one card re-columned everything downstream of it. A chain opened at
  translate(0,0) scale(1), so its entry card sat flush in the frame's corner
  and anything previously dragged out of the default column was off screen.
  Wires drew in a hairline token on the canvas ground with no arrowhead —
  near-invisible, and directionless where they were visible. A card dragged
  left of the origin had its wires clipped by an `inset-0` svg sized from a
  positives-only box. And the create-and-connect drop threw the drop point
  away, so the new card appeared wherever the grid put it, wired both to the
  card you dragged from AND to the chain's tail, because the api auto-wires
  every append.

  All of that is fixed. A drag moves the card and only the card; a click still
  opens the ticket. Panning is "anything that is not a card, a port or a wire".
  The grid is computed for every step from the graph alone — placing a card
  moves nothing else — and **Tidy up** (the wand floating over the canvas)
  re-lays the whole chain on it and persists it. Opening a chain fits it. Wires carry
  arrowheads and the strong hairline, lifting to accent on hover; the canvas
  has a dot grid that pans and zooms with the graph, and agents' avatars wear
  an accent ring so a chain's human/agent mix reads at a glance. The drop point
  is where the new card lands, negative coordinates included — the svg is laid
  over the graph's real box now. **A double-click on empty canvas opens the
  same composer with no wire**, which is how a chain with no steps gets its
  first one: there was no out-port to drag from, so the acceptance gesture
  ("authored entirely by dragging, no picker touched") could not start.

  `POST /api/workchains/{id}/steps` takes `wire: false`: the step lands
  unwired. Appends still wire to the tail by default — that is what every
  picker means by "add to this chain" — but the canvas knows the one wire it
  drew, and the volunteered tail edge was an invisible second predecessor that
  derived the new step BLOCKED behind unrelated work. Two other repairs
  alongside it: the wedge insert's position shift ran on the pool instead of
  the transaction, so a `after`-names-a-foreign-step 400 answered having
  already pushed every later position up by one; and the task drawer's
  move-earlier/move-later arrows now only appear on a chain that is already a
  line (`chainIsLinear`) — they ship the linear `positions` payload, which the
  api answers by rewriting the chain's edges to ONE line, so a nudge on a
  branched chain silently flattened every fan-out and join. `WorkchainRail.svelte`
  goes: nothing has imported it since the rail left the lens.

  The lens around it is rebuilt to match. ONE chain fills the view: a toolbar
  line, and the canvas takes every pixel under it — the UNCHAINED list that
  used to stack below is gone, and with it the page scroll that put a wiring
  editor behind a scrollbar. The toolbar carries the lens's two menus. The
  **workchain menu** (its trigger names the focused chain) switches between
  chains and holds every verb that acts on one: New workchain, Rename, Pause /
  Resume, Delete — rather than a switcher in one place and half the verbs on a
  canvas header. Creating one FOCUSES it: naming a chain is asking to work on
  it, and a create that only moved the menu's `1/2` read left the reader
  looking at the chain they already had. **Add tickets** is the old UNCHAINED
  set as a searchable menu, its count on the trigger so work sitting outside
  every chain still announces itself; a row click appends it and leaves the menu open (seeding a pipeline
  is a run of picks), and the canvas re-fits so the reader sees where the card
  landed. The canvas keeps only what needs canvas state — zoom, fit, Tidy up —
  floating over its own bottom-left corner.

  Verified: `bun run verify` green end to end (check, svelte-check 0 errors
  with the 2 pre-existing a11y warnings, 1316 ui tests across 82 files);
  `cargo fmt --check` and `clippy -p talaria-routes-boards --lib -D warnings`
  green; the `workchains_live` router suite run against a real dev Postgres +
  Redis, including two new cases (`wire: false` lands a step with no edges and
  derives head, and a refused wedge leaves every position alone). Driven in a
  real browser against the dev stack: the acceptance gesture end to end on an
  EMPTY chain — double-click seeds A, two out-port drags to empty canvas
  compose B and C, and the read comes back with exactly `A→B, A→C` and nothing
  volunteered; landing A in a done column turns both wires gold-dashed and both
  successors READY (the fan-out); a card drag at zoom 2.16 tracks the cursor
  1:1 and leaves no overlay open; an empty-canvas drag pans; Tidy up rewrites
  every node's position; wire click selects and Delete cuts; the lens has no
  scrolling container left (the canvas measures 729px of a 1000px viewport,
  the rest being the board's own chrome); the workchain menu switches, creates,
  renames, pauses and deletes; Add tickets searches the ten unchained tickets
  and a pick appends one, leaving the menu open and the canvas re-fitted;
  switching to a chain placed 2400px off the origin lands every card inside
  the frame.
