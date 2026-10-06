- **The first guardrail that is judged rather than matched.** The five existing
  rules are regexes standing in for meaning, and `zero_tool_claim` shows the
  limit: it fires on a *claim phrasing*, so a reply that says "staging is on the
  new key; deploys are green" — no verb of doing, no first person, just a new
  world asserted as fact — walks straight past it while claiming a completed
  action as surely as "I rotated it" would. `implied_completion` asks the
  question instead: does this reply leave a reader believing something happened
  that nothing in the turn's tool record performed?

  **Default off, medium, and observe-only by construction.** Off because
  sending completion text to a decision model is an operator's choice to make
  knowingly. *Medium* rather than high so it stays out of `HIGH_SEVERITY_RULES`,
  which the fitness band rule reads to **fail** models — a rule whose
  false-positive rate nobody has measured must not start condemning them. And
  the evaluator runs **detached, after the reply has already gone back to the
  caller**, so it can record a finding but can never annotate or redact: the
  measurement cannot change the answer it is measuring. The guard's existing
  mode ladder does the rest — `Observe` discloses nothing, so an operator can
  run this for weeks, read `guard_findings` grouped by `check_type`, and decide
  from their own traffic whether it earns `Annotate`.

  An uncalibrated answer **never files**. A provider that answered without a
  real distribution behind it would put a fabricated number in
  `Finding.confidence`, which `min_confidence` compares against and the fitness
  page reads as a per-model confabulation rate. A guard finding is a fact about
  a model; an invented probability is not one.

  **The rule is declared in the guard and answered in the port**, because
  `talaria-decide` depends on `talaria-gateway` and the reverse would be a
  cycle. So `rule_ids()`, `rule_severities()`, the admin per-rule toggle and the
  harness `GuardDecl` validation all see it like any other rule, while the call
  is made by the route that already runs the structural pass. `RuleDef` gained a
  `semantic` flag and `run_guardrails` skips those **by declaration** rather
  than by falling through `run_rule`'s catch-all — so a *structural* rule that
  forgets its evaluator still fails loudly, which is the shape of bug that file
  is scar tissue from.

  **What the codebase's own tests taught, and the correction they forced.** The
  first attempt put `implied_completion` in the fitness adversarial corpus,
  because that tier asserts every rule id is attacked by a seed. Its tests
  refused, and they were right: tier 3 scores a model by running the real rules
  over a *recorded* reply with no model in the loop, so `run_guardrails` skips a
  judged rule, `rule_fired` is always false, and a seed targeting one would
  report **every model as perfectly resistant** — the precise false-clean that
  tier's own header warns against. The escape hatch, a seed's own `fell`
  predicate, is worse: writing one means writing the matcher whose
  non-existence is the rule's entire reason for being. So the coverage
  requirement now reads `structural_rule_ids()`, and semantic rules are
  measured where a model exists.

  Verified: 58 tests in `talaria-decide` (4 new — the rule ids and severities
  are held against the guard's own registry, so a rule declared in one place and
  not the other fails rather than silently never firing; and the state names the
  tool record even when it is empty, because "no tool ran" is the fact the
  judgment turns on and an absent key reads as "unknown"). `talaria-fitness` 416
  passed, `talaria-gateway` 66, `talaria-harness-defs` 435. clippy clean across
  the four touched crates; `bun run gate` green.
