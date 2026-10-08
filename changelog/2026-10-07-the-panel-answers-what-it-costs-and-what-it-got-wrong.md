- **The decision-model panel now answers what the feature costs and what it got
  wrong, and the one control it could not see is visible from it.** An audit of
  the control surface turned up seven gaps; six are closed here and the seventh
  is stated rather than papered over.

  **1 · NOTHING WAS METERED, and this is the worst of the seven.** These calls
  deliberately do not go through the metered relay — a decision is not a
  conversation, and routing one through would put guard passes and persona
  accounting on a yes/no — so they never reached `usage_events` and the port's
  cost was invisible everywhere in the product. An operator who switched five
  sites on had no way to find out what that cost. The provider reports billable
  tokens on the reply (`usage.input_tokens`, and `prompt_tokens` on the chat
  wire); `decide_shadow` now carries them per judgment and each site shows its
  own total. **NULL means not reported, never zero** — a classifier sidecar
  reports no usage at all, and a zero there would make a site look free. A
  fan-out's usage rides every answer it produced, because the request is what
  was billed and splitting it per question would invent a number, so the row
  carries `fanned` as the divisor instead.

  **2 · WE RECORDED THE MODEL WE ASKED FOR, NOT THE ONE THAT ANSWERED.** These
  differ routinely and silently: `jev-latest` is an alias, and the service's own
  response documents its `model` field as "may differ from the alias supplied
  in the request". A ledger that records the alias says `jev-latest` for ever,
  so an agreement rate spanning a rollover from one version to the next
  averages two different models into one number — and deciding whether to trust
  a model is the only thing that ledger is for. Every judgment now carries the
  model the reply named, the report tallies them per site, and **the panel warns
  when more than one answered**, because that rate is then about neither of
  them.

  **3 · A RATE WITH NO CASES BEHIND IT.** The report said "agreed on 94% of
  300"; nothing answered "which 300, and what were the other 6%" without a psql
  prompt. So the question that actually decides whether a site is safe to
  switch over — *was this disagreement the model being right, or wrong?* — had
  no surface at all. `POST {action: "rows", site}` returns the rows,
  **disagreements first**, and the panel renders them per site. Readable
  whatever the config says: an operator who just switched the provider off
  after a bad week is exactly the person who needs to read why, and gating that
  on `configured` would hide the evidence behind the switch the evidence is
  about. The subject is still a **reference, never the text** — a ticket
  message is somebody's words and this ledger is the wrong place to accumulate
  them.

  **4 · THE RERANK SITE RECORDED NOTHING**, so its census row said "no
  comparisons recorded yet" for ever and read as a bug rather than as a choice.
  It now records one comparison per search, asking the same question the
  brief's ordering does: did the reorder change what comes **first**? Vector
  order is the baseline, because that is what search returns with reranking off
  and it is also the fallback.

  **5 · THE GUARD SITE GENUINELY HAS NOTHING TO COMPARE**, and that is not an
  oversight to fix: a guard finding *is* the output, and the regex rules
  structurally cannot answer the semantic question, so there is no second
  answer. A site with nothing to compare does not belong in a comparison
  ledger. `SiteDef.measured_at` lets it say where its numbers are —
  `guard_findings`, grouped by check type on Guardrails — and the panel renders
  that instead of an empty state.

  **6 · THE PER-ROOM GATE WAS INVISIBLE FROM THE PANEL.** Unprompted agent
  replies need two switches open, and the second is per-channel by necessity —
  a room where that is wrong has to opt out without the capability coming off
  everywhere. But "which rooms would start talking if I flip this" is the
  question somebody asks immediately before flipping it, and it had no answer
  here. The panel now lists them, read-only, with how many agents each has,
  since a room with none cannot speak however its switch reads. The switch
  itself stays in the room's own settings: two spellings of one switch is how
  they come to disagree.

  **7 · THE CAPS ARE STILL CONSTANTS** — `MAX_JUDGED` (40 brief rows),
  `MAX_CANDIDATES` (8 agents), `MAX_TOOLS` (120), the per-wire text budgets.
  Left deliberately: each one bounds a request against a context window rather
  than expressing a preference, and an operator who raises one past what their
  provider accepts gets a refusal rather than a trade-off. Said here so it is a
  decision rather than an omission.

  **One bug the work itself caught.** `sites_public` flattened `measured_at` to
  a constant `None` on the way out instead of reading it from the def — so the
  panel would have rendered, the sentence would simply never have been there,
  and the one site that needed it would have shown an empty ledger for ever.
  That class of mistake is invisible from the UI, so it is now pinned by a test
  that projects a def through the public shape and compares the fields.

  **And one the work itself got wrong, which is worth recording because the
  class recurs.** Adding three fields to the public `Judgment` meant finding
  every literal that constructs it. The search covered `api/crates` and missed
  `api/tests/it/decide_shadow_live.rs` — the root package's integration tests,
  which reach into the crates directly — so CI failed on a sixth literal after
  a green local gate. The gate is not at fault: it compiles only the packages
  the diff touches, and `talaria-api --all-targets` pulls in the whole graph,
  which is the expensive thing it exists to avoid. **When a crate's public type
  changes, the search is `api/`, not `api/crates`.** The repair also earned its
  keep: those live tests now assert the new columns against a real Postgres —
  `judgments` 8, `metered` 8, `tokens_in` 960, `acted` 0, and one model in the
  tally — which is where an `integer` column and a `u32` field get to disagree
  and the unit tests' in-memory shapes cannot tell.

  Verified: 93 tests in `talaria-decide` (1 new, plus the report-shape test
  extended to cover cost, the metered denominator and a two-model site — the
  fixture is deliberately a site where two models answered, because that is the
  confound the tally exists to make visible). `Judgment`'s three new fields
  were added to the five existing literals by locating each `Judgment {` block
  rather than sweeping on `latency_ms:`, which appears in eight unrelated
  structs. clippy clean across every touched crate; `bun run typecheck` 0
  errors; `bun run gate` green. The migration is three `add column if not
  exists` and the snapshot was regenerated against a scratch
  `postgres:16-alpine` — a three-line diff, replay confirmed idempotent
  (`applied: 0, schema matches snapshot`).

  **Not exercised live.** No real reply has had its usage read, and no real
  rollover has been observed splitting a site's rate in two — the shapes are
  pinned against TypeSafe's published `SystemOneResponse`/`Usage` schemas and
  the OpenAI convention the chat wire already meets.
