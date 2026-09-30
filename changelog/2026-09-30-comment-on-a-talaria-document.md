- **You can comment on a Talaria document, and on the one a work session is
  building.** Comment threads existed only for knowledge-base docs; the
  documents agents actually produce — and the living document beside a work
  session — had nowhere to say "this paragraph is wrong". They do now, with the
  same threads, quote anchors, resolve rule and notifications the knowledge base
  has always had.

  The same table, not a second one. A comment is a comment: a parallel
  `artifact_comments` would have duplicated the thread shape, the resolve rule
  and every route that reads them, and then drifted. `doc_id` became nullable,
  `artifact_id` joined it, and a constraint holds exactly one of them — a row
  pointing at both, or at neither, is refused rather than stored and rendered
  somewhere strange.

  In a work session the comments swap into the document pane rather than sitting
  beside it: the pane is 44% of the stage and splitting it again would leave
  neither half usable. A pinned Google file has no Talaria comments at all,
  deliberately — Google's own comments live inside the embed, and a second
  comment system on the same paragraph would be a worse answer than either.

  One narrower rule than the knowledge base, stated rather than discovered: a
  knowledge doc's owner can resolve a thread they did not start, and an
  artifact's cannot. Reading an artifact from inside the comment engine would be
  a dependency cycle, so the author and the thread starter can resolve, and an
  owner who wants a thread gone deletes it through the route that already gates
  on the artifact.

  Verified: `bun run check` green (285 routes, generated reference regenerated);
  `bun run typecheck` 0 errors (2 pre-existing a11y warnings in WorkchainCanvas,
  untouched); 1332 ui tests pass; every touched api crate parses and is
  formatted under `cargo fmt --check`. Compilation is CI's.
