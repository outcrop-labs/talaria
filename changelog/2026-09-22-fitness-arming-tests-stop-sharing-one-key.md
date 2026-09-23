- **The fitness-arming live tests stopped sharing one capability key.** The
  binary's two tests both used `pl-main:qwen3-14b` and each began by
  forgetting that key's facts — and libtest runs a binary's tests
  concurrently, so one test's reset could delete the other's probe facts
  mid-flight. That is a timing lottery, and CI's slower runners lost it every
  run (the api-integration gate failed on `a_reported_tool_call_becomes_a_
  widened_inbox_surface` in three consecutive runs) while a fast dev box kept
  winning it. Each test now fabricates its own model id
  (`qwen3-14b-reported` / `qwen3-14b-declared`), so no reset can touch a
  sibling's rows and the isolation no longer depends on scheduling.
  Verified: the binary green five consecutive runs against scratch
  postgres+redis (the race window is gone by construction — separate keys
  cannot collide), `cargo fmt --check` clean.
