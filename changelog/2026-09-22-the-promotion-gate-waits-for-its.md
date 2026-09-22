- **The promotion gate waits for its evidence instead of failing ahead of it.**
  The gate ran the moment `rc` was pushed — the same push that STARTS the CI
  and rc-deploy runs it reads — so on every synchronize it found no green run
  for the new tip and failed within seconds, and a failed required check is
  never re-evaluated: the promotion pull request sat red through two green
  deploys, its only automatic recovery a body edit that requires the
  `PROMOTION_TOKEN` this repository has not set. The gate now folds each
  workflow's runs for the head commit into a state — green (any successful
  run; a re-run after a flake still counts), waiting (a run still in flight),
  absent (none registered; five minutes of that is a trigger problem, not
  evidence arriving), red (all finished, none green) — and polls every 30s
  until both halves are green or one is red. Its timeout rose from 5 to 95
  minutes to cover rc-deploy's own worst case (the called api-package job's
  60 plus the deploy job's 30).
  Verified: the state folding run locally against the live Actions API for
  both tips this wedge produced — 66241c38 reads `CI=green · rc-deploy=red`
  (its deploy was cancelled by the next push: correctly red, the documented
  recovery being a re-run) and 2f075152 read `CI=waiting · rc-deploy=green`
  mid-flight, the loop's reason to keep polling rather than decide;
  `bash -n` on the step's script; `bun run check` green with the branch-flow
  anchors still reading flow.yml.
