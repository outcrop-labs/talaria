- **CPU samples read the field docker actually emits.** The resource
  sampler's `{{json .}}` parser read `CPUPercentage` — the documented Go
  template name — while the CLI's JSON spells it `CPUPerc`, so every CPU
  sample since the feature landed read 0.00 (25,329 rows on the dogfood
  instance, all zero, while mem and pids were correct). Both spellings are
  now accepted, pinned by the exact JSON shape from the prod daemon.
  Verified: unit test with the daemon's literal output. Not yet observed
  live: the fix needs the next app deploy (the running instance still
  samples zero until then) — cpu_percent should read non-zero for busy
  agents within a minute of the deploy.
