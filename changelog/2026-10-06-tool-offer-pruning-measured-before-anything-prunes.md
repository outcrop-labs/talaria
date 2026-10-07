- **Tool-offer pruning, measured before anything prunes.** The agent toolkit is
  81 tools, and every agent turn carries the definitions it was offered — input
  tokens on every turn of every session, whether or not that turn had any use
  for `cancel_google_event`. A decision model could pick the plausibly-useful
  subset in one round trip and the turn would carry a fraction of the
  descriptions.

  **This prunes nothing.** The failure mode is specific and bad: drop a tool the
  agent then needed and the turn breaks in a way that reads as the model being
  stupid rather than as the gateway having hidden its hands. Nobody should
  accept that on the strength of an argument, so it measures instead.

  **The measurement is not "did the model agree"** — there is no baseline
  opinion to agree with, because production offers everything. The question that
  matters is *would pruning have broken this turn*: judge the offered tools,
  compute the subset that would have survived, then check it against the tools
  the reply **actually called**. A keep-set containing every called tool is a
  safe prune; one missing even a single called tool is the failure, recorded as
  a disagreement. Grouped per **caller**, because a Workbench session and a
  personal assistant do not have the same tool spread and should not be judged
  by one number.

  The keep floor is deliberately **permissive** (0.15) — the sharpest asymmetry
  on the port. Keeping a tool nobody needed costs a few hundred input tokens;
  dropping one that was needed breaks the turn. A measurement at a generous
  floor answers whether even the generous version is safe; a strict one would
  only prove that strictness is unsafe.

  **Its own switch, off by default** — configuring a decision model is not
  consent to put every agent turn through a question per offered tool. It is a
  checkbox on the decision-model panel, with the cost stated next to it. A turn
  offering more tools than the cap is **skipped rather than truncated**, because
  judging a subset and reporting "the prune would have been safe" would be a lie
  about which tools were weighed.

  Also adds `shadow::Derived` beside `Compare`, because this site's answer is a
  *set* rather than a single judgment: `Compare` decides agreement with a
  comparator over one `Judgment`, and there is no single probability behind a
  keep-set — so `probability`, `certainty` and `calibrated` stay null rather
  than carrying a summary statistic nobody could interpret. (My first draft
  wrote a `Compare` row and then `UPDATE`d it by `order by created_at desc limit
  1`, which is a race and a lie about the shape; the primitive is the honest
  fix.)

  Verified: 64 tests in `talaria-decide` (6 new) — tools read from both the
  wrapped and bare wire shapes, calls read from both `tool_calls` and the
  retired `function_call`, an unreadable body yields nothing so the pass can
  never be why a completion misbehaves, the subject is the last user turn rather
  than the whole transcript (a session that once used Drive should not keep
  every Drive tool alive forever), the keep floor pinned by behaviour, and the
  safety verdict is exactly "every called tool survived" — including that a turn
  which called nothing cannot be broken. clippy clean; `bun run typecheck`
  (5472 files) clean; `bun run gate` green with the regenerated API reference.
