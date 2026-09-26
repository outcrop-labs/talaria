- **A workbench start on a full host queues the job instead of failing it.**
  `start_job` ran the admission door and turned a refusal into an error, so a
  busy box meant the agent retried blindly or gave up and the ticket sat. The
  refusal now parks the job: the row lands with `status='queued'` (`queued_at`
  stamps the FIFO place and survives, `queued_reason` carries the refusal and
  clears on promotion), and the agent's answer carries `queuePosition`,
  `waitReason`, and a note to poll `job_status` rather than retry — nothing
  exists to run yet, so no clone URL, workdir, or harness lines. A one-box FIFO
  sweep (leased, not per-instance, head-of-line with no skip-ahead: a blocked
  heavy head stops the pass, a light job does not leapfrog) promotes queued
  jobs whenever `admit_work` says the host has room again, clearing the
  ticket-level `work_wait` the refusal left on the board; `job_status` answers
  a queued job with its place and the same phase line the board spells.
  Admission got real numbers to judge with: `talaria-host-metrics` now exposes
  the host's core count, load-per-core, and free bytes on the data root
  (`f_bavail`, resolved through the host root so a containerized api still
  reads the host's disk), and `talaria-fleet-budget` reads them (env-tunable,
  every refusal names each failing dimension with its numbers). Verified:
  `bun run check` and `bun run gate` green (fmt, clippy `-D warnings`, unit
  suites for talaria-api, talaria-fleet-budget, talaria-host-metrics,
  talaria-jobs, talaria-scheduler, talaria-workbench-mcp,
  talaria-workbench-queue; svelte-check 0 errors; 1277 ui tests), and the new
  live suite `api/tests/it/workbench_queue.rs` (`cargo test --test it --
  --ignored workbench_queue::` against a scratch Postgres — the shape CI runs,
  both tests in ONE process): the rank window, the head-of-line flip that
  clears the reason and unblocks the follower, and the refusal that parks the
  whole line — 2 passed, 0 failed. That suite runs one test at a time behind
  a module lock, because the queue it measures is box-wide by design: run
  concurrently, each test answered the other's rows (position 3 where it
  seeded 2; the sweep promoting the sibling's job), which is how they first
  went red in CI. The two migration statements live at the END of MIGRATIONS,
  not beside the table they alter — the ledger is keyed by index, so a
  mid-array insert renumbers everything after it and a deployed instance
  refuses to boot; the upgrade replay (main's array, then this one on top of
  the same database) is what catches that, and it is green: 386 baseline
  statements, then exactly 2 applied, snapshot matching on both the fresh and
  the upgraded database, and `applied: 0` on a second pass.
  The pre-existing `live_proc_reads_this_machine` test fails on this devbox
  because `/` here is an overlay (filtered as a pseudo fs); it is untouched by
  this change and passes on a host with a real root filesystem.
