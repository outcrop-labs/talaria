- **An agent can now change part of a document instead of rewriting the whole
  thing.** `update_document` took *"New **full** markdown body"*, so every
  change an agent made to a document was a regeneration of all of it. That is
  three bad things at once: a one-line fix is billed for the entire document,
  any section the model does not bother to re-emit is **silently dropped**, and
  whatever a person typed into the same document while the agent was thinking is
  **overwritten**. `edit_document` takes the edit instead of the result.

  Two forms per edit. `oldString`/`newString` replaces exact text and
  **must match exactly once** — zero matches and many matches are both refused,
  neither softened into "do the first one", because a tool that guesses which
  paragraph you meant eventually rewrites the wrong one and the model has no way
  to find out. `section`/`markdown` rewrites the body under a markdown heading
  and keeps the heading line, which is the form prose actually needs: quoting a
  whole paragraph back exactly is the kind of thing a model gets subtly wrong,
  and a heading is an address it cannot typo invisibly. Edits apply in order, so
  one call can fix two things, and **a failure anywhere applies none of them** —
  a half-applied batch leaves a document in a state neither the model nor the
  person asked for.

  **The refusals are written to be read by the model that has to recover**,
  because that string is the entire tool result it gets back. An ambiguous
  anchor says how many times it matched and to add surrounding context; a
  missing one says to re-read and quote exactly. A missed heading **names the
  headings the document actually has** (capped at 24) rather than telling the
  agent to go and look — the recovery is the message, and "read the document"
  spends a turn to learn what we already had in hand.

  Three smaller decisions worth knowing. A **sheet is refused**: its body is
  `JSON.stringify(rows)`, so a find-and-replace inside it would corrupt the grid
  rather than edit a cell, and the refusal names `update_document with rows`
  instead. A **CRLF document matches a needle written with plain newlines** —
  the needle is expanded to the document's own line endings rather than the
  document being normalized, so an edit cannot quietly rewrite every line it did
  not touch. And **an edit that changes nothing does not save**, because every
  content change snapshots a version and the version history is what a person
  reads to see what the agent did.

  `update_document` stays, for replacing a document wholesale and for writing a
  spreadsheet grid — and its description, plus `create_document`'s, now points
  at `edit_document` so a model reaching for the full rewrite is told the cheap
  tool exists.

  **The authority and the reindex are the PUT's own, shared rather than
  reimplemented.** `body_editor` ("may this caller rewrite this artifact's body,
  and under what name is it stamped") and `reindex_content` came out of
  `artifacts_id.rs` and are now called by both routes. Two answers to "may this
  person edit this document" is how the tool plane and the UI come to disagree,
  and an edit that skipped the reindex would leave agents retrieving a paragraph
  the document no longer contains.

  The matching engine is its own dependency-free crate (`talaria-doc-edit`) for
  a reason: the rules are what decide whether a model can use the tool at all,
  so they want tests that compile in a second rather than tests behind sqlx. The
  **fitness sandbox calls that same engine**, so a candidate model is graded
  against the real match-once rule and the real refusal sentences — an
  approximation kinder than production would certify models that cannot actually
  use the tool, which is the exact flattery the toolbox invariants exist to stop.

  No decision-model site was added here, deliberately. The one place a judgment
  looked tempting — picking which of several matches or which heading was meant
  — is the one place acting on a guess silently rewrites the wrong text, so the
  ambiguity is reported and the headings are listed instead. Code owns what code
  can do.

  Verified: 31 tests in `talaria-doc-edit` (the match-once boundaries, the
  all-or-nothing batch, section ranges stopping at a sibling and swallowing a
  deeper heading, a `#` inside a fenced block not being a heading, a re-emitted
  heading not written twice, the CRLF retry not firing when the literal form
  already matches, and a multi-byte needle not panicking when a refusal clips
  it); 7 more on the shared wire reader; 10 in `talaria-routes-knowledge`; 74 in
  `talaria-fitness-toolbox` (two new — a real edit and the spreadsheet refusal,
  both asserting a refused edit leaves the version untouched); 9 in
  `talaria-fitness-talaria-tools`, where the four sync invariants hold the
  catalogue to the toolkit's real description, argument names and required set.
  `bun run docs:api` regenerated — and `gen-docs` learned `body_editor` is a
  dual guard rather than emitting `unknown(body_editor)` into the reference,
  the same explicit entry `acting_user` already has for the same reason. clippy
  clean across every touched crate; `bun run gate` green.
