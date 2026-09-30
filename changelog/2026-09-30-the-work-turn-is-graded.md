- **The Work surface's turn is graded like every other agent job.** A new
  harness in the fitness suite measures what a model does in a work session, so
  a candidate model is asked the question this surface actually asks before it
  is trusted with somebody's documents.

  It grades the two things the Work prompt asks that nothing else in the suite
  asks. The first is restraint about output: the pane is showing the file, so a
  model that makes the edit AND pastes the new section into the chat has done
  the work and made the surface worse — the one sentence the person needed is
  buried under a copy of what they are already looking at. The second is the
  queued/immediate split under this prompt specifically, including the case
  where a model is too cautious: a document the agent just created is its own to
  edit, and calling that "queued for approval" sends someone looking for a card
  that will never appear. A fourth fixture covers the deck nobody can edit —
  the right answer is to hand over the words and say plainly that a person has
  to paste them.

  The harness uses the product's own prompt rather than a copy of it, so it
  cannot keep passing while the surface drifts underneath it — and a test
  asserts exactly that.

  Verified: `bun run check` green; the new crate parses and is formatted under
  `cargo fmt --check`, and the harness id is unique against the registry's own
  duplicate guard. Compilation and the eval replay are CI's.
