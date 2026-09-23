- **Workbench job branches now obey the repo's own branch rules.** `start_job`
  minted every job branch as `talaria/<ticket-ref>-<slug>`, but the platform's
  per-grant branch law (`workbench_repos.branch_prefix`, enforced on every
  sandbox push) can require a prefix — on outcrop-labs/talaria it is `agent/`,
  so every job branch on our own repo was unpushable from birth and
  `finish_job` could never see commits; PRs shipped only through the manual
  push-an-agent-branch-and-call-REST workaround (TALA-16 / PR #402), and job
  completion bookkeeping never fired. `start_job` now mints the job branch
  under the grant's configured prefix when one exists (the historical
  `talaria/…` shape otherwise), the job's rules text says the branch satisfies
  the repo's rules, and `finish_job` pre-checks the job's branch against the
  same law — a legacy job with an unpushable name gets an honest refusal
  pointing at abandon-and-restart instead of a misleading "no commits yet"
  after GitHub 404s the compare. Tools and docs updated to match. Verified:
  `cargo fmt`, clippy `-D warnings`, and the api test suite green, including
  the new self-referential test (a minted branch passes its own repo's
  `push_allowed`; the old `talaria/*` shape fails it).
