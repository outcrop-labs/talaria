- **An agent's code reaches main only through a human's merge.** The work
  session's dispatch brief now carries the repo-hygiene law as a numbered
  step — push to a branch named for the ticket, open a PR, put the link in
  the outcome; the toolkit skill states the same rule for every other
  surface — and it is enforced where behavior cannot be: `main` on the
  granted repos now requires a pull request (branch protection, zero
  approvals needed, administrators exempt), so an installation-token push to
  main is declined by GitHub while human direct pushes are unchanged.
  Verified live from inside an agent container with the injected credential:
  a branch push succeeds (`hygiene-probe` created and cleaned up), a push to
  main is refused (`protected branch hook declined`). Two private repos
  (talaria-website, skal-website) cannot carry protection on the org's free
  plan — the prompt and skill rules stand there, and upgrading the plan or a
  push-webhook revert are the options if they need the hard wall too.
