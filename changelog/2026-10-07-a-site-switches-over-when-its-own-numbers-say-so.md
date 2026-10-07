- **A decision site can now be switched over from the panel, at a threshold an
  operator sets, beside the numbers that justify it.** Shadow mode's whole
  argument is that a site adopts the port when *its own traffic* says so — but
  until now the switch was a `const f64` in whichever crate held the call site
  (`SEMANTIC_FLOOR` in `talaria-workflows`, `ALIGN_FLOOR` in `talaria-gaps`,
  `KEEP_FLOOR` in the tool pass), so reading the ledger and acting on it were
  separated by a pull request. Someone who can see that a site agreed with the
  existing code on 94% of 300 answers and can do nothing about it has been
  given a dashboard, not a feature.

  **`talaria_decide::sites` is the census**: every place in Talaria that asks a
  decision model anything, what it does with the answer, where it is switched
  on, and the floor it acts at. Six entries, and the three the panel does not
  own are listed anyway, each naming the surface that does — the guardrail rule
  toggles, the rerank provider picker, the tool-shadow consent switch. "Where
  is a decision model being used" has to have one complete answer, or the
  answer is "nobody is sure". `set_site` refuses the ones it does not own
  rather than offering a second spelling of the same switch.

  Each floor keeps the cost asymmetry that chose it, as prose next to the
  number, because that asymmetry is the only honest way to pick a threshold
  before there are numbers — and because an operator moving one should see what
  they are trading.

  **A FLOOR IS NOT ONE THING, and conflating the two readings would have been a
  silent regression in both directions at once.** `workflow-match` gated on
  `probability >= 0.75`; `gap-align` on `certainty >= 0.85`. Routing both
  through one `gated()` call would have moved the workflow bar from p ≥ 0.75 to
  p ≥ 0.875 *and* started pulling a workflow in on a **confident no**, because
  distance from the middle cannot tell the two ends apart and the call site
  only checks whether a judgment came back. So the census declares how each
  site reads its floor: `FLOOR_LEAN` for a one-directional question where a
  confident no means "do nothing", `FLOOR_CERTAINTY` for a two-sided one — and
  for every Choice or Score, which have no "probability of yes" to lean on at
  all. Four of the six lean. Every site's reading and default are pinned by a
  test naming the site, not spot-checked.

  **The ticket gate becomes a cascade, which is the one switch that changes
  what gets spent.** Off (the default) it is unchanged: the LLM harness decides
  and the port is asked the same question detached with nobody listening —
  double cost, which is the price of the measurement and a reason not to leave
  a site there forever. On, the port is asked **first** and a confident answer
  is taken, skipping the harness turn entirely, which is the gate's own stated
  economics ("cheaper than the reply it prevents"). An **uncertain** answer
  falls through to the harness, so the expensive judge still runs on exactly
  the messages worth running it on. Fail-open is preserved exactly: the floor
  is read as certainty, so the whole uncertain middle still answers true.

  One consequence worth naming: a turn the port decides writes no
  `harness_runs` row, because no harness ran. `shadow::record_acted` is the
  replacement trace.

  **`shadow_report` grew a third denominator.** A judgment a site *acted* on
  has no baseline — the path it replaced stopped running — so folding those in
  as agreements would make every switched-over site read as 100% agreement
  forever, which is the one number that must not be invented here. Rows with an
  empty baseline are counted as `acted`, the agreement rate stays over
  `answered`, and the panel says which is which. The three insert sites in
  `shadow.rs` collapsed into one private row builder on the way, which removes
  the duplication `record` and `record_derived` already had.

  **A new `bun run check` rule: `decide-site-not-in-census`.** A call site
  recording under an id the census does not declare is the worst of both
  worlds — it costs a judgment per call and can never be switched on, because
  `gate_of` answers `{ on: false, floor: 1.0 }` for an id it does not know —
  and nothing fails, because the call site falls back exactly as it did before.
  Same class as the nine cross-checks that were all failing unrun.

  Verified: 76 tests in `talaria-decide` (7 new — the census is internally
  consistent, an absent row and an absent field both take the registered
  default, a nonsense floor does not become one, an unknown id cannot be
  switched on by a typo, a lean floor rejects a confident no, a certainty floor
  acts at both ends, and every site's reading and default are pinned by name);
  12 in `talaria-routes-admin`, 10 in `talaria-workflows`, 5 in `talaria-gaps`,
  2 in `talaria-jobs`. The new invariant was proven to be a gate by renaming a
  census id and watching it fail with `site: "gap-align" — not in
  DECIDE_SITES`. clippy clean across all five crates; `bun run typecheck` 0
  errors; `bun run gate` green.

  **Not yet exercised live.** The cascade's acting path has never run against a
  real decision provider on real ticket traffic — the sites it switches are the
  ones whose thresholds have no measurements behind them yet, which is the
  reason this exists rather than a reason to doubt it.
