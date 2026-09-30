- **A work session's turns now carry a harness, and the approval queue is
  graded.** The Work surface was the only conversational surface with no mode
  prompt at all: `kind='plan'` turns carry one, `kind='research'` turns carry
  one, and `kind='work'` carried nothing — so the agent did not know it was on
  the Work surface, did not know a document sat in a pane beside the chat, and
  did not know which of its writes queue behind a human. That was the least
  instructed surface in the product and the one acting on people's real
  documents.

  The work-mode prompt spells the queued/immediate split out tool by tool
  rather than summarising it as "writes queue", because it genuinely is not
  uniform and rounding it off is wrong in both directions. Round toward
  "everything queues" and the agent reports a doc it really did create as
  merely pending, and the person waits for an approval that never comes. Round
  toward "I can act" and it says a spreadsheet was updated while the write sits
  in a queue. It is also precise about Slides, where the line
  is narrow: an agent can replace a deck's words and cannot restyle or extend
  it, so a request to reword is work it does and a request for a new slide is
  one it declines plainly.

  Four new fixtures grade the queue in the Google eval group, which previously
  covered only a queued Doc edit. Three ask the question the rest of that group
  asks — did the model over-claim — for the writes that had no fixture: a Drive
  move or rename, a cancellation (where the attendees are told only after
  approval, so "I've cancelled it and let them know" is two false statements in
  one breath), and a meeting (where quoting a Meet link that does not exist yet
  is the invented-link failure in a new hat).

  The fourth is the first fixture in that group that fails a model for being
  **too** careful. A Google Doc the agent created is immediately its own to
  edit — the tool result says so in as many words — and a model that has
  learned "Google writes wait for a human" hedges anyway. That hedge sends the
  person hunting for an approval card that will never appear, which is a
  sentence that was simply untrue.

  Verified: `bun run check` green; rustfmt applied; the prompt carries unit
  tests that assert the facts inside the prose rather than its wording, so a
  tool moving between the immediate and queued lists fails a test rather than
  quietly misinforming an agent. Compilation and the eval replay are CI's.
