- **The ticket drafter knows the board it is drafting for.** "Draft tickets"
  turned a plan into a flat list from the conversation alone: it could not
  know that half the work was already an open ticket, who on the board does
  what, or that four of its tickets were one deliverable. The reviewer found
  that out one card at a time, and the duplicates it filed were real tickets
  a person then had to close.

  The drafter is now shown the board — its open tickets (`id #no [status]
  title`), its labels, its members — rendered into the prompt the way the
  workflow map already is, and the proposal shape grew three fields:

  - **`parent`** — an *epic*. Three or more tickets that serve one deliverable
    make the deliverable its own ticket and point at it; one level only. The
    review shows the epic badge and "Part of …" on the children; creation
    files parents first and children with `parentId`, and a child whose
    parent the reviewer dropped files as a top-level ticket.
  - **`assignees`** — board members, by id, only when the discussion said who
    does it. They reach the ticket as `user:<id>`, the spelling a ticket
    uses for a person; a bare id would have read as an agent model.
  - **`duplicateOf`** — the open ticket this proposal would re-propose. The
    card says "Possible duplicate of #no …" with a link, and the proposal
    **starts unticked**: ticking it is the reviewer saying "no, this is
    different", which is the decision that was theirs all along.

  **Two readers keep the model honest.** The verify step reads the ids back
  out of the very block the model was shown: a `duplicateOf` naming no open
  ticket, an assignee naming no member, or a nested epic earns the repair
  turn rather than a card nobody can act on. And after the draft, the
  **`ticket-dupe` census site** (choice, certainty 0.85, on) judges each
  proposal the model left unflagged against the open tickets whose titles
  overlap it — "the same work, not the same area" — with the model's own
  silence as the shadow baseline; when it acts, the proposal is flagged and
  unticked the same way. The judgment is per proposal, so a ten-ticket
  draft on a busy board is up to ten calls; an empty board or no overlap
  costs nothing.

  Verified: 441 tests in `talaria-harness-defs` (+6: `parent` remapped like
  a dependency with `null` never read as index zero, a nested child lifted to
  its epic rather than refused, ids coerced as ids and not prose, the block's
  id parsers, every branch of the context check, a `duplicateOf` nothing on
  the board earning the repair turn); 12 fixtures
  across three bands (+2: the open ticket named instead of re-proposed; a
  four-part deliverable grouped under one epic); 416 in `talaria-fitness`
  unchanged; 14 in `talaria-runs-plan-draft` (+7: the overlap ranking, a
  model-flagged duplicate left alone, the unticked duplicate); 93 in
  `talaria-decide` including the census consistency tests; 3 in
  `talaria-channel-plan` (the block pinned character for character); 10 UI
  tests for the create order and the store's normalize; svelte-check clean.
  The review walk's Save body learned the three fields too — without that
  every autosave would have dropped the epic links and the duplicate flag.

  A live draft needs a model and the Z.ai account answers "insufficient
  balance" today, so the fitness fixtures are the proof of the prompt until
  it is recharged; the plumbing is proven by the tests above.
