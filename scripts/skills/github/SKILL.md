---
name: github
description: GitHub the Talaria way — plain git over https with injected credentials; there is no gh CLI and no auth to set up. Branch and PR; the workbench opens the PR.
---

# GitHub (Talaria)

There is no `gh` CLI in this container and no GitHub auth to configure —
Talaria injects the credential at git time, and it never appears in your
context, your output, or your disk. This skill is the signpost: the GitHub
methodology on this box is Talaria's, and it lives in two places.

- **Any git operation** — `git clone`, `pull`, and `push` over plain
  `https://` URLs just work for the repos granted to you. Never diagnose
  access by checking for configured auth (you will correctly find nothing);
  never run `gh auth login`, never create tokens or SSH keys. The full
  discipline, including what to do when an authenticated operation fails, is
  the **talaria-toolkit** skill's "Git and GitHub" section.
- **Engineering work on a repo** — drive it through the workbench
  (**workbench-driving** skill): `start_job`, work in the workdir it gives
  you, `finish_job` — Talaria verifies the branch and opens the pull request
  with the ticket-linked body, returning the PR URL for your outcome. `main`
  is protected and a human merges; never force-push, never rewrite shared
  history.

Issues, PR reviews beyond your own branch, and other API-shaped GitHub work
have no sanctioned agent path here. If a task needs them, that is a gap to
report (`report_gap`), not a workaround to build.
