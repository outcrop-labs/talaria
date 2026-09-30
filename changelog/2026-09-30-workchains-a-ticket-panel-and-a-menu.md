- **Workchains: a ticket panel you drag out of, and a canvas that answers a
  right-click.** Four complaints from driving the lens, all of them fair.

  **The new-ticket composer could not be dismissed.** It closed on exactly two
  things: Escape while its own input held focus, and a 9px `esc` link. Click
  anywhere else — the canvas, a card, the toolbar — and it stayed open with no
  way back, because the surface's keydown returns early while it is open (so
  the input can own Space and Delete). It now closes on a document mousedown
  outside it (the same test Popover and DropdownMenu make) and on Escape
  wherever the focus is, and it carries a real ✕.

  **A bare gesture created tickets; a right-click did nothing.** Double-click
  on empty canvas opened the composer with no menu, no affordance and — per
  the above — no exit. Right-click now opens a proper context menu: *New ticket
  here*, *Add an existing ticket*, *Zoom to fit*, *Tidy up*. Double-click stays
  as the accelerator, which is the only reason it was there (a chain with no
  steps has no out-port to drag from, so without it the composer is
  unreachable), but it is no longer the only way in or a trap once you are.

  **Adding tickets was a popover over a flat list.** Fine for six tickets,
  unusable for sixty. It is now a pop-out **ticket panel** down the left of the
  lens: searchable, filterable, and a drag source — a row drags onto the canvas
  and the card lands where it is dropped, unwired, because the gesture already
  said where and a volunteered tail edge would be a predecessor nobody asked
  for. Clicking a row still appends to the chain (the api wires it to the tail
  — the pipeline reading). The panel collapses to a `Tickets n` button in the
  toolbar, remembered per board. The house pattern is the Gantt chart's
  unscheduled list, typed dataTransfer payload and all.

  **The board's filter row moved into that panel** while this lens is up. Those
  facets filter the ticket list, not the graph, and the list was no longer on
  screen with them; meanwhile the row they sat in was the only horizontal space
  the chain's own verbs had. `Board.svelte` hides its query row for this view
  and hands the state down — the route still owns the URL encoding, so search
  and facets keep round-tripping through it exactly as before.

  **The workchain menu was barebones.** It now carries the chain's progress on
  every row (switching chains is a decision; `3/7` is the fact it turns on),
  plus **Copy link to this workchain** — the focused chain lives in the URL
  (`?chain=`) now, so a chain is a thing you can send someone, the same deep-link
  rule every other selection on this board follows — and **Straighten into a
  single line**, which is the `positions` verb the rail used to own and which
  became unreachable when the rail left the lens. It is the only way back from a
  tangle to a pipeline, and its confirm names the real cost ("4 wire(s) replaced
  by 3"). It disables itself on a chain that is already a line.

  **Finished tickets never appear in the panel.** You do not queue work that is
  already over, and the picker filling up with it was half of what made it hard
  to sift. The predicate is the board's own `isClosedStatus` — the done
  category, the bare `done` key, and the off-board terminals (failed,
  cancelled) — so the panel cannot end up with a second opinion about what
  closed means. It holds whatever the filters say: selecting the Done facet
  does not talk them back in, because "not in the picker" is a property of the
  ticket, not a view the reader can argue with. The count being held back rides
  under the list (`3 finished hidden`), and the empty state says which kind of
  empty it is.

  `filterCandidates` and `WorkchainCandidate` go with the popover they served.

  Verified: `bun run gate` exit 0 (check, svelte-check 0 errors with the 2
  pre-existing a11y warnings, 1323 ui tests across 82 files; no Rust surface
  in the diff, so no cargo). Driven in a real browser against a dev stack:
  right-click on empty canvas opens the menu and creates nothing; *New ticket
  here* opens the composer, which then closes on an outside click AND on Escape
  with focus moved to the panel's search box; a row dragged from the panel onto
  the canvas fires `POST /steps {wire:false}` then `PATCH nodes` with the drop
  coordinates, and the card renders at the drop point as its own head; the
  panel's search narrows the list and round-trips through `?q=`; the facets
  render in the column; the panel hides to a toolbar button and comes back;
  Straighten turns `WL-1→WL-2 WL-2→WL-3 WL-3→WL-4 WL-1→WL-3` into the three-wire
  line after a confirm that counts the wires; Copy link yields
  `?view=workchains&chain=…` and opening it lands on that chain (and a second
  link on the other one). Marking two tickets `done` and one `cancelled` drops
  all three out of the panel (`WL-8 WL-7 WL-6 WL-5` left, `3 finished hidden`
  under the list), and neither the Done status facet nor the archived chip puts
  any of them back — both leave the list empty.
