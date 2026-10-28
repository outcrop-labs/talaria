- **pr-watch takes the CI workflow's name from the PR's own repository, and the PR template asks for a `changelog/` file instead of a hand edit of `CHANGELOG.md`.** Before green, the watcher requires a finished run of the CI workflow for the PR's head. It used to read that workflow's name from the `ci.yml` in its working directory. So `--repo owner/name`, or a `--pr` URL from another repository, waited on a workflow that repository never runs, or silently guessed `CI` when the working directory had no `ci.yml`. The target repository is now `--repo`, or the repository the PR's URL names. The name comes from that repository's `ci.yml` at the PR's head, read from GitHub, which is the copy a `pull_request` run uses. `gh run list` and the failed-job logs now query that repository too. If GitHub cannot answer, this checkout's `ci.yml` is used only when one of its remotes is the target; otherwise the watcher assumes `CI`. Either fallback is stated in the pending reason, the proof line and `--json` (`ciWorkflow.source`, `ciWorkflow.note`). A failed read is not cached, so the next poll tries again. `.github/PULL_REQUEST_TEMPLATE.md` now asks for `changelog/YYYY-MM-DD-<slug>.md` and says `CHANGELOG.md` is written only by the release roll. The `## CHANGELOG entry` heading that judge-pr reads is unchanged. Closes follow-up R25.

  Verified:
  - **Gates.** `bun run check` and `bun run gate` pass. `scripts/hooks/pr-watch.test.mjs` now has 42 checks (up from 32), and `scripts/judge-pr.test.mjs` has 19 (up from 18). Both also pass under node.
  - **Tests.** The new checks cover:
    - every repository spelling gh and git use;
    - `--repo` taking precedence over the PR URL;
    - the read at the PR head, including the exact `gh api` call, and the cwd's `ci.yml` left unread;
    - a GitHub Enterprise host;
    - each fallback and the note it carries.
  - **End to end.** One check runs the real watcher in a scratch git repository with a fake `gh` on PATH. The scratch `ci.yml` says `Local CI`, and the target's says `Pipeline`. The watcher goes green on `Pipeline`, both with `--repo` and with a bare PR URL, and sends `--repo` to `gh run list`. With the target unreadable, it reports a stated `CI` guess and never `Local CI`. With a remote that names the target, it uses `Local CI` and says so.
  - **Mutations.** Each change below was made on a scratch copy, and named tests failed for each:
    - the cwd's `ci.yml` winning (the reported bug): 6 tests failed;
    - runs not sent to the target repository;
    - a fallback with no note;
    - the local copy used without a matching remote;
    - the PR URL not treated as a target.

    The template test fails on rc's checklist line.
  - **Before and after.** Same fake setup, from a clone whose `ci.yml` says `Local CI`. rc's watcher printed `pending — no Local CI workflow run … yet`. This branch's printed `green … Pipeline run settled`.
  - **Live.** The watcher ran against #615 on GitHub, from a directory that is not a checkout (so it has no `ci.yml`), with `--repo outcrop-labs/talaria` and again with a bare PR URL. Both printed `green`. The JSON read `ciWorkflow: {name: "CI", source: "outcrop-labs/talaria@e6742a8dfa7c", note: ""}`, so the name was read from GitHub at the PR head, not guessed.
