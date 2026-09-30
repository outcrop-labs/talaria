- **The job sweep measures, as well as asking whether a job is over.**
  Reclaim existed but nothing called it. The sweep was status-driven end to
  end — "is this job over?", act on the answer — so a job that is alive and
  legitimate held whatever its toolchain wrote, unmeasured, until the volume
  filled. It now runs two passes. The status pass is unchanged. The size pass
  measures every live job (`started`, `awaiting_approval`, `pr_open`), asks
  its repo what is rebuildable, and reclaims oldest-first, so the job most
  likely to be resumed keeps its cache longest. A job goes either because it
  is over its ceiling — its own `maxWorkdirGib`, else 25 GiB, because one job
  is not entitled to the whole disk — or because the filesystem holding the
  workbench volume passed 80%, which is the backstop for several
  legitimately active jobs that together fill it and none of which should be
  cancelled. The repo's policy is read at sweep time with the same marker
  scan `prepare_env` already runs, rather than stored on `workbench_jobs`: a
  column would have to be kept in step with a file the repo owns, and the
  repo is the authority, so ask the repo. There is no idle timer to tune and
  get wrong — the script reads `/proc` for a live process whose cwd is inside
  the workdir and answers BUSY, so a job compiling this minute is asked again
  next sweep instead of losing a `target/` mid-link. Every step is
  best-effort per job: an unreachable container, a workdir with no checkout,
  a repo with no policy, or a busy build is skipped, never retried in a tight
  loop, and never allowed to fail the sweep. Reclaims are logged with the
  paths dropped and the bytes recovered, and the sweep's summary line carries
  the total; putting them on the ticket's activity is documented as not yet
  wired rather than implied. Verified: `bun run check`, `cargo fmt --check`,
  and five new pure unit tests covering the per-job ceiling and the repo
  override, a tight volume reclaiming a job under its ceiling, an opted-out
  repo staying untouchable even then, which job statuses are asked at all,
  and a `df` that printed nothing not reading as 0% full. The Rust was not
  compiled locally — CI's `fmt + clippy + test (api)` is the proof line.
