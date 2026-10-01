- **A sandbox that was never rolled says so.** A render change reaches a
  container only when the agent is re-rendered and ROLLED, and nothing forced
  that or noticed it had not happened — so a fix could be correct, reviewed,
  tested and merged and still be absent from every running agent, with no
  symptom but the thing it was supposed to fix. That is not hypothetical:
  the cont-init hook that hands the harness directories to the runtime user
  has been in the render with a test, while the live instance ran containers
  whose `/etc/cont-init.d` held only Hermes's own three scripts, leaving
  `/opt/data/workbench/harness/pi` root-owned and the harness with nowhere to
  write. The render now leaves a shape token in the workbench config dir and
  `doctor` compares what the container carries against what the running
  Talaria expects, reporting `sandbox: STALE` and naming the fix when they
  differ — including when the container carries no token at all, which is
  what every agent built before this will report. The token lives in
  `talaria-workbench-harnesses` because the render writes it and the MCP
  reads it and neither crate depends on the other. It is bumped by hand when
  the sandbox shape changes, so a forgotten bump is the one way this can
  still be quiet; the harness probe and the state-dir writability check added
  alongside it are the backstop for that, since they measure the sandbox
  rather than describe it. Verified: `bun run check`,
  `cargo fmt --all --check`. The Rust was not compiled locally — CI's
  `fmt + clippy + test (api)` is the proof line.
