- **Drafting tickets from a plan starts again.** Every plan-draft POST answered
  500 "could not start the plan draft — see server logs" because
  `BUILD_DISPATCH` — the run assembly plan-draft, research, and guided reindex
  enqueue through — was never `.set()`. The seam and its constructor
  (`work_dispatch::dispatch_deps`, the same assembly agent-hire calls directly)
  both existed; the boot wiring did not, so the failure was every install, not
  one host. `register_all` now sets it, and the boot test that pins the job
  table pins the seam the same way: with the set removed it fails on
  `BUILD_DISPATCH fell out of the boot wiring`, and it passes with the set.
  Verified: `cargo test -p talaria-jobs --lib tests::register_all_declares_the_whole_ported_table`
  — red without the wire, green with it.
