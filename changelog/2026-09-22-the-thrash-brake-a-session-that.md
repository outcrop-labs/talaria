- **The thrash brake: a session that fails every tool call now parks itself
  instead of streaming forever.** A work-session turn has no total timeout by
  design — frames arriving means the turn runs — but on 2026-09-22 that law
  met an agent whose every tool call errored for half an hour: the run read
  `running`, the lease kept renewing, and the ticket showed activity that did
  not exist. The talaria-events plugin now reports each tool call's verdict
  (`status: "error"` for the persona's own `{"success": false}` / `{"error":…}`
  convention), and ten consecutive failures abort the turn mid-stream — the
  same interrupt a stop produces — with a sentence on the ticket saying the
  agent's tool plane is failing and its container likely needs a roll. The
  transcript of everything the turn did say is captured before the abort, so
  the record has no hole. Known blind spot, on the record: an agent whose
  plugin reports nothing disables the brake silently.
  Verified: the streak law pinned by unit test (`talaria-runs-work-session`,
  25 passed — breach at ten, reset on success, stream/running/junk frames
  inert); the plugin classifier exercised against the incident's own result
  shapes (8/8); cargo test green across every touched crate.
