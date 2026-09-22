- **New-agent onboarding got the same treatment: the Muse refine of an
  agent design is no longer a silent field swap.** On the review step of
  the new-agent modal, a refine now shows an honest in-progress state
  ("Refining the design: identity, soul, and starter skills" with a pulse)
  for the seconds the whole-agent contract takes, and on landing it folds
  with a "Refine applied" receipt — which fields the design touched and
  the ±lines on the soul — that holds until the next refine. The describe
  step's first design keeps its generating block. Decisions live in
  `ui/src/lib/agent-onboard-refine` (unit-tested, 10 cases); verified in a
  real browser against an API stub: design in-progress state, draft lands
  in the review fields, refine in-flight state, receipt math on a trimmed
  soul, failed refine leaves the draft untouched, error line shows.
