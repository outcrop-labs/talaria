- **An unbounded `apt-get` could eat a whole CI job, and it did — twice in one
  evening, on two different pull requests.** `api/.cargo/config.toml` passes
  `-C link-arg=-fuse-ld=mold` for `x86_64-unknown-linux-gnu` unconditionally,
  so a missing `ld.mold` fails the link outright with collect2's "cannot find
  'ld'". That made a single `sudo apt-get update && apt-get install mold` — no
  timeout of its own, duplicated verbatim in two workflows — **a hard
  dependency of every api build in CI**, and apt is the least reliable thing in
  the job.

  What it cost: the promotion pull request's `live-tests` ran **45m17s** and the
  api job on the next one ran **30m19s**, both killed by their job timeout with
  `Format`, `Clippy`, `Test` and every later step **skipped**. Neither failure
  named apt. A job killed by its own `timeout-minutes` renders in
  `gh pr checks` as `fail`, so both read as a test failure on a tree that was
  green — and the natural response, re-running, just rolls the dice again. Over
  the preceding 30 `api-integration` runs, **8 concluded `cancelled`**.

  **`.github/actions/mold` is now the one place that knows how to get it**,
  used by both workflows, doing three things in order of how much they are
  worth:

  1. **Don't call apt when the binary is already there.** Free, and some runner
     images ship it.
  2. **BOUND EACH CALL.** This is the change that matters: it turns a
     45-minute silent death into a named one with a ceiling of ten. A composite
     action's steps **cannot** carry `timeout-minutes` — that key is only valid
     on a workflow job's own steps — so the bound is `timeout(1)` around each
     apt invocation, which is better anyway: it bounds the two calls
     separately rather than the step as a whole.
  3. **FALL BACK RATHER THAN DIE.** `CARGO_ENCODED_RUSTFLAGS` set to the empty
     list overrides the config file's target `rustflags` wholesale, so cargo
     links with the default linker. A slower link is the right failure, and it
     is the argument the config file already makes about its own job cap:
     "slow, not a pinned laptop. That is the right failure." A green build four
     minutes slower beats a red one that took forty-five and tested nothing.
     The fallback warns loudly, because a permanently slower CI is something to
     be told about once rather than discover from a graph.

  Verified, and the first attempt at verifying it was wrong in a way worth
  recording. `CARGO_ENCODED_RUSTFLAGS=""` was **checked, not assumed**:
  `cargo check -v -p talaria-params --target x86_64-unknown-linux-gnu` passes
  `fuse-ld=mold` normally and passes it nowhere with the variable set (plain
  `RUSTFLAGS=""` also works; the encoded spelling is the documented
  unambiguous one for "an empty list" rather than "unset"). The action's script
  was then driven through all three paths against fake `sudo` binaries — mold
  already present exits clean and exports nothing; apt failing fast warns and
  exports the fallback; apt **hanging** is bounded. That last case first
  reported success in 0.0s, which was a lie: `sleep` was missing from the
  minimal PATH the test used, so the fake `sudo` died instantly and the bound
  was never exercised at all. With `sleep` available it takes 8.0s for four 2s
  windows — so the bound genuinely fires, and at the shipped windows
  (120s + 180s, twice) the ceiling is ten minutes rather than forty-five.
