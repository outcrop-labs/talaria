- **Workchains: undo, snapping, splice-on-drop — and the canvas stops carrying
  warnings.** An audit pass over the node editor: what it was missing, what it
  was getting away with, and what a reader could not put back.

  **Undo.** A graph editor without one is a graph editor you edit nervously.
  ⌘/Ctrl+Z puts back the canvas's destructive verbs — a cut wire, a whole card's
  wires cut at once, a Tidy Up that rearranged everything, a card dragged
  somewhere you did not mean — and both the canvas and the wire context menus
  name it (`Undo wire cut`) rather than leaving it a keyboard secret. It is
  deliberately session-local and shallow: the truth is the server's, several
  people share a chain, and a deep stack replayed over someone else's edits
  would put back a world nobody was in. The stack empties when you switch
  chains, because a different graph is a different history.

  **Cards snap to the grid.** The canvas has drawn a dot field since the
  wiring editor landed and nothing lined up with it. A dragged card now rests
  on the nearest intersection, which is most of what makes a hand-arranged
  canvas look arranged; Alt drops the snap for the one card that wants to sit
  between the dots. Tidy Up's grid already agreed with the field, so the two
  layouts finally meet.

  **A ticket dropped ON a wire splices into it.** Drag a row out of the panel,
  hold it over a connection — the wire lights up instead of the landing ghost —
  and let go: A → new → B, with the A → B you dropped onto cut. (The api's
  `after:` wedge is the other shape: it moves EVERY one of the anchor's
  successors onto the new step, which is right for "insert into a line" and
  wrong for "insert into this one wire". Three explicit edges say what the
  gesture meant, and the two adds land before the cut so a failure part-way
  leaves more structure than you drew, never a chain severed in the middle.)

  **Three bugs the pointer layer was getting away with.** A `pointercancel`
  mid-drag — a touch that turned into a scroll, a card removed by a refetch —
  left `dragId` set for ever, and the card stayed pinned to a position nothing
  was updating; it now drops the drag cleanly. The click-swallowing flag that
  keeps a drag from opening the ticket was never cleared when no click followed,
  so the reader's next honest click on that card was eaten; a new gesture clears
  it. And `endDrag` ignored which pointer was ending, so a second finger could
  end the first one's drag.

  **The canvas carries no lint.** Its two a11y warnings are gone rather than
  tolerated: the surface's `tabindex` is now an explicit scoped ignore that says
  WHY (`role=application` is a focus stop on purpose — space to pan, Delete to
  cut, ⌘/Ctrl+Z to undo are unreachable otherwise), and the wire's hit plane
  lost the redundant `onclick` that raised the other one. Selection already had
  one writer on pointerdown; now it has only one.

  **The pinnings.** The new logic is pure and tested rather than buried in the
  component: `snapToGrid`, `nearestWire` (one walk, shared by wire selection and
  splice targeting — they agreed by accident while the loop lived inside the
  component), `spliceIntoWire` and `invertWireOps`, which is what makes undo's
  plan testable without a server. `findEdge`, `wireMidpoint` and `chainBranches`
  go: nothing had called them since the rails left the lens, and a dead export
  with a test around it reads like API. `successorsOf` drops to module-private.

  Verified: `bun run gate` exit 0 — check, **svelte-check 0 errors and 0
  warnings**, 1332 ui tests across 82 files (no Rust surface in the diff, so no
  cargo). Driven in a real browser against a dev stack: cutting a wire and
  pressing ⌘/Ctrl+Z restores exactly the edge that went (`WL-4→WL-12` back after
  the cut), the canvas menu offers `Undo wire cut` and does the same, Tidy Up
  changes the layout and undo puts every card back; a dragged card lands on the
  24px grid (`WL-1(72,48)`) and its move undoes to the previous spot; a ticket
  dragged from the panel onto the `WL-3→WL-4` wire lights that wire, suppresses
  the landing ghost, and lands as `WL-3→WL-8 WL-8→WL-4` with `WL-3→WL-4` gone —
  writes in the planned order, both adds before the cut. The earlier suites
  re-run green on top: right-click still opens the menu and creates nothing, the
  composer still dismisses on an outside click and on Escape, a plain drop still
  lands unwired at the drop point.
