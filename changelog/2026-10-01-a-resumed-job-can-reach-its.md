- **A resumed job can reach its harness again, and the watch panes stop
  inventing turns.** `start_job` answered with the harness invocation lines;
  `job_status` answered with a workdir, a clone URL and no way to run
  anything. A job is started once and resumed on every later turn, so from
  the second turn onward an agent held a guide saying "First turn: jsonRun"
  and no definition of `jsonRun` anywhere it could still reach — the only
  copy was in a response from a session that had ended. On the 2026-09-30
  dogfood run that is exactly what happened: the agent called `doctor` and
  `job_status`, never `start_job`, ran
  `find /opt/data/workbench/harness -name '*oh-my-pi*'`, read the oh-my-pi
  skill hunting for a command, found none, and built the ticket itself with
  cargo. There was no agent-to-harness transcript to show because the harness
  was never invoked. Both answers now come from one `harness_answer`, so a
  started job carries `harnesses[0].jsonRun` / `continueJsonRun`, `sessionDir`
  and the models on every poll, and the tool's own description says so.
  Two rendering faults rode on top. `isHarnessFrame` matched the string
  `oh-my-pi` anywhere in any tool frame, so reading the skill document,
  `read_file` on its markdown, and the `find` that went looking for the
  binary all opened "turns" — every turn in that run was a false positive.
  Detection is now restricted to tools that can actually run something
  (`terminal`, `process_manage`, `execute_code`) and to text carrying
  `--session-dir`, which every invoke template has and no mention of the
  harness does. And the splitter only understood a foreground command, while
  a backgrounded harness is started as a managed process and then POLLED
  through a `session_id` the polls never name — so the harness's whole side
  of the conversation arrived as ordinary agent tool calls and landed in the
  agent pane as raw JSON. The session handle is now tracked and its polls
  fold into the turn they belong to. Tool detail in the agent pane is clipped
  to 240 characters with the full text still in the transcript, because a
  managed-process poll answers with up to 2 KiB of JSON and a session driving
  a build answers with hundreds of them. Measured on that run's real 1897
  frames: 21 fabricated turns became 0 (correct — the harness never ran), and
  the agent pane went from 216,636 characters to 106,878. Verified:
  `bun run check`, `cargo fmt --all --check`, the full ui suite (83 files,
  1348 tests), `svelte-check` (5419 files, 0 errors), and
  `work-watch.test.ts` rebuilt from the frame shapes that run actually
  emitted — the previous version invented a foreground `terminal` call
  carrying the whole harness command, a shape production never produced once,
  which is how a JSON wall and a Turns tab of pure false positives both
  passed a green suite. Writing those real shapes down immediately caught a
  further bug: a backgrounded launch wraps the command in the process tool's
  JSON, so the steer was being read as the wrapper's own `command` key.
