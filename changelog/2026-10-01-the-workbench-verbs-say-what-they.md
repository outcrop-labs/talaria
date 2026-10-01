- **The workbench verbs say what they did.** `start_job`, `doctor`,
  `prepare_env`, `finish_job` and the rest wrote **nothing** to the server
  log for their whole existence — the only lines matching "workbench" in a
  day of the live instance's output were the scheduler's own
  `workbench-queue-sweep ok in 3ms — nothing to do` heartbeat. The one
  logging call the dispatcher had, `log_wtool_line`, publishes to the
  *agent's watch stream* and returns silently when there is no Redis, no
  model, or no live work session, so a `doctor` that failed outside a run was
  recorded in no place at all. That is why finding out why the harness could
  not start took ssh into a container and probes run by hand rather than
  `docker logs`. Every verb now logs once at the dispatcher, which is the one
  place they all pass through: `info` on success with the elapsed time,
  `warn` when the agent is refused (no grant, cap reached, plan missing —
  expected, but the reason is worth having), and `error` when Talaria itself
  throws, because that is a bug rather than a policy. The job id, repo and
  task id ride along when the call carries them, since "start_job failed"
  with no subject is a line nobody can act on; the arguments do not, because
  a plan is long and a clone URL is a credential. Verified: `bun run check`
  and `cargo fmt --all --check`, and the absence it fixes was confirmed on
  the live instance by counting every workbench line in 24 hours of output.
  The Rust was not compiled locally — CI's `fmt + clippy + test (api)` is the
  proof line.
