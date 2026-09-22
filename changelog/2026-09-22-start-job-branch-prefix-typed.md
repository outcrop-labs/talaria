- **start_job: a branch prefix typed "agent/" no longer mints `agent//…`.**
  The repo grant's branch prefix was composed as-typed, so a trailing slash
  produced branch names GitHub refuses with a 422 (`refs/heads/agent//tala-35-…`
  — the failure Doug reported on TALA-35) while passing the platform's own
  push rule, dead-ending the job at creation. The prefix is now trimmed of
  slashes at the mint (existing stored rows included) and at the config
  route, and the regression is pinned by the exact spelling from the incident.
  Verified: unit tests pin `agent/` and `/agent/` to the trimmed composition
  and the round trip through `push_allowed`; cargo test green.
