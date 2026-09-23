- **The talaria-events plugin loads — its hooks never fired before.** Two
  loader-contract misses kept the plugin silent through its whole first
  deploy: Hermes refuses a plugin with no `register()` entry point (logged
  once at boot, easy to miss), and the hook names it registers must be the
  loader's own spelling — `pre_tool_call` / `post_tool_call`, not the
  `on_pre_tool_call` / `on_post_tool_call` the functions happen to be named
  (the loader warns "registered unknown hook" and drops them). With both
  fixed, the run-detail Live pane finally shows tool events with their
  arguments and results (`toolfull` frames on the run's watch stream), the
  per-turn transcripts carry them, and the thrash brake's failure signal
  exists on a real fleet.
  Verified: on the dogfood instance, a live work session's tail carried 31
  `toolfull` frames within two minutes of the fix (previously zero ever),
  including full terminal command previews; all four agent containers load
  the plugin with no loader warnings; unit test pins the register()
  contract.
