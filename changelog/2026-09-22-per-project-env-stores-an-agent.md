- **Per-project env stores: an agent's build gets its `.env`, sealed at rest
  and scoped by the repo grant.** Each granted repository can carry a set of
  environment variables in the workbench admin (beside the branch rules):
  KEY chips with write-only values — a value never crosses the API in either
  direction after it is saved; it is sealed with the same envelope the
  workspace secret vault uses and leaves the database exactly once, when the
  fleet render materializes `/opt/workbench-env/owner/repo.env` into the
  containers of the agents granted that repo (read-only mount, shell-safe
  single-quoting, absent entirely when the store is empty). The work-session
  brief names the file's path when one exists, so the agent sources it the
  way a developer would. The visibility class is the inverse of the vault by
  design and the panel says so: these values ARE readable by the agent's
  build — exactly what a project `.env` is for; org credentials stay in
  workspace secrets, which are never revealed to anything. Access control
  needs no new grants: the existing repo grant IS the env grant. Verified
  live: a variable set through the API arrives verbatim in the granted
  agent's container file and in no other agent's, and the stored row is
  ciphertext.
