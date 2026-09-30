- **An agent can change the words in a Slides deck — and still cannot build
  one.** `update_google_slides` replaces text across a deck: the number that
  moved, the date that slipped, a client's name misspelled on nine slides. Like
  every other write to someone's Google account it queues for a human first,
  raises an approval card in the conversation, and reports how many occurrences
  actually changed — zero being a real answer that means the text was not found,
  which the agent is told to report rather than call success.

  The narrowness is the design, not a milestone on the way to more. Slides edits
  through batched requests against shape and placeholder ids, so adding a bullet
  means knowing which placeholder on which layout it belongs to, and a tool that
  guessed would produce decks with text off the edge of the slide and boxes on
  top of each other. Replacing words needs none of that: it swaps strings inside
  boxes a designer already placed, so the deck keeps the shape its author gave
  it. A deck's LAYOUT belongs to whoever made it, and changing its words is a
  different act from building it.

  So the agent is told exactly where the line is, and graded on holding it: a
  request to reword is work it does, and a request for a new slide is one it
  declines plainly while handing over the copy to paste. The eval that used to
  check it never claimed to edit a deck now checks it never claims to have
  ADDED one — with the deck tools armed, so the refusal is a real choice rather
  than an empty toolbox.

  Verified: `bun run check` green (285 routes, generated reference regenerated);
  `bun run typecheck` 0 errors (2 pre-existing a11y warnings in WorkchainCanvas,
  untouched); 1332 ui tests pass; the Slides crate's own tests cover the reply
  accounting, including a batch where one phrase matched nothing; `cargo fmt
  --check` clean on every touched crate. Compilation is CI's.
