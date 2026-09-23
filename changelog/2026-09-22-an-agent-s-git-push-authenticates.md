- **An agent's `git push` authenticates again — the credential helper read a
  key the api never accepted and a URL nothing ever set.** The fleet-rendered
  git credential helper (`git-credential-talaria`) sent the chassis's own
  server key where `/api/secrets/git-credential` authenticates the agent's
  `tak_` credential — every helper call 401'd — and built its URL from
  `TALARIA_API_URL`, a variable no renderer ever wrote, so even a valid key
  would have posted to a relative address. The helper now sends
  `TALARIA_AGENT_KEY`, and the render bakes `TALARIA_API_URL` into every
  agent service env, derived from the app's own `TALARIA_GATEWAY_SELF_URL`
  (dev-defaulted to the docker host-gateway) — one env, so the agent's shell
  and the workbench harnesses running in the same container both get it. A
  failed fetch now says so on stderr instead of exiting silently: a
  credential that does not exist and a helper that cannot ask had the same
  silence. Verified: the helper's contract is pinned by render tests (the
  agent key, never the chassis key; the env-derived URL), and live on outcrop
  after the roll + a fleet reconcile the helper answered a real GitHub
  installation token for a granted repo from inside Doug's container.
