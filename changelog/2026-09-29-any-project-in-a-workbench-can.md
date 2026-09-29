- **Any project in a workbench can declare what its jobs may give back.**
  Job teardown asked "is this job over?" and never "how big is this?", so a
  job that is perfectly alive — ticket open, agent working — held a checkout
  plus whatever its toolchain wrote beside it, with nothing measuring and
  nothing capping it. `teardown.rs` puts a cold Rust `target/` at 4–20 GiB on
  its own, which makes ten live jobs in a department 200 GiB of entirely
  legitimate work whose first symptom is a build failing on ENOSPC. Teardown
  gains a third rung between Stop and Remove: **reclaim** drops only what the
  project says it can rebuild, and leaves the checkout, the branch, `.git`
  and every uncommitted edit alone, so a reclaimed job that resumes pays a
  rebuild and nothing else. A project declares its own rules in the
  `.talaria/workbench.toml` it already uses for `apt` and `[tools]` — a
  `[cleanup]` table with `artifacts` (merged over detection; `[]` opts out),
  `keep` (never dropped, covering its subtree, winning over everything), and
  `maxWorkdirGib`. A project that declares nothing still works on day one,
  because the same marker scan `prepare_env` runs for toolchains now also
  detects build output: Cargo, npm, Python, Maven, Gradle, .NET and Mix.
  What is absent from that table is deliberate — `node_modules` itself is
  rebuildable only over the network so only its cache goes, `vendor/` is
  often committed and load-bearing offline, and Go's build cache lives
  outside the repo. Cheap and local is reclaimable; slow or remote is not.
  Reclaim refuses outright while any process has a cwd inside the workdir —
  Stop's own predicate, inverted — because a build whose `target/`
  disappears mid-link fails in a way nobody can read. Paths are validated in
  Rust and again in the script: absolute paths, `..` at any position, empty
  segments and `.git` at any depth or casing never reach the `rm -rf`, which
  is running inside somebody else's container. Verified: `bun run check` and
  seven new pure unit tests in `talaria-workbench-mcp` covering detection per
  ecosystem, declared-merges-over-detected, the empty-list opt-out, `keep`
  beating a detected default and covering its subtree, rejected declarations
  being reported rather than silently dropped, and the path guard. The Rust
  was not compiled locally — the box had three worktrees cold-building — so
  CI's `fmt + clippy + test (api)` is the proof line.
