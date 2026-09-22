- **W8 (part 1): one stale-board banner, one page skeleton.** The board's three
  lenses each wrote the same "a reference read failed, the tickets did not"
  banner — list, kanban and gantt agreed on the shape and disagreed, twice, on
  the sentence. `components/board/StaleBoardNotice.svelte` is that banner: it
  takes the statuses read, an optional labels read, the word the lens uses
  ("Columns" in kanban) and, for gantt, its own sentence for "the status set
  never arrived"; everything else — which read is blamed, what the retry
  refetches, and the rule that a failed labels read only costs tint — is in one
  place. And `routes/app/ArtifactPageSkeleton.svelte` is gone: it was
  `KbDocPageSkeleton` with the prose widths frozen, and only the widths had
  drifted (58% where knowledge's list lands 88% on the tenth bar).
  `ArtifactEditor` renders `<KbDocPageSkeleton bars={10} />` — knowledge's
  widths, deliberately.

  Verified in the running app, not inferred: with `/api/boards/<id>/labels`
  aborted, the extracted banner renders "Could not load labels…" **and the board
  still draws its 5 tickets across 6 columns** (the mark-don't-replace rule); and
  the artifact editor's cold load renders exactly **10** prose bars with the
  detail read delayed, which is the geometry the deleted skeleton had.
  `bun run verify` green (1,186 tests), `bun run check` green (15 rules, 0
  duplicate clusters).
