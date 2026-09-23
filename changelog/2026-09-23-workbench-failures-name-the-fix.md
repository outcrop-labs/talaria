- **Workbench failures name the fix instead of inviting a retry.** A live-job
  cap refusal lists each job's id, repo, branch, and workdir as JSON the agent
  can pass to `finish_job`. A finish on a branch with no commits names the
  workdir, branch, and `refs/heads/…` it checked. `job_status`, `finish_job`,
  and `merge_to_testing` refuse a non-uuid job id (`omp-tala35`, `x`) instead
  of handing it to Postgres (`invalid input syntax for type uuid`). `start_job`'s
  schema states that omitted effort is standard and that standard/heavy require
  `plan`. A stored branch prefix of `agent/` cannot mint `agent//…` for a
  ticket slug or a ticketless `job-<repo>-<suffix>` — both go through one join
  that collapses empty components. Outside a job workdir,
  `git-credential-talaria` reads the checkout's origin when git omits the path,
  and a pathless decline says so instead of bare `github.com`. Do not `git
  credential fill` — that prints the token. Verified: `bun run verify` (check,
  svelte-check, 1267 ui tests); the api gate from `api/` on rustc 1.97.1
  (`cargo fmt --check`, `clippy -D warnings`, `cargo test` — 89 suites). A
  pathless `git credential get` against a checkout whose origin is
  `https://github.com/outcrop-labs/talaria.git` returned that repo's path.
