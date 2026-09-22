- **A git credential the platform won't give fails fast instead of hanging the
  agent.** A push to a repo off the agent's grant list made the credential
  helper decline (correct), git fall back to its terminal prompt, and that
  prompt waits forever in a harness PTY — the agent reads as stuck, and the
  2026-09-14 incident was exactly this shape (the granted repo's pushes
  worked; the probe of an ungranted one hung). The render now sets
  `GIT_TERMINAL_PROMPT=0` in every agent service env, so every such case is
  git's immediate "could not read Username" — which the toolkit skill already
  tells the agent to `report_problem`. The helper's decline and the operator
  log both name the repo now (`no credential for github.com/owner/repo`), so
  the next stuck agent is diagnosable from one log line. Verified: an
  ungranted path through `git credential fill` in Doug's container fails
  immediately post-roll with the repo named in the stderr.
