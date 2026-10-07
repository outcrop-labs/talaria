- **The daily brief's tiebreak can be asked instead of assumed.** The focus
  queue orders by `(bucket, due-ness, explicit priority, age)`,
  lexicographically. The **bucket** is sound and stays untouched: failed work
  outranks a waiting message because those are different *categories* of
  thing, not different intensities. The three fields after it are a tiebreak by
  accident — "due in six days, high priority, filed Tuesday" versus "due in
  eight days, urgent, filed this morning" is decided by which field happens to
  come first in the comparator, and the answer a person would give has nothing
  to do with that ordering.

  A **Score** is exactly that question: a probability-weighted position on five
  ordered levels, one per row, all in one request over shared state. The levels
  describe concrete situations — who is waiting, and what happens if the row
  slides — because a scale of adverbs gets answered as a vibe.

  **JUDGE WITHIN A BUCKET, NEVER ACROSS ONE**, and that is the property the
  tests are about rather than the ordering itself. Reordering across buckets
  would let a confident model drop a failed deploy below an unread mention, and
  no threshold makes that acceptable: the buckets are a policy decision a
  person already made. `apply_within_buckets` rebuilds the order by
  concatenating per-bucket runs, so the bucket sequence cannot change even if
  the reorder misbehaves, and a test drives a bucket-5 row scored **1.0**
  against bucket-0 rows scored **0.0** to prove it stays last.

  **All-or-nothing per bucket.** A bucket where any row lacks a usable judgment
  keeps its whole deterministic order — half-judged is ordered by neither
  policy and would read as a bug in whichever one the reader expected. And rows
  scored identically come back exactly as they went in, so a provider that
  answers everything the same leaves the brief byte-for-byte unchanged. That is
  the property that makes this safe to ship: the judgment only ever breaks ties
  the old comparator was breaking arbitrarily.

  **It measures with the site switched off**, which is the default and the same
  bargain the ticket gate makes. The question is asked, the order it *would*
  have produced is computed, one comparison is recorded — did the judgment
  change what the person sees **first**, which is the only question worth
  asking of an ordering — and the deterministic order is what the brief gets. A
  reorder that shuffles rows 6 through 9 is not worth a provider call, and a
  ledger that said "the order differed" without saying where could not tell the
  two apart. An install with no provider configured pays nothing: `decide`
  answers `None` before a request is built.

  **Capped at 40 rows.** A brief's sources return up to 200 each; one Score per
  row over shared state would push a single request past any provider's
  context window, and nobody reads row 90 and wonders whether it should have
  been row 85. The top of the deterministic order is judged; the rest keeps it.

  The question and the reading live in `talaria_decide::focus`, generic over the
  item type — `(key, state)` in, urgency per key out — because **two** crates
  hold a focus sort over two different item types (`talaria-daily-brief-focus`
  for the daily brief, `talaria-inbox-focus::policy` for the live queue), and a
  question built in each is how the two come to judge different things while
  claiming to judge one. Only the daily brief is wired here; the live queue can
  adopt the same question without a second one.

  **The census cross-check got sharper on the way.** `decide-site-not-in-census`
  only matched a bare identifier, so `site: talaria_decide::focus::SITE` was
  invisible to it. It now resolves a path-qualified constant through the module
  the path names — and deliberately *not* through a global name→value map,
  because `SITE` is declared in more than one module with different values and
  a global map would let an uncensused `SITE` pass on the strength of a
  censused one, which is the rule certifying the thing it exists to catch.

  Verified: 12 tests in `talaria-decide`'s focus module and 81 in the crate
  (5 new here — the levels are a well-formed Score and each one stands on its
  own, the top level normalizes to exactly 1.0 and nothing sorts off the end
  including NaN and infinity, a bucket missing one judgment changes nothing,
  equal urgency preserves the incoming order in both directions, and the site
  reads its floor as certainty because a Score has no probability of yes to
  lean on); 9 in `talaria-daily-brief-focus` (3 new — no row crosses a bucket
  boundary however urgent, the reorder is a permutation with nothing lost or
  duplicated, the judgment breaks ties within a bucket, and each bucket is
  judged on its own so one missing score does not freeze its neighbours). The
  widened invariant was proven to be a gate by typoing the module constant and
  watching `bun run check` fail with `site: "focus-rank-typo" — not in
  DECIDE_SITES`. clippy clean across `talaria-decide`,
  `talaria-daily-brief-focus` and `talaria-daily-brief`; `bun run gate` green.

  **Not exercised live.** No real Score has ranked a real brief; the site is
  off by default and its 0.60 floor is reasoned from the asymmetry (a wrong
  order on a page read top to bottom costs seconds of scanning, and every row
  is still there) rather than measured.
