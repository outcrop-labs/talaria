- **`execute_code` works again on fleet agents: rendered configs now carry
  `approvals: { unattended_mode: approve, cron_mode: approve }`.** Talaria
  drives every agent through the Hermes api — an "unattended platform" in
  Hermes' approval model — and cron jobs run the same way, so the fail-closed
  defaults denied `execute_code` outright (the live error: "This session runs
  on an unattended platform (api_server) with no user present to approve it")
  and stalled every dangerous-command prompt until timeout; agents limped
  back to terminal one-shots. The container is the sandbox and tirith is the
  guard, so both modes flip to approve, preserving any other approvals keys
  an agent def carries. Verified: a unit test pins the override (authored
  keys preserved, both modes flipped), and a live render in an isolated
  worktree emits the approvals block into the agent's config.yaml.
