- **Plugins are kept maintained: the workbench job sweep reads each plugin's PRs and puts its maintainer back to work, a merged change waits for an admin as an update approval, and a broken build, a host-upgrade break or an eval regression becomes a ticket for the maintainer.** The `workbench-job-sweep` now runs a plugin pass after its workspace passes. It never merges, pushes, comments on GitHub or edits a repository: its GitHub calls are all GETs.
  - **PR pass.** For each open PR on a GitHub plugin's repo it reads the checks verdict and the review decision (the `pr_status` read). Failing checks or a change request is a comment on the PR's ticket naming the failing checks and the reviewers. On a PR a workbench job opened, the maintainer is then assigned, the ticket moves out of review into the board's working column, and the ticket is offered to dispatch, which honours a person's stop. A PR somebody else opened is said on the plugin's ticket, and nobody is put on it. Checks are reported once per head commit and a change request once per review, so a fix pushed under a standing review does not wake the agent again. A PR that merges is a line on its ticket.
  - **Update pass.** When the plugin's branch moves past the installed commit, the sweep raises an `app_update` approval (admin-only, bounded to the plugin's board). It announces the approval at once, says so on the plugin's ticket naming the merged PRs, and keeps one waiting update per plugin. **Update** in Manage → Apps → Plugin updates runs `update_app`, which builds and probes the new version beside the live one, swaps it in and rolls back on failure. **Reject** takes a reason. A plugin set to `auto` is updated by the sweep itself. Each outcome is a line on the plugin's ticket, and a commit an admin rejected, or one that failed to apply, is not raised again.
  - **Notice tickets.** A failed build, a talaria.json this host refuses, a break after a host upgrade, and a worse band in the plugin's harness columns in Models → Fitness each become a ticket on the plugin's board, assigned to its maintainer. There is at most one open ticket per plugin per kind. A host-upgrade ticket names the `sdk` range the app needs and the version this host provides.
  - **Request a change and `app_harness_fitness`.** Every plugin card has **Request a change**, which files a ticket for the maintainer, created by the person asking (`POST /api/admin/apps/change-request`, for board editors and admins). A read-only `app_harness_fitness` tool returns the plugin's fitness columns and the host's harness report.

  Verified:
  - **Unit tests.** `cargo test`: talaria-plugins 13 + 3 ignored, talaria-approvals 19, talaria-workbench-mcp 54, talaria-routes-admin 16, talaria-jobs 2. The unit tests pin these rules:
    - checks are news once per head commit;
    - a change request is news once per review, not per push;
    - the comment names failing checks and reviewers and never offers a merge;
    - an SDK refusal and a new host build each read as a host-upgrade break;
    - only a worse band counts as a regression.

    Clippy `-D warnings` and fmt are clean on every touched crate.
  - **Live test.** The ignored test passed against a freshly migrated postgres 16 (486 statements; snapshot regenerated). It drove `upkeep_pass` against a loopback GitHub serving recorded PR answers, a `file://` repository and a mock app host:
    - PR #7, with a failing `test` check and ann's change request, produced one comment on its ticket. The ticket moved from review to `in_progress` with the maintainer assigned.
    - PR #8, opened by ann and failing `lint`, was said on the plugin's ticket with nobody put on it.
    - A second pass said nothing new.
    - After the merge and a new commit, `plugin PR #7 merged into main` and `… is on its default branch (merged: #7) — the update from … waits for an admin's approval` landed on the ticket, with one pending request, announced, carrying `prs: [7]`.
    - `decide_update` ran stage and activate and recorded `approved update: updated plugin … from … to …`.
    - A build pointer that was ready on host h1 and failed on h2 filed one host-upgrade ticket for the maintainer, and only one.
    - A fitness cell that went from ready to unfit filed an eval-regression ticket.
    - Every GitHub call was a GET.
  - **Worktree stack, real scheduler and app host.**
    - The first sweep applied the waiting update of `notes-plugin` (policy `auto`) from 2884e42 to 33b3a0e. It wrote `automatic update: updated plugin "notes-plugin" from 2884e42 to 33b3a0e` on PLUG-1, plus an audited `app.update` and an already-decided request row.
    - With the policy back on `approve`, a third commit pushed, the live manifest's `sdk` set to `^9.0`, and the plugin's fitness cell turned unfit, the next sweep logged `raised 1 plugin update approval(s), filed 2 plugin ticket(s)`. It filed PLUG-5, "broke after the host upgrade", naming `this app needs @talaria/sdk ^9.0; this host provides 1.0.0`. It filed PLUG-6, "harness evals regressed", naming `notes-plugin:summarize on gpt-oss-120b: ready → unfit`. Both were assigned to Nora Dev.
    - `app_harness_fitness` returned the column, and refused a missing slug.
  - **Chromium at 1440×900.**
    - Manage → Apps → Plugin updates showed `notes-plugin 33b3a0e → 412b790`. **Update** answered `applied`, swapped the tree to 412b790 (restoring `sdk: ^1.0`) and wrote the line on PLUG-1.
    - **Request a change** filed PLUG-4, assigned to Nora Dev and created by the admin, and linked to it.
    - At 390×844 the open form does not scroll sideways.
