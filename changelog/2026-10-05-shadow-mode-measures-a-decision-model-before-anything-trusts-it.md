- **Shadow mode: a decision model gets measured on your own traffic before
  anything trusts it.** The decision port shipped with no consumers on purpose.
  This is how a site acquires one — not by someone deciding the model looks good
  in a fixture, but by the existing code continuing to decide exactly as it
  always did while the port is asked the same question with nobody listening.
  Both answers land in `decide_shadow` with the probability and the latency, and
  a threshold is then chosen from what this install actually saw.

  `shadow::compare` takes the answer the site **already produced** and returns
  nothing. There is no value a caller could accidentally act on, which is the
  whole design: shadow mode that can influence the decision is not shadow mode.
  It runs detached, so a slow or unreachable provider costs the request nothing,
  and it never surfaces an error, because a failed measurement must not become a
  failed feature.

  **The ticket-thread gate is the first wired site**, and the two askers share
  one definition of the question. `RELEVANT_MEANS` and `NOT_RELEVANT_MEANS` were
  promoted out of the harness's prompt into public consts that both the prompt
  and the shadow's yes/no primitive read — a comparison between two
  differently-worded questions measures the wording, not the model.

  Two things the ledger is careful about. **The silences are in the
  denominator**: `compared` counts every attempt and `answered` only the ones
  where the port replied, so an agreement rate cannot be quietly computed over
  just the calls that worked. And **the text that was judged is not recorded** —
  a ticket message is somebody's words, and a comparison ledger is the wrong
  place to accumulate them; `subject_ref` points at the row an operator can open
  under the permissions that row already has.

  Admin → Agents reads it back per site: agreement rate, how many answers
  carried no real distribution, median and p99 latency, and the one number a
  threshold is actually chosen from — mean certainty **on the comparisons where
  the port disagreed**. A provider whose disagreements are its least certain
  answers is one a threshold can filter; a provider that disagrees confidently
  is one to read case by case before switching anything over.

  Verified: 6 new unit tests in `talaria-decide::shadow` (the rendering and
  agreement rules per primitive, including that a choice answer never reads as
  agreement through the yes/no comparator, and that `render` and `noul_agrees`
  agree about p=0.5), 54 passing in the crate overall; `cargo clippy -p
  talaria-decide -p talaria-jobs -p talaria-routes-admin --all-targets` clean;
  `cargo test -p talaria-jobs` 2 passed; `bun run typecheck` (svelte-check, 5432
  files) clean. The migration was applied by the real runner on a worktree stack
  (415 statements total), and the aggregate query was run against ten
  representative rows — five agreements, three disagreements and two silences —
  confirming `compared` 10 / `answered` 8 / `agreed` 5, p50 115ms, p99 480ms,
  and a disagreement certainty of 0.593 averaged over only the real
  disagreements rather than over the silences.
