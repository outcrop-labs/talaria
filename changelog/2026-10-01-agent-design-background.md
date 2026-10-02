- **TALA-11 (part 2): the agent design runs in the background.** Closing the
  New Agent modal no longer kills the generation. The whole draftAgent()
  orchestration — fields, chat window, elapsed tick, refine receipt — moved
  into a module-level reactive store (`lib/agent-design.svelte`), because the
  modal remounts per open and a run that survives its window cannot live in
  component state. Cancel during a design is an exit, not a cancel; reopening
  re-enters the run (generating: the splash plus the purpose text; ready: the
  review step with fields applied; error: describe with the purpose and the
  server's sentence). The roster carries a slim status row above the hires —
  "Designing <first words of the purpose>…", click to reopen — and a success
  toast (linking to /agents) fires when the design lands while the modal is
  closed. A second describe while one runs re-enters the same run rather than
  silently replacing it; an idle finished design is replaced.

  Verified: new store unit tests (`agent-design.test.ts`, 11 cases across
  single-flight, refine shape, toast gating, claim) pass; full `bun run test`
  85 files / 1360 tests green; svelte-check 5424 files, 0 errors, 0 warnings.
